use nalgebra::{Vector2, Vector3};
use serde_json::json;
use spectral_caustics::{
    camera::Camera,
    config::{
        Config,
        cameraconfig::CameraConfig,
        materialconfig::MaterialConfig,
        sceneconfig::{EnvironmentMisHeuristic, ObjectConfig},
    },
    geometry::{beveled_prism::BeveledPrism, object::Hittable},
    render::{Render, framebuffer::Framebuffer},
    scene::Scene,
};

fn config() -> Config {
    let mut config = Config::from_json(include_str!("../configs/beveled_prism.json")).unwrap();
    config.render.width = 20;
    config.render.height = 16;
    config.render.camera_spp = 8;
    config.render.samples_per_pixel_axis = 1;
    config.render.thread_count = Some(1);
    config.scene.environment_samples = 3;
    config.scene.objects.swap(0, 1);
    config.scene.debug_object_id = Some(1);
    config
}

fn materials() -> [MaterialConfig; 4] {
    [
        MaterialConfig::Dielectric { ior: 1.52 },
        MaterialConfig::Mirror {
            reflectance: [0.95; 3],
        },
        MaterialConfig::GgxMetal {
            eta: [0.2; 3],
            k: [3.0; 3],
            roughness: 0.0,
        },
        MaterialConfig::GgxMetal {
            eta: [0.2; 3],
            k: [3.0; 3],
            roughness: 0.4,
        },
    ]
}

#[test]
fn json_roundtrips_builds_materials_and_keeps_debug_target_separate_from_spectral_prism() {
    for bevel in [0.02, 0.035, 0.05] {
        for material in materials() {
            let mut config = config();
            if let ObjectConfig::BeveledPrism {
                material: slot,
                bevel_size_m,
                ..
            } = &mut config.scene.objects[1]
            {
                *slot = material;
                *bevel_size_m = bevel;
            }
            let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
            assert_eq!(loaded, config);
            let scene = Scene::from_config(&loaded.scene).unwrap();
            assert_eq!(
                (scene.prism_id, scene.ground_id, scene.debug_object_id),
                (None, Some(0), Some(1))
            );
            assert!(
                scene.objects[1]
                    .material_id()
                    .and_then(|id| scene.material(id))
                    .is_some()
            );
        }
    }
    let mut config = config();
    config
        .scene
        .objects
        .push(Config::default().scene.objects.remove(0));
    let scene = Scene::from_config(&config.scene).unwrap();
    assert_eq!(scene.prism_id, Some(2));
    assert_eq!(scene.debug_object_id, Some(1));
    let mut json = serde_json::to_value(config).unwrap();
    json["scene"]["objects"][1]
        .as_object_mut()
        .unwrap()
        .remove("material");
    let loaded = Config::from_json(&json.to_string()).unwrap();
    assert!(matches!(
        loaded.scene.objects[1],
        ObjectConfig::BeveledPrism {
            material: MaterialConfig::Dielectric { ior: 1.52 },
            ..
        }
    ));
}

#[test]
fn invalid_json_geometry_and_material_are_rejected_before_rendering() {
    let base = serde_json::to_value(config()).unwrap();
    for (field, value) in [
        ("bevel_size_m", json!(0)),
        ("bevel_size_m", json!(-0.01)),
        ("bevel_size_m", json!(0.5)),
        ("bevel_size_m", json!(1e200)),
        ("bevel_size_m", json!(null)),
        ("cross_section_m", json!([[0, 0], [1, 0], [2, 0]])),
        ("cross_section_m", json!([[0, 0], [1, 0]])),
        ("cross_section_m", json!([[0, null], [1, 0], [0, 1]])),
        ("y_min_m", json!(1)),
        ("y_max_m", json!(-1)),
        ("material", json!({"type":"mirror","reflectance":[2,1,1]})),
        ("radius_m", json!(0.03)),
    ] {
        let mut bad = base.clone();
        bad["scene"]["objects"][1][field] = value;
        assert!(Config::from_json(&bad.to_string()).is_err(), "{field}");
    }
    for field in ["bevel_size_m", "cross_section_m", "y_min_m", "y_max_m"] {
        let mut bad = base.clone();
        bad["scene"]["objects"][1]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(Config::from_json(&bad.to_string()).is_err());
    }
    let mut bad = config();
    if let ObjectConfig::BeveledPrism { bevel_size_m, .. } = &mut bad.scene.objects[1] {
        *bevel_size_m = f64::NAN;
    }
    assert!(bad.to_json().is_err());
    assert!(Scene::from_config(&bad.scene).is_err());
}

#[test]
fn camera_center_ray_hits_each_new_bevel_face_at_both_test_sizes() {
    let triangle = [
        Vector2::new(-0.5, 1.0),
        Vector2::new(0.5, 1.0),
        Vector2::new(0.0, 1.8),
    ];
    for bevel in [0.02, 0.05] {
        let prism = BeveledPrism::new(triangle, -1.0, 1.0, bevel).unwrap();
        for i in 0..3 {
            let previous = triangle[i] + (triangle[(i + 2) % 3] - triangle[i]).normalize() * bevel;
            let next = triangle[i] + (triangle[(i + 1) % 3] - triangle[i]).normalize() * bevel;
            let midpoint = (previous + next) * 0.5;
            let edge = next - previous;
            let normal = Vector3::new(edge.y, 0.0, -edge.x).normalize();
            let point = Vector3::new(midpoint.x, 0.25, midpoint.y);
            let camera = Camera::from_config(&CameraConfig {
                position: (point + normal * 0.3).into(),
                target: point.into(),
                up: [0.0, 1.0, 0.0],
                fov_y_degrees: 35.0,
            })
            .unwrap();
            let ray = camera.ray_for_pixel(0, 0, 1, 1).unwrap();
            let hit = prism.intersect(&ray).unwrap();
            assert!((hit.distance - 0.3).abs() < 1e-12);
            assert!((hit.coords - point).norm() < 1e-12);
            assert!((hit.normal - normal).norm() < 1e-12);
            assert!(hit.normal.dot(&ray.direction) < 0.0);
        }
    }
}

fn identical(a: &Framebuffer, b: &Framebuffer) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    assert!(a.pixels.iter().zip(&b.pixels).all(|(a, b)| {
        a.iter()
            .zip(b.iter())
            .all(|(a, b)| a.to_bits() == b.to_bits())
    }));
}

#[test]
fn existing_materials_and_debug_reuse_rgb_paths_across_mis_depths_and_threads() {
    for material in materials() {
        let mut config = config();
        if let ObjectConfig::BeveledPrism { material: slot, .. } = &mut config.scene.objects[1] {
            *slot = material.clone();
        }
        let mut scene = Scene::from_config(&config.scene).unwrap();
        let lighting = Render::new(config.render.clone())
            .unwrap()
            .render_lighting(&scene)
            .unwrap();
        assert!(
            lighting
                .caustics
                .pixels
                .iter()
                .all(|p| *p == Vector3::zeros())
        );
        for heuristic in [
            EnvironmentMisHeuristic::Balance,
            EnvironmentMisHeuristic::Power,
        ] {
            scene.environment_mis_heuristic = heuristic;
            for depth in [0, 1, 4] {
                scene.max_diffuse_bounces = depth;
                config.render.debug_path = false;
                let ordinary = Render::new(config.render.clone())
                    .unwrap()
                    .render_camera_glass_components(&scene, &lighting, 8, 42)
                    .unwrap();
                let mut reference = None;
                for threads in [1, 2, 4, 1] {
                    config.render.thread_count = Some(threads);
                    config.render.debug_path = true;
                    let images = Render::new(config.render.clone())
                        .unwrap()
                        .render_camera_glass_components(&scene, &lighting, 8, 42)
                        .unwrap();
                    identical(&images.direct, &ordinary.direct);
                    identical(&images.caustics, &ordinary.caustics);
                    identical(&images.combined, &ordinary.combined);
                    for ((direct, caustics), combined) in images
                        .direct
                        .pixels
                        .iter()
                        .zip(&images.caustics.pixels)
                        .zip(&images.combined.pixels)
                    {
                        assert!((direct + caustics - combined).norm() < 1e-12);
                    }
                    if let Some(debug) = &images.path_debug {
                        assert_eq!(debug.target_object_id, Some(1));
                        assert!(debug.pixels.iter().any(|p| p.target_samples > 0));
                        assert!(
                            debug
                                .pixels
                                .iter()
                                .all(|p| p.terminations.values().sum::<u64>() == p.target_samples)
                        );
                        let events = |f: fn(
                            &spectral_caustics::render::integrator::debug::PathDebugPixel,
                        ) -> u64| {
                            debug.pixels.iter().map(f).sum::<u64>()
                        };
                        match material {
                            MaterialConfig::Dielectric { .. } => {
                                assert!(events(|p| p.events.specular_transmission) > 0)
                            }
                            MaterialConfig::GgxMetal { roughness, .. } if roughness > 0.0 => {
                                assert!(events(|p| p.events.glossy_reflection) > 0 || depth == 0)
                            }
                            _ => assert!(events(|p| p.events.specular_reflection) > 0),
                        }
                        if let Some(expected) = &reference {
                            assert_eq!(debug, expected);
                        }
                    }
                    reference = images.path_debug;
                }
            }
        }
    }
}

use crate::support::{assert_camera_images_bits_eq, test_materials as materials};
use nalgebra::Vector2;
use nalgebra::Vector3;
use serde_json::json;
use spectral_caustics::camera::Camera;
use spectral_caustics::config::Config;
use spectral_caustics::config::camera::CameraConfig;
#[cfg(feature = "diagnostics")]
use spectral_caustics::config::material::MaterialConfig;
use spectral_caustics::config::scene::EnvironmentMisHeuristic;
use spectral_caustics::config::scene::ObjectConfig;
use spectral_caustics::geometry::beveled_prism::BeveledPrism;
use spectral_caustics::geometry::beveled_prism::ROUND_SEGMENTS;
use spectral_caustics::geometry::object::Hittable;
use spectral_caustics::renderer::Renderer;

fn config() -> Config {
    let mut config = Config::from_json(include_str!("../../configs/beveled_prism.json")).unwrap();
    config.render.width = 20;
    config.render.height = 16;
    config.render.camera_spp = 8;
    config.render.samples_per_pixel_axis = 1;
    config.render.thread_count = Some(1);
    config.scene.environment.as_mut().unwrap().map_path = None;
    config.scene.environment_samples = 3;
    config.scene.objects.swap(0, 1);
    #[cfg(feature = "diagnostics")]
    {
        config.render.diagnostics.target = Some(1);
    }
    config
}

#[cfg(feature = "diagnostics")]
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
            let scene = loaded.build().unwrap();
            assert_eq!(
                (
                    scene.photon.caustics_object_id,
                    scene.photon.ground_id,
                    scene.diagnostic_target
                ),
                (None, Some(0), Some(1))
            );
            assert_eq!(scene.scene.objects[1].material_id(), 1);
        }
    }
    let mut config = config();
    config
        .scene
        .objects
        .push(Config::default().scene.objects.remove(0));
    let scene = config.build().unwrap();
    assert_eq!(scene.photon.caustics_object_id, Some(2));
    assert_eq!(scene.diagnostic_target, Some(1));
    let mut json = serde_json::to_value(config).unwrap();
    json["scene"]["objects"][1]
        .as_object_mut()
        .unwrap()
        .remove("material");
    let loaded = Config::from_json(&json.to_string()).unwrap();
    assert!(matches!(
        loaded.scene.objects[1],
        ObjectConfig::BeveledPrism {
            material: MaterialConfig::Dielectric {
                ior: 1.52,
                dispersion: Some(_)
            },
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
        assert!(
            Config::from_json(&bad.to_string())
                .and_then(|config| {
                    config.validate()?;
                    Ok(config)
                })
                .is_err(),
            "{field}"
        );
    }
    for field in ["bevel_size_m", "cross_section_m", "y_min_m", "y_max_m"] {
        let mut bad = base.clone();
        bad["scene"]["objects"][1]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            Config::from_json(&bad.to_string())
                .and_then(|config| {
                    config.validate()?;
                    Ok(config)
                })
                .is_err()
        );
    }
    let mut bad = config();
    if let ObjectConfig::BeveledPrism { bevel_size_m, .. } = &mut bad.scene.objects[1] {
        *bevel_size_m = f64::NAN;
    }
    assert!(bad.to_json().is_err());
    assert!(bad.build().is_err());
}

#[test]
fn camera_center_ray_hits_all_eight_facets_of_each_round_corner_at_both_test_sizes() {
    let triangle = [
        Vector2::new(-0.5, 1.0),
        Vector2::new(0.5, 1.0),
        Vector2::new(0.0, 1.8),
    ];
    for bevel in [0.02, 0.05] {
        let prism = BeveledPrism::new(triangle, -1.0, 1.0, bevel).unwrap();
        for facet in
            (0..3 * (ROUND_SEGMENTS + 1)).filter(|i| i % (ROUND_SEGMENTS + 1) < ROUND_SEGMENTS)
        {
            let a = prism.vertices()[facet];
            let b = prism.vertices()[facet + 1];
            let midpoint = (a + b) * 0.5;
            let edge = b - a;
            let normal = Vector3::new(edge.z, 0.0, -edge.x).normalize();
            let point = Vector3::new(midpoint.x, 0.25, midpoint.z);
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

#[test]
fn existing_materials_and_debug_reuse_rgb_paths_across_mis_depths_and_threads() {
    let configs = [
        (EnvironmentMisHeuristic::Balance, 0),
        (EnvironmentMisHeuristic::Power, 1),
        (EnvironmentMisHeuristic::Balance, 4),
        (EnvironmentMisHeuristic::Power, 1),
    ];
    for (material, (heuristic, depth)) in materials().into_iter().zip(configs) {
        let mut config = config();
        config.render.width = 12;
        config.render.height = 10;
        config.render.camera_spp = 4;
        if let ObjectConfig::BeveledPrism { material: slot, .. } = &mut config.scene.objects[1] {
            *slot = material.clone();
        }
        let mut scene = config.build().unwrap();
        let lighting = spectral_caustics::integrator::photon::integrate_xyz(
            &scene,
            scene.photon.caustics_object_id,
            scene.photon.ground_id.expect("receiver"),
            &config.render,
        )
        .unwrap();
        assert!(
            lighting
                .caustics
                .pixels()
                .iter()
                .all(|p| *p == Vector3::zeros())
        );
        scene.camera_options.environment_mis_heuristic = heuristic;
        scene.camera_options.max_diffuse_bounces = depth;
        #[cfg(feature = "diagnostics")]
        {
            config.render.diagnostics.path = false;
        }
        let ordinary = Renderer::new(config.render.clone())
            .unwrap()
            .render(&scene)
            .unwrap();
        #[cfg(feature = "diagnostics")]
        let mut reference = None;
        for threads in [1, 2, 4] {
            config.render.thread_count = Some(threads);
            #[cfg(feature = "diagnostics")]
            {
                config.render.diagnostics.path = true;
            }
            let images = Renderer::new(config.render.clone())
                .unwrap()
                .render(&scene)
                .unwrap();
            assert_camera_images_bits_eq(&images, &ordinary);
            for ((direct, caustics), combined) in images
                .direct
                .pixels()
                .iter()
                .zip(images.caustics.pixels())
                .zip(images.combined.pixels())
            {
                assert!((direct + caustics - combined).norm() < 1e-12);
            }
            #[cfg(feature = "diagnostics")]
            if let Some(debug) = &images.path_debug {
                assert_eq!(debug.target_object_id, Some(1));
                assert!(debug.pixels.iter().any(|p| p.target_samples > 0));
                assert!(
                    debug
                        .pixels
                        .iter()
                        .all(|p| p.terminations.values().sum::<u64>() == p.target_samples)
                );
                let events = |f: fn(&spectral_caustics::diagnostics::PathDebugPixel) -> u64| {
                    debug.pixels.iter().map(f).sum::<u64>()
                };
                match material {
                    MaterialConfig::Dielectric { .. } => {
                        assert!(events(|p| p.events.specular_transmission) > 0)
                    }
                    MaterialConfig::GgxMetal { roughness, .. } if roughness > 0.0 => {
                        assert!(events(|p| p.events.glossy_reflection) > 0)
                    }
                    _ => assert!(events(|p| p.events.specular_reflection) > 0),
                }
                if let Some(expected) = &reference {
                    assert_eq!(debug, expected);
                }
            }
            #[cfg(feature = "diagnostics")]
            {
                reference = images.path_debug;
            }
        }
    }
}

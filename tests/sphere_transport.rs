use nalgebra::Vector3;
use rand::RngCore;
use serde_json::json;
use spectral_caustics::{
    config::{
        Config,
        materialconfig::MaterialConfig,
        sceneconfig::{EnvironmentMisHeuristic, ObjectConfig},
    },
    geometry::{object::Hittable, ray::Ray, sphere::Sphere},
    material::{Material, ScatterType, dielectric::Dielectric},
    render::{Render, framebuffer::Framebuffer, render_camera_glass},
    scene::Scene,
};

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

fn config(material: MaterialConfig) -> Config {
    let mut config = Config::from_json(include_str!("../configs/sphere.json")).unwrap();
    config.render.width = 16;
    config.render.height = 12;
    config.render.camera_spp = 8;
    config.render.samples_per_pixel_axis = 1;
    config.render.thread_count = Some(1);
    config.scene.environment_samples = 3;
    // 目标置于非零编号，避免把球体默认为 prism_id 或 objects[0]。
    config.scene.objects.swap(0, 1);
    config.scene.debug_object_id = Some(1);
    if let ObjectConfig::Sphere { material: slot, .. } = &mut config.scene.objects[1] {
        *slot = material;
    }
    config
}

#[test]
fn sphere_json_binds_all_materials_and_keeps_prism_and_debug_ids_independent() {
    for material in materials() {
        let config = config(material.clone());
        let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
        assert_eq!(loaded, config);
        let scene = Scene::from_config(&loaded.scene).unwrap();
        assert_eq!(
            (scene.prism_id, scene.ground_id, scene.debug_object_id),
            (None, Some(0), Some(1))
        );
        let hit = scene
            .intersect(&Ray::new(Vector3::new(0.0, -3.0, 0.83), Vector3::y()).unwrap())
            .unwrap();
        assert_eq!(hit.object_id, Some(1));
        let bound = scene.material(hit.material_id.unwrap()).unwrap();
        assert_eq!(
            bound.ior(),
            if let MaterialConfig::Dielectric { ior } = material {
                Some(ior)
            } else {
                None
            }
        );
        assert_eq!(
            bound.has_non_delta(),
            matches!(material, MaterialConfig::GgxMetal { roughness, .. } if roughness > 0.001)
        );
    }
    let mut config = config(materials()[0].clone());
    let prism = Config::default().scene.objects.remove(0);
    config.scene.objects.push(prism);
    let scene = Scene::from_config(&config.scene).unwrap();
    assert_eq!(scene.prism_id, Some(2));
    assert_eq!(scene.debug_object_id, Some(1));
    config.scene.debug_object_id = None;
    assert_eq!(
        Scene::from_config(&config.scene).unwrap().debug_object_id,
        Some(2)
    );
    config.scene.objects.pop();
    assert_eq!(
        Scene::from_config(&config.scene).unwrap().debug_object_id,
        None
    );

    let mut json = serde_json::to_value(&config).unwrap();
    json["scene"]["objects"][1]
        .as_object_mut()
        .unwrap()
        .remove("material");
    let loaded = Config::from_json(&json.to_string()).unwrap();
    assert!(matches!(
        loaded.scene.objects[1],
        ObjectConfig::Sphere {
            material: MaterialConfig::Dielectric { ior: 1.52 },
            ..
        }
    ));
}

#[test]
fn sphere_json_rejects_invalid_geometry_materials_fields_and_debug_targets() {
    let config = config(materials()[0].clone());
    let base = serde_json::to_value(&config).unwrap();
    for (field, value) in [
        ("radius_m", json!(0)),
        ("radius_m", json!(-1)),
        ("radius_m", json!(1e200)),
        ("radius_m", json!(1e-200)),
        ("radius_m", json!(null)),
        ("radius_m", json!("1")),
        ("center_m", json!([0, 0])),
        ("center_m", json!([0, null, 1])),
        ("material", json!({"type":"dielectric","ior":-1})),
        ("vertices_m", json!([])),
    ] {
        let mut bad = base.clone();
        bad["scene"]["objects"][1][field] = value;
        assert!(Config::from_json(&bad.to_string()).is_err(), "{field}");
    }
    for field in ["center_m", "radius_m"] {
        let mut bad = base.clone();
        bad["scene"]["objects"][1]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(Config::from_json(&bad.to_string()).is_err());
    }
    for value in [json!(2), json!(-1), json!(0.5), json!("sphere")] {
        let mut bad = base.clone();
        bad["scene"]["debug_object_id"] = value;
        assert!(Config::from_json(&bad.to_string()).is_err());
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut bad = config.clone();
        if let ObjectConfig::Sphere { center_m, .. } = &mut bad.scene.objects[1] {
            center_m[0] = value;
        }
        assert!(bad.to_json().is_err());
        assert!(Scene::from_config(&bad.scene).is_err());
    }
}

struct Transmit;
impl RngCore for Transmit {
    fn next_u32(&mut self) -> u32 {
        u32::MAX
    }
    fn next_u64(&mut self) -> u64 {
        u64::MAX
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        dest.fill(255);
    }
}

#[test]
fn sphere_two_interfaces_match_analytic_snell_direction_and_cancel_eta_squared() {
    let sphere = Sphere::new(Vector3::zeros(), 1.0).unwrap();
    let glass = Dielectric::new(1.5).unwrap();
    for impact in [0.0_f64, 0.6, 0.95] {
        let ray = Ray::new(Vector3::new(-3.0, impact, 0.0), Vector3::x()).unwrap();
        let enter = sphere.intersect(&ray).unwrap();
        assert!(enter.normal.dot(&ray.direction) < 0.0);
        let inside = glass
            .sample(-ray.direction, enter.normal, &mut Transmit)
            .unwrap();
        assert_eq!(inside.event, ScatterType::SpecularTransmission);
        assert!((inside.weight.x - (1.0_f64 / 1.5).powi(2)).abs() < 1e-14);
        // 从真实表面继续，Hittable 的 epsilon 排除刚命中的根。
        let exit = sphere
            .intersect(&Ray::new(enter.coords, inside.wi).unwrap())
            .unwrap();
        assert!(exit.normal.dot(&inside.wi) > 0.0);
        let outside = glass
            .sample(-inside.wi, exit.normal, &mut Transmit)
            .unwrap();
        assert_eq!(outside.event, ScatterType::SpecularTransmission);
        let incidence = impact.asin();
        let refraction = (impact / 1.5).asin();
        let exit_angle = 2.0 * refraction - incidence;
        assert!(
            (exit.coords - Vector3::new(exit_angle.cos(), exit_angle.sin(), 0.0)).norm() < 1e-12
        );
        let deflection = 2.0 * (refraction - incidence);
        assert!(
            (outside.wi - Vector3::new(deflection.cos(), deflection.sin(), 0.0)).norm() < 1e-12
        );
        assert!(
            (inside.weight.component_mul(&outside.weight) - Vector3::repeat(1.0)).norm() < 1e-12
        );
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
fn sphere_rgb_components_and_debug_repeat_across_threads_mis_depths_and_seeds() {
    for material in materials() {
        let mut config = config(material);
        let mut scene = Scene::from_config(&config.scene).unwrap();
        let renderer = Render::new(config.render.clone()).unwrap();
        let lighting = renderer.render_lighting(&scene).unwrap();
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
                for seed in [42, 123] {
                    config.render.debug_path = false;
                    let ordinary = Render::new(config.render.clone())
                        .unwrap()
                        .render_camera_glass_components(&scene, &lighting, 8, seed)
                        .unwrap();
                    assert!(ordinary.path_debug.is_none());
                    let mut previous = None;
                    for threads in [1, 2, 4, 1] {
                        config.render.thread_count = Some(threads);
                        config.render.debug_path = true;
                        let renderer = Render::new(config.render.clone()).unwrap();
                        let images = renderer
                            .render_camera_glass_components(&scene, &lighting, 8, seed)
                            .unwrap();
                        identical(&ordinary.direct, &images.direct);
                        identical(&ordinary.caustics, &images.caustics);
                        identical(&ordinary.combined, &images.combined);
                        assert!(images.combined.pixels.iter().any(|p| p.norm() > 0.0));
                        for ((a, b), c) in images
                            .direct
                            .pixels
                            .iter()
                            .zip(&images.caustics.pixels)
                            .zip(&images.combined.pixels)
                        {
                            assert!((a + b - c).norm() < 1e-12);
                        }
                        if let Some(debug) = &images.path_debug {
                            assert_eq!(debug.target_object_id, Some(1));
                            assert!(debug.pixels.iter().any(|p| p.target_samples > 0));
                            for pixel in &debug.pixels {
                                assert_eq!(pixel.samples, 8);
                                assert_eq!(
                                    pixel.target_samples,
                                    *pixel.first_hit_objects.get(&1).unwrap_or(&0)
                                );
                                assert_eq!(
                                    pixel.terminations.values().sum::<u64>(),
                                    pixel.target_samples
                                );
                            }
                            if let Some(expected) = &previous {
                                assert_eq!(debug, expected);
                            }
                        }
                        previous = images.path_debug;
                    }
                }
            }
        }
        // 独立 RGB 入口也能直接追踪球体，不需要光源焦散图。
        assert!(
            render_camera_glass(&scene, 8, 6, 4, 42)
                .unwrap()
                .pixels
                .iter()
                .any(|p| p.norm() > 0.0)
        );
    }
}

#[test]
fn adding_a_distant_sphere_preserves_prism_spectral_caustics() {
    let mut config = Config::default();
    config.render.width = 24;
    config.render.height = 18;
    config.render.samples_per_pixel_axis = 1;
    config.render.thread_count = Some(2);
    let renderer = Render::new(config.render.clone()).unwrap();
    let ordinary = Scene::from_config(&config.scene).unwrap();
    let expected = renderer.render_lighting(&ordinary).unwrap();
    assert!(expected.caustics.pixels.iter().any(|p| p.norm() > 0.0));
    config.scene.objects.push(ObjectConfig::Sphere {
        center_m: [100.0, 100.0, 1.0],
        radius_m: 0.8,
        material: materials()[0].clone(),
    });
    config.scene.debug_object_id = Some(2);
    let scene = Scene::from_config(&config.scene).unwrap();
    assert_eq!(scene.prism_id, ordinary.prism_id);
    let actual = renderer.render_lighting(&scene).unwrap();
    identical(&expected.direct, &actual.direct);
    identical(&expected.caustics, &actual.caustics);
}

#[test]
fn debug_target_can_switch_between_sphere_ground_and_none_without_changing_radiance() {
    let config = config(materials()[0].clone());
    let mut scene = Scene::from_config(&config.scene).unwrap();
    let renderer = Render::new(config.render.clone()).unwrap();
    let lighting = renderer.render_lighting(&scene).unwrap();
    let reference = renderer
        .render_camera_glass_components(&scene, &lighting, 8, 42)
        .unwrap();
    for target in [Some(1), Some(0), None] {
        scene.debug_object_id = target;
        let images = renderer
            .render_camera_glass_components(&scene, &lighting, 8, 42)
            .unwrap();
        identical(&images.direct, &reference.direct);
        identical(&images.caustics, &reference.caustics);
        identical(&images.combined, &reference.combined);
        if let Some(debug) = images.path_debug {
            assert_eq!(debug.target_object_id, target);
            assert_eq!(
                debug.pixels.iter().any(|p| p.target_samples > 0),
                target.is_some()
            );
            for pixel in debug.pixels {
                assert_eq!(
                    pixel.target_samples,
                    target
                        .and_then(|id| pixel.first_hit_objects.get(&id).copied())
                        .unwrap_or(0)
                );
                assert_eq!(
                    pixel.terminations.values().sum::<u64>(),
                    pixel.target_samples
                );
            }
        }
    }
    scene.debug_object_id = Some(scene.objects.len());
    assert_eq!(
        renderer
            .render_camera_glass_components(&scene, &lighting, 8, 42)
            .is_err(),
        cfg!(debug_assertions)
    );
}

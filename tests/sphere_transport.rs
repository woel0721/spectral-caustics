mod support;
use nalgebra::Vector3;
use serde_json::json;
use spectral_caustics::config::Config;
use spectral_caustics::config::material::MaterialConfig;
#[cfg(feature = "diagnostics")]
use spectral_caustics::config::scene::EnvironmentMisHeuristic;
use spectral_caustics::config::scene::ObjectConfig;
use spectral_caustics::geometry::object::Hittable;
use spectral_caustics::geometry::ray::Ray;
use spectral_caustics::geometry::sphere::Sphere;
use spectral_caustics::material::Material;
use spectral_caustics::material::ScatterType;
use spectral_caustics::material::dielectric::Dielectric;
use support::{TransmissionRng as Transmit, test_materials as materials};
#[cfg(feature = "diagnostics")]
use support::{assert_camera_images_bits_eq, assert_framebuffer_bits_eq};

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
    #[cfg(feature = "diagnostics")]
    {
        config.render.diagnostics.target = Some(1);
    }
    if let ObjectConfig::Sphere {
        material: slot,
        caustics,
        ..
    } = &mut config.scene.objects[1]
    {
        *slot = material;
        *caustics = false;
    }
    config
}

#[cfg(feature = "diagnostics")]
#[test]
fn sphere_json_binds_all_materials_and_keeps_prism_and_debug_ids_independent() {
    for material in materials() {
        let config = config(material.clone());
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
        let hit = scene
            .scene
            .intersect(&Ray::new(Vector3::new(0.0, -3.0, 0.83), Vector3::y()).unwrap())
            .unwrap();
        assert_eq!(hit.object_id, 1);
        let bound = scene.scene.material(hit.material_id);
        assert_eq!(
            bound.ior(),
            if let MaterialConfig::Dielectric { ior, .. } = material {
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
    let scene = config.build().unwrap();
    assert_eq!(scene.photon.caustics_object_id, Some(2));
    assert_eq!(scene.diagnostic_target, Some(1));
    #[cfg(feature = "diagnostics")]
    {
        config.render.diagnostics.target = None;
    }
    assert_eq!(config.build().unwrap().diagnostic_target, Some(2));
    config.scene.objects.pop();
    assert_eq!(config.build().unwrap().diagnostic_target, None);

    let mut json = serde_json::to_value(&config).unwrap();
    json["scene"]["objects"][1]
        .as_object_mut()
        .unwrap()
        .remove("material");
    let loaded = Config::from_json(&json.to_string()).unwrap();
    assert!(matches!(
        loaded.scene.objects[1],
        ObjectConfig::Sphere {
            material: MaterialConfig::Dielectric { ior: 1.52, .. },
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
    for field in ["center_m", "radius_m"] {
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
    for value in [json!(2), json!(-1), json!(0.5), json!("sphere")] {
        let mut bad = base.clone();
        bad["scene"]["debug_object_id"] = value;
        assert!(
            Config::from_json(&bad.to_string())
                .and_then(|config| {
                    config.validate()?;
                    Ok(config)
                })
                .is_err()
        );
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut bad = config.clone();
        if let ObjectConfig::Sphere { center_m, .. } = &mut bad.scene.objects[1] {
            center_m[0] = value;
        }
        assert!(bad.to_json().is_err());
        assert!(bad.build().is_err());
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

#[cfg(feature = "diagnostics")]
#[test]
fn sphere_rgb_components_and_debug_repeat_across_threads_mis_depths_and_seeds() {
    for material in materials() {
        let mut config = config(material);
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
        for heuristic in [
            EnvironmentMisHeuristic::Balance,
            EnvironmentMisHeuristic::Power,
        ] {
            scene.camera_options.environment_mis_heuristic = heuristic;
            for depth in [0, 1, 4] {
                scene.camera_options.max_diffuse_bounces = depth;
                for seed in [42, 123] {
                    config.render.camera_spp = 8;
                    config.render.seed = seed;
                    #[cfg(feature = "diagnostics")]
                    {
                        config.render.diagnostics.path = false;
                    }
                    let ordinary = spectral_caustics::integrator::camera::integrate_rgb(
                        &scene,
                        &config.render,
                        Some(&lighting),
                    )
                    .unwrap();
                    assert!(ordinary.path_debug.is_none());
                    let mut previous = None;
                    for threads in [1, 2, 4, 1] {
                        config.render.thread_count = Some(threads);
                        #[cfg(feature = "diagnostics")]
                        {
                            config.render.diagnostics.path = true;
                        }
                        let images = support::run_in_threads(threads, || {
                            spectral_caustics::integrator::camera::integrate_rgb(
                                &scene,
                                &config.render,
                                Some(&lighting),
                            )
                            .unwrap()
                        });
                        assert_camera_images_bits_eq(&ordinary, &images);
                        assert!(images.combined.pixels().iter().any(|p| p.norm() > 0.0));
                        for ((a, b), c) in images
                            .direct
                            .pixels()
                            .iter()
                            .zip(images.caustics.pixels())
                            .zip(images.combined.pixels())
                        {
                            assert!((a + b - c).norm() < 1e-12);
                        }
                        #[cfg(feature = "diagnostics")]
                        {
                            let debug = images
                                .path_debug
                                .as_ref()
                                .expect("path diagnostics must be populated");
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
            spectral_caustics::integrator::camera::integrate_rgb(
                &scene,
                &spectral_caustics::config::render::RenderSetting {
                    width: 8,
                    height: 6,
                    camera_spp: 4,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .unwrap()
            .pixels()
            .iter()
            .any(|p| p.norm() > 0.0)
        );
    }
}

#[cfg(feature = "diagnostics")]
#[test]
fn adding_a_distant_sphere_preserves_prism_spectral_caustics() {
    let mut config = Config::default();
    config.render.width = 24;
    config.render.height = 18;
    config.render.samples_per_pixel_axis = 1;
    config.render.thread_count = Some(2);
    let ordinary = config.build().unwrap();
    let expected = spectral_caustics::integrator::photon::integrate_xyz(
        &ordinary,
        ordinary.photon.caustics_object_id,
        ordinary.photon.ground_id.expect("receiver"),
        &config.render,
    )
    .unwrap();
    assert!(expected.caustics.pixels().iter().any(|p| p.norm() > 0.0));
    config.scene.objects.push(ObjectConfig::Sphere {
        caustics: false,
        medium_interface: None,
        center_m: [100.0, 100.0, 1.0],
        radius_m: 0.8,
        material: materials()[0].clone(),
    });
    #[cfg(feature = "diagnostics")]
    {
        config.render.diagnostics.target = Some(2);
    }
    let scene = config.build().unwrap();
    assert_eq!(
        scene.photon.caustics_object_id,
        ordinary.photon.caustics_object_id
    );
    let actual = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        &config.render,
    )
    .unwrap();
    assert_framebuffer_bits_eq(
        expected.direct.as_ref().unwrap(),
        actual.direct.as_ref().unwrap(),
    );
    assert_framebuffer_bits_eq(&expected.caustics, &actual.caustics);
}

#[cfg(feature = "diagnostics")]
#[test]
fn debug_target_can_switch_between_sphere_ground_and_none_without_changing_radiance() {
    let mut config = config(materials()[0].clone());
    config.render.camera_spp = 8;
    config.render.seed = 42;
    let mut scene = config.build().unwrap();
    let lighting = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        &config.render,
    )
    .unwrap();
    config.render.diagnostics.path = false;
    let reference = spectral_caustics::integrator::camera::integrate_rgb(
        &scene,
        &config.render,
        Some(&lighting),
    )
    .unwrap();
    config.render.diagnostics.path = true;
    for target in [Some(1), Some(0), None] {
        scene.diagnostic_target = target;
        let images = spectral_caustics::integrator::camera::integrate_rgb(
            &scene,
            &config.render,
            Some(&lighting),
        )
        .unwrap();
        assert_camera_images_bits_eq(&images, &reference);
        let debug = images
            .path_debug
            .expect("path diagnostics must be populated");
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
    scene.diagnostic_target = Some(scene.scene.objects.len());
    assert!(
        spectral_caustics::integrator::camera::integrate_rgb(
            &scene,
            &config.render,
            Some(&lighting)
        )
        .is_err()
    );
    assert!(
        spectral_caustics::integrator::camera::integrate_spectral(&scene, &config.render, None)
            .is_err()
    );
}

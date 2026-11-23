use crate::support::assert_camera_images_bits_eq;
use nalgebra::Vector3;
use rand::SeedableRng;
use rand::rngs::StdRng;
use spectral_caustics::config::RenderJob;
use spectral_caustics::config::material::MaterialConfig;
use spectral_caustics::config::render::RenderSetting;
use spectral_caustics::config::scene::ObjectConfig;
use spectral_caustics::config::scene::SceneConfig;
use spectral_caustics::integrator::photon::integrate_xyz;
use spectral_caustics::material;
use spectral_caustics::material::ScatterType;

#[test]
fn mirror_json_roundtrip_builds_trait_object_and_validates_parameters() {
    let config: MaterialConfig =
        serde_json::from_str(r#"{"type":"mirror","reflectance":[0.2,0.5,0.9]}"#).unwrap();
    config.validate().unwrap();
    assert_eq!(
        serde_json::from_str::<MaterialConfig>(&serde_json::to_string(&config).unwrap()).unwrap(),
        config
    );
    let material = material::from_config(&config).unwrap();
    let s = material
        .sample(Vector3::z(), Vector3::z(), &mut StdRng::seed_from_u64(42))
        .unwrap();
    assert_eq!(s.weight, Vector3::new(0.2, 0.5, 0.9));
    assert_eq!(s.event, ScatterType::SpecularReflection);
    for value in [-1.0, 1.1, f64::NAN, f64::INFINITY] {
        let bad = MaterialConfig::Mirror {
            reflectance: [value, 0.5, 0.5],
        };
        assert!(bad.validate().is_err());
        assert!(material::from_config(&bad).is_err());
    }
    for json in [
        r#"{"type":"mirror"}"#,
        r#"{"type":"mirror","reflectance":[1,1,1],"roughness":0.3}"#,
    ] {
        assert!(serde_json::from_str::<MaterialConfig>(json).is_err());
    }
}

#[test]
fn mirror_prism_does_not_generate_the_ideal_glass_spectral_caustic() {
    let mut config = SceneConfig {
        environment: None,
        ..Default::default()
    };
    for object in &mut config.objects {
        if let ObjectConfig::Prism {
            material, caustics, ..
        } = object
        {
            *caustics = false;
            *material = MaterialConfig::Mirror {
                reflectance: [0.9; 3],
            };
        }
    }
    let scene = RenderJob::from_config(&config).unwrap();
    let settings = RenderSetting {
        width: 32,
        height: 32,
        samples_per_pixel_axis: 2,
        ..Default::default()
    };
    let lighting = integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.unwrap(),
        &settings,
    )
    .unwrap();
    assert!(
        lighting
            .caustics
            .pixels()
            .iter()
            .all(|p| *p == Vector3::zeros())
    );
    assert!(
        lighting
            .direct
            .as_ref()
            .unwrap()
            .pixels()
            .iter()
            .any(|p| p.norm() > 0.0)
    );
}

#[test]
fn ggx_json_roundtrips_full_config_and_rejects_invalid_parameters() {
    use spectral_caustics::config::Config;
    for roughness in [0.0, 0.001, 0.4, 1.0] {
        let mut config = Config::default();
        let expected = MaterialConfig::GgxMetal {
            eta: [0.2, 0.8, 1.5],
            k: [3.0, 2.0, 1.0],
            roughness,
        };
        if let ObjectConfig::Prism {
            material, caustics, ..
        } = &mut config.scene.objects[0]
        {
            *caustics = false;
            *material = expected.clone();
        }
        let restored = Config::from_json(&config.to_json().unwrap()).unwrap();
        assert_eq!(restored, config);
        let material = material::from_config(&expected).unwrap();
        assert_eq!(material.has_delta(), roughness <= 0.001);
        assert!(material.ior().is_none());
    }
    for field in 0..3 {
        for value in [-1.0, f64::NAN, f64::INFINITY] {
            let mut eta = [1.0; 3];
            let mut k = [2.0; 3];
            if field == 0 {
                eta[1] = value;
            } else if field == 1 {
                k[2] = value;
            }
            let bad = MaterialConfig::GgxMetal {
                eta,
                k,
                roughness: if field == 2 { value } else { 0.5 },
            };
            assert!(bad.validate().is_err());
            assert!(material::from_config(&bad).is_err());
        }
    }
    for json in [
        r#"{"type":"ggx_metal","eta":[1,1,1],"roughness":0.4}"#,
        r#"{"type":"ggx_metal","eta":[1,1,1],"k":[2,2,2],"roughness":0.4,"ior":1.5}"#,
    ] {
        assert!(serde_json::from_str::<MaterialConfig>(json).is_err());
    }
}

#[test]
fn mirror_and_ggx_components_repeat_bitwise_across_threads_mis_depths_and_seeds() {
    use spectral_caustics::config::scene::EnvironmentConfig;
    use spectral_caustics::config::scene::EnvironmentMisHeuristic;

    let temp = crate::support::TempDir::new("material-transport");
    let hdr = temp.write_rgbe_4x2_hdr("env.hdr", false, true);
    let cases = [
        (
            MaterialConfig::Mirror {
                reflectance: [0.9, 0.5, 0.2],
            },
            EnvironmentMisHeuristic::Balance,
            0,
        ),
        (
            MaterialConfig::GgxMetal {
                eta: [0.2, 0.8, 1.5],
                k: [3.0, 2.0, 1.0],
                roughness: 0.55,
            },
            EnvironmentMisHeuristic::Power,
            4,
        ),
        (
            MaterialConfig::GgxMetal {
                eta: [0.2, 0.8, 1.5],
                k: [3.0, 2.0, 1.0],
                roughness: 0.0,
            },
            EnvironmentMisHeuristic::Balance,
            1,
        ),
    ];
    for (material, heuristic, depth) in cases {
        let mut config = SceneConfig {
            environment: Some(EnvironmentConfig {
                map_path: Some(hdr.clone()),
                ..Default::default()
            }),
            environment_samples: 3,
            ..Default::default()
        };
        for object in &mut config.objects {
            if let ObjectConfig::Prism {
                material: m,
                caustics,
                ..
            } = object
            {
                *caustics = matches!(material, MaterialConfig::Dielectric { .. });
                *m = material.clone();
            }
        }
        let mut scene = RenderJob::from_config(&config).unwrap();
        assert!(scene.photon.caustics_object_id.is_none());
        scene.camera_options.environment_mis_heuristic = heuristic;
        scene.camera_options.max_diffuse_bounces = depth;
        let mut reference = None;
        for threads in [1, 2, 4] {
            let images = crate::support::run_in_threads(threads, || {
                spectral_caustics::integrator::camera::integrate_rgb(
                    &scene,
                    &spectral_caustics::config::render::RenderSetting {
                        width: 10,
                        height: 8,
                        camera_spp: 4,
                        seed: 42,
                        ..Default::default()
                    },
                    None,
                )
                .unwrap()
            });
            for i in 0..images.combined.pixels().len() {
                assert!(
                    (images.direct.pixels()[i] + images.caustics.pixels()[i]
                        - images.combined.pixels()[i])
                        .norm()
                        < 1e-12
                );
                assert_eq!(images.caustics.pixels()[i], Vector3::zeros());
            }
            if let Some(expected) = &reference {
                assert_camera_images_bits_eq(&images, expected);
            } else {
                reference = Some(images);
            }
        }
    }
}

#[test]
fn ggx_ground_is_supported_by_rgb_camera_and_rejected_by_irradiance_binding() {
    let mut config = SceneConfig::default();
    for object in &mut config.objects {
        if let ObjectConfig::Ground { material, .. } = object {
            *material = MaterialConfig::GgxMetal {
                eta: [0.2; 3],
                k: [3.0; 3],
                roughness: 0.5,
            };
        }
    }
    let scene = RenderJob::from_config(&config).unwrap();
    let lighting = integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.unwrap(),
        &RenderSetting {
            width: 16,
            height: 12,
            samples_per_pixel_axis: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        spectral_caustics::integrator::camera::integrate_rgb(
            &scene,
            &spectral_caustics::config::render::RenderSetting {
                width: 16,
                height: 12,
                camera_spp: 4,
                seed: 42,
                ..Default::default()
            },
            Some(&lighting)
        )
        .is_err()
    );
    assert!(
        spectral_caustics::integrator::camera::integrate_rgb(
            &scene,
            &spectral_caustics::config::render::RenderSetting {
                width: 16,
                height: 12,
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

#[test]
fn caustics_selection_requires_dielectric_and_defaults_off_for_all_supported_shapes() {
    use spectral_caustics::config::Config;
    for source in [
        include_str!("../../configs/sphere_spectral.json"),
        include_str!("../../configs/vertical_prism_spectral_caustics.json"),
    ] {
        let mut value: serde_json::Value = serde_json::from_str(source).unwrap();
        value["render"]["camera_mode"] = serde_json::json!("rgb");
        value["scene"]["objects"][0]["material"] =
            serde_json::json!({"type":"mirror","reflectance":[1.0,1.0,1.0]});
        value["scene"]["objects"][0]["caustics"] = true.into();
        let invalid = Config::from_json(&value.to_string()).unwrap();
        assert!(invalid.validate().is_err());
        assert!(invalid.build().is_err());
        value["scene"]["objects"][0]
            .as_object_mut()
            .unwrap()
            .remove("caustics");
        let valid = Config::from_json(&value.to_string()).unwrap();
        valid.validate().unwrap();
        assert!(valid.build().unwrap().photon.caustics_object_id.is_none());
    }
}

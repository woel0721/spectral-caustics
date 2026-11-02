use nalgebra::Vector3;
use rand::{SeedableRng, rngs::StdRng};
use spectral_caustics::{
    config::rendersetting::RenderSetting,
    config::{
        materialconfig::MaterialConfig,
        sceneconfig::{ObjectConfig, SceneConfig},
    },
    material::{self, ScatterType},
    render::caustics::render_spectral_lighting,
    scene::Scene,
};

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
        if let ObjectConfig::Prism { material, .. } = object {
            *material = MaterialConfig::Mirror {
                reflectance: [0.9; 3],
            };
        }
    }
    let scene = Scene::from_config(&config).unwrap();
    let settings = RenderSetting {
        width: 32,
        height: 32,
        samples_per_pixel_axis: 2,
        ..Default::default()
    };
    let lighting =
        render_spectral_lighting(&scene, scene.prism_id, scene.ground_id.unwrap(), &settings)
            .unwrap();
    assert!(
        lighting
            .caustics
            .pixels
            .iter()
            .all(|p| *p == Vector3::zeros())
    );
    assert!(lighting.direct.pixels.iter().any(|p| p.norm() > 0.0));
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
        if let ObjectConfig::Prism { material, .. } = &mut config.scene.objects[0] {
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
    use spectral_caustics::{
        config::sceneconfig::{EnvironmentConfig, EnvironmentMisHeuristic},
        render::render_camera_glass_components,
    };
    let settings = RenderSetting {
        width: 16,
        height: 12,
        samples_per_pixel_axis: 1,
        ..Default::default()
    };
    for material in [
        MaterialConfig::Mirror {
            reflectance: [0.9, 0.5, 0.2],
        },
        MaterialConfig::GgxMetal {
            eta: [0.2, 0.8, 1.5],
            k: [3.0, 2.0, 1.0],
            roughness: 0.55,
        },
        MaterialConfig::GgxMetal {
            eta: [0.2, 0.8, 1.5],
            k: [3.0, 2.0, 1.0],
            roughness: 0.0,
        },
    ] {
        let mut config = SceneConfig {
            environment: Some(EnvironmentConfig {
                map_path: Some("assets/HDRI/autoshop_01_4k.hdr".into()),
                ..Default::default()
            }),
            environment_samples: 3,
            ..Default::default()
        };
        for object in &mut config.objects {
            if let ObjectConfig::Prism { material: m, .. } = object {
                *m = material.clone();
            }
        }
        let mut scene = Scene::from_config(&config).unwrap();
        let lighting =
            render_spectral_lighting(&scene, scene.prism_id, scene.ground_id.unwrap(), &settings)
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
                for seed in [42, 123] {
                    let mut reference = None;
                    for threads in [1, 2, 4, 1] {
                        let images = rayon::ThreadPoolBuilder::new()
                            .num_threads(threads)
                            .build()
                            .unwrap()
                            .install(|| {
                                render_camera_glass_components(&scene, &lighting, 16, 12, 8, seed)
                                    .unwrap()
                            });
                        for i in 0..images.combined.pixels.len() {
                            assert!(
                                (images.direct.pixels[i] + images.caustics.pixels[i]
                                    - images.combined.pixels[i])
                                    .norm()
                                    < 1e-12
                            );
                            assert_eq!(images.caustics.pixels[i], Vector3::zeros());
                        }
                        let bits: Vec<_> = [&images.direct, &images.caustics, &images.combined]
                            .into_iter()
                            .flat_map(|b| {
                                b.pixels
                                    .iter()
                                    .map(|v| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
                            })
                            .collect();
                        if let Some(expected) = &reference {
                            assert_eq!(&bits, expected);
                        } else {
                            reference = Some(bits);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn reflective_prisms_ignore_preexisting_glass_caustic_maps_but_keep_direct() {
    use spectral_caustics::render::render_camera_glass_components;
    let settings = RenderSetting {
        width: 32,
        height: 24,
        samples_per_pixel_axis: 2,
        ..Default::default()
    };
    let mut config = SceneConfig::default();
    let glass = Scene::from_config(&config).unwrap();
    let lighting =
        render_spectral_lighting(&glass, glass.prism_id, glass.ground_id.unwrap(), &settings)
            .unwrap();
    assert!(lighting.caustics.pixels.iter().any(|p| p.norm() > 0.0));
    for material in [
        MaterialConfig::Mirror {
            reflectance: [0.9; 3],
        },
        MaterialConfig::GgxMetal {
            eta: [1.5; 3],
            k: [2.0; 3],
            roughness: 0.5,
        },
    ] {
        for object in &mut config.objects {
            if let ObjectConfig::Prism { material: m, .. } = object {
                *m = material.clone();
            }
        }
        let scene = Scene::from_config(&config).unwrap();
        let images = render_camera_glass_components(&scene, &lighting, 32, 24, 16, 42).unwrap();
        assert!(
            images
                .caustics
                .pixels
                .iter()
                .all(|p| *p == Vector3::zeros())
        );
        assert!(images.direct.pixels.iter().any(|p| p.norm() > 0.0));
    }
}

#[test]
fn ggx_ground_is_supported_by_rgb_camera_and_rejected_by_irradiance_binding() {
    use spectral_caustics::render::{render_camera_glass, render_camera_glass_components};
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
    let scene = Scene::from_config(&config).unwrap();
    let lighting = render_spectral_lighting(
        &scene,
        scene.prism_id,
        scene.ground_id.unwrap(),
        &RenderSetting {
            width: 16,
            height: 12,
            samples_per_pixel_axis: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(render_camera_glass_components(&scene, &lighting, 16, 12, 4, 42).is_err());
    assert!(
        render_camera_glass(&scene, 16, 12, 4, 42)
            .unwrap()
            .pixels
            .iter()
            .any(|p| p.norm() > 0.0)
    );
}

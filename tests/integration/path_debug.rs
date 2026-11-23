use crate::support::{TempDir, assert_camera_images_bits_eq, assert_framebuffer_bits_eq};
use spectral_caustics::config::Config;
use spectral_caustics::config::material::MaterialConfig;
use spectral_caustics::config::scene::EnvironmentConfig;
use spectral_caustics::config::scene::EnvironmentMisHeuristic;
use spectral_caustics::config::scene::ObjectConfig;
use spectral_caustics::diagnostics::StopReason;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct TemporaryDirectory(TempDir);
impl TemporaryDirectory {
    fn new() -> Self {
        Self(TempDir::new("spectral-path-debug"))
    }
    fn hdr(&self) -> PathBuf {
        self.0.write_rgbe_4x2_hdr("test.hdr", false, false)
    }
}
impl std::ops::Deref for TemporaryDirectory {
    type Target = PathBuf;
    fn deref(&self) -> &Self::Target {
        &self.0.0
    }
}

fn config() -> Config {
    let mut config = Config::default();
    config.render.width = 24;
    config.render.height = 18;
    config.render.camera_spp = 12;
    config.render.samples_per_pixel_axis = 1;
    config.render.thread_count = Some(1);
    config.render.diagnostics.target = Some(0);
    config.scene.environment_samples = 3;
    config.scene.max_diffuse_bounces = 4;
    config
}

#[test]
fn debug_path_defaults_false_roundtrips_and_rejects_non_bool_values() {
    let config = config();
    assert!(!config.render.diagnostics.path);
    let mut json: serde_json::Value = serde_json::from_str(&config.to_json().unwrap()).unwrap();
    json["render"]["diagnostics"]
        .as_object_mut()
        .unwrap()
        .remove("path");
    assert!(
        !Config::from_json(&json.to_string())
            .unwrap()
            .render
            .diagnostics
            .path
    );
    json["render"]["diagnostics"]["path"] = true.into();
    let loaded = Config::from_json(&json.to_string()).unwrap();
    assert!(loaded.render.diagnostics.path);
    assert_eq!(
        Config::from_json(&loaded.to_json().unwrap()).unwrap(),
        loaded
    );
    for value in [
        serde_json::json!(null),
        serde_json::json!(1),
        serde_json::json!("true"),
    ] {
        json["render"]["diagnostics"]["path"] = value;
        assert!(
            Config::from_json(&json.to_string())
                .and_then(|config| {
                    config.validate()?;
                    Ok(config)
                })
                .is_err()
        );
    }
}

#[test]
fn debug_observes_same_rgb_xyz_paths_and_repeats_across_threads_materials_and_environments() {
    let directory = TemporaryDirectory::new();
    let hdr = directory.hdr();
    let cases = [
        (
            MaterialConfig::Dielectric {
                ior: 1.52,
                dispersion: None,
            },
            None,
            EnvironmentMisHeuristic::Balance,
        ),
        (
            MaterialConfig::Mirror {
                reflectance: [0.95; 3],
            },
            Some(hdr.clone()),
            EnvironmentMisHeuristic::Power,
        ),
        (
            MaterialConfig::GgxMetal {
                eta: [0.2; 3],
                k: [3.0; 3],
                roughness: 0.4,
            },
            None,
            EnvironmentMisHeuristic::Power,
        ),
        (
            MaterialConfig::GgxMetal {
                eta: [0.2; 3],
                k: [3.0; 3],
                roughness: 0.0,
            },
            Some(hdr),
            EnvironmentMisHeuristic::Balance,
        ),
    ];
    for (material, map, heuristic) in cases {
        let mut config = config();
        config.render.width = 12;
        config.render.height = 10;
        config.render.camera_spp = 8;
        config.scene.environment = Some(EnvironmentConfig {
            map_path: map,
            ..Default::default()
        });
        config.scene.environment_mis_heuristic = heuristic;
        if let ObjectConfig::Prism {
            material: m,
            caustics,
            ..
        } = &mut config.scene.objects[0]
        {
            *caustics = matches!(material, MaterialConfig::Dielectric { .. });
            *m = material.clone();
        }
        let scene = config.build().unwrap();
        config.render.seed = 42;
        let lighting = spectral_caustics::integrator::photon::integrate_xyz(
            &scene,
            scene.photon.caustics_object_id,
            scene.photon.ground_id.expect("receiver"),
            &config.render,
        )
        .unwrap();
        let ordinary = spectral_caustics::integrator::camera::integrate_rgb(
            &scene,
            &config.render,
            Some(&lighting),
        )
        .unwrap();
        assert!(ordinary.path_debug.is_none());
        let check_photons = matches!(material, MaterialConfig::Dielectric { .. });
        let mut reference = None;
        for threads in [1, 2, 4] {
            config.render.thread_count = Some(threads);
            config.render.seed = 42;
            config.render.diagnostics.path = true;
            let (photons, images) = crate::support::run_in_threads(threads, || {
                let photons = check_photons.then(|| {
                    spectral_caustics::integrator::photon::integrate_xyz(
                        &scene,
                        scene.photon.caustics_object_id,
                        scene.photon.ground_id.expect("receiver"),
                        &config.render,
                    )
                    .unwrap()
                });
                let active = photons.as_ref().unwrap_or(&lighting);
                let images = spectral_caustics::integrator::camera::integrate_rgb(
                    &scene,
                    &config.render,
                    Some(active),
                )
                .unwrap();
                (photons, images)
            });
            if let Some(photons) = photons {
                assert_framebuffer_bits_eq(
                    photons.direct.as_ref().unwrap(),
                    lighting.direct.as_ref().unwrap(),
                );
                assert_framebuffer_bits_eq(&photons.caustics, &lighting.caustics);
            }
            assert_camera_images_bits_eq(&images, &ordinary);
            let stats = images.path_debug.as_ref().unwrap();
            if let Some(expected) = &reference {
                assert_eq!(stats, expected);
            }
            assert!(
                stats
                    .pixels
                    .iter()
                    .all(|p| p.samples == config.render.camera_spp as u64)
            );
            assert!(
                stats
                    .pixels
                    .iter()
                    .all(|p| p.terminations.values().sum::<u64>() == p.target_samples)
            );
            assert!(
                stats
                    .pixels
                    .iter()
                    .any(|p| p.target_samples > 0 && p.target_samples < p.samples)
            );
            assert!(
                stats
                    .pixels
                    .iter()
                    .any(|p| p.terminations.contains_key(&StopReason::EnvironmentEscape))
            );
            let sum = |f: fn(&spectral_caustics::diagnostics::PathDebugPixel) -> u64| {
                stats.pixels.iter().map(f).sum::<u64>()
            };
            match &material {
                MaterialConfig::Mirror { .. } => {
                    assert!(sum(|p| p.events.specular_reflection) > 0);
                    assert_eq!(sum(|p| p.total_internal_reflection), 0);
                }
                MaterialConfig::Dielectric { .. } => {
                    assert!(sum(|p| p.events.specular_transmission) > 0)
                }
                MaterialConfig::GgxMetal { roughness, .. } if *roughness > 0.0 => {
                    assert!(sum(|p| p.events.glossy_reflection) > 0);
                    assert_eq!(sum(|p| p.total_internal_reflection), 0);
                }
                _ => assert!(sum(|p| p.events.specular_reflection) > 0),
            }
            reference = images.path_debug;
        }
    }
}

#[test]
fn command_line_gates_aovs_by_feature_and_protects_physical_outputs() {
    let directory = TemporaryDirectory::new();
    let mut config = config();
    config.render.exposure = 0.0;
    // Test the material with geometry behind it, rather than an opaque prefix only.
    config.render.output_path = directory.0.join("reflection_count.png");
    let path = directory.0.join("scene.json");
    let run = |config: &Config| {
        config.save(&path).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_spectral-caustics"))
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&config);
    let names = [
        "path_target.png",
        "aux_reflection_count.png",
        "transmission_count.png",
        "first_hit_normal.png",
        "path_debug.json",
    ];
    for name in names {
        assert!(!directory.0.join(name).exists());
    }
    let ordinary_names = [
        "reflection_count.png",
        "camera_direct.png",
        "camera_caustics.png",
    ];
    let ordinary: Vec<_> = ordinary_names
        .iter()
        .map(|name| fs::read(directory.0.join(name)).unwrap())
        .collect();
    config.render.diagnostics.path = true;
    run(&config);
    for (name, expected) in ordinary_names.iter().zip(ordinary) {
        assert_eq!(fs::read(directory.0.join(name)).unwrap(), expected);
        assert!(
            image::open(directory.0.join(name))
                .unwrap()
                .into_rgb8()
                .pixels()
                .all(|p| p.0 == [0; 3])
        );
    }
    let aovs: Vec<_> = names
        .iter()
        .map(|name| {
            let file = directory.0.join(name);
            if name.ends_with(".png") {
                let image = image::open(&file).unwrap();
                assert_eq!((image.width(), image.height()), (24, 18));
                assert_eq!(image.color(), image::ColorType::Rgb8);
                assert!(image.into_rgb8().pixels().any(|p| p.0 != [0; 3]));
            }
            fs::read(file).unwrap()
        })
        .collect();
    let json: serde_json::Value = serde_json::from_slice(aovs.last().unwrap()).unwrap();
    assert_eq!(json["pixels"].as_array().unwrap().len(), 24 * 18);
    assert!(
        json["pixels"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["samples"] == 12)
    );
    // Ordinary rendering changes with exposure; event records and AOV encoding do not.
    config.render.exposure = 2.0;
    run(&config);
    for (name, expected) in names.iter().zip(aovs) {
        assert_eq!(fs::read(directory.0.join(name)).unwrap(), expected);
    }
    assert!(
        image::open(&config.render.output_path)
            .unwrap()
            .into_rgb8()
            .pixels()
            .any(|p| p.0 != [0; 3])
    );
    assert!(
        json["pixels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["terminations"].get("environment_escape").is_some())
    );
}

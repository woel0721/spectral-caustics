#![cfg(feature = "diagnostics")]
mod support;
use spectral_caustics::config::Config;
use spectral_caustics::config::material::MaterialConfig;
use spectral_caustics::config::scene::EnvironmentConfig;
use spectral_caustics::config::scene::EnvironmentMisHeuristic;
use spectral_caustics::config::scene::ObjectConfig;
#[cfg(feature = "diagnostics")]
use spectral_caustics::diagnostics::StopReason;
use spectral_caustics::renderer::Renderer;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use support::{assert_camera_images_bits_eq, assert_framebuffer_bits_eq};

struct TemporaryDirectory(PathBuf);
impl TemporaryDirectory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "spectral-path-debug-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn hdr(&self) -> PathBuf {
        let path = self.0.join("test.hdr");
        let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n".to_vec();
        for _ in 0..2 {
            for pixel in [
                [128, 64, 32, 130],
                [64, 128, 32, 131],
                [32, 64, 128, 132],
                [128, 32, 64, 133],
            ] {
                bytes.extend_from_slice(&pixel);
            }
        }
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
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

#[cfg(feature = "diagnostics")]
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

#[cfg(feature = "diagnostics")]
#[test]
fn debug_observes_same_rgb_xyz_paths_and_repeats_across_threads_materials_and_environments() {
    let directory = TemporaryDirectory::new();
    let hdr = directory.hdr();
    for material in [
        MaterialConfig::Dielectric {
            ior: 1.52,
            dispersion: None,
        },
        MaterialConfig::Mirror {
            reflectance: [0.95; 3],
        },
        MaterialConfig::GgxMetal {
            eta: [0.2; 3],
            k: [3.0; 3],
            roughness: 0.4,
        },
        MaterialConfig::GgxMetal {
            eta: [0.2; 3],
            k: [3.0; 3],
            roughness: 0.0,
        },
    ] {
        for map in [None, Some(hdr.clone())] {
            for heuristic in [
                EnvironmentMisHeuristic::Balance,
                EnvironmentMisHeuristic::Power,
            ] {
                let mut config = config();
                config.scene.environment = Some(EnvironmentConfig {
                    map_path: map.clone(),
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
                let renderer = Renderer::new(config.render.clone()).unwrap();
                let lighting = spectral_caustics::integrator::photon::integrate_xyz(
                    &scene,
                    scene.photon.caustics_object_id,
                    scene.photon.ground_id.expect("receiver"),
                    (renderer).settings(),
                )
                .unwrap();
                let ordinary = spectral_caustics::integrator::camera::integrate_rgb(
                    &scene,
                    &spectral_caustics::config::render::RenderSetting {
                        width: (renderer).settings().width,
                        height: (renderer).settings().height,
                        camera_spp: config.render.camera_spp,
                        seed: 42,
                        ..Default::default()
                    },
                    Some(&lighting),
                )
                .unwrap();
                assert!(ordinary.path_debug.is_none());
                let mut reference = None;
                for threads in [1, 2, 4, 1] {
                    config.render.thread_count = Some(threads);
                    #[cfg(feature = "diagnostics")]
                    {
                        config.render.diagnostics.path = true;
                    }
                    let renderer = Renderer::new(config.render.clone()).unwrap();
                    let photons = spectral_caustics::integrator::photon::integrate_xyz(
                        &scene,
                        scene.photon.caustics_object_id,
                        scene.photon.ground_id.expect("receiver"),
                        (renderer).settings(),
                    )
                    .unwrap();
                    assert_framebuffer_bits_eq(
                        photons.direct.as_ref().unwrap(),
                        lighting.direct.as_ref().unwrap(),
                    );
                    assert_framebuffer_bits_eq(&photons.caustics, &lighting.caustics);
                    let images = spectral_caustics::integrator::camera::integrate_rgb(
                        &scene,
                        &spectral_caustics::config::render::RenderSetting {
                            width: (renderer).settings().width,
                            height: (renderer).settings().height,
                            camera_spp: config.render.camera_spp,
                            seed: 42,
                            ..config.render.clone()
                        },
                        Some(&photons),
                    )
                    .unwrap();
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
                        stats.pixels.iter().any(|p| {
                            p.terminations.contains_key(&StopReason::EnvironmentEscape)
                        })
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
    }
}

#[cfg(feature = "diagnostics")]
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
    #[cfg(feature = "diagnostics")]
    {
        config.render.diagnostics.path = true;
    }
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

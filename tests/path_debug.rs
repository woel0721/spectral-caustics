use spectral_caustics::{
    config::{
        Config,
        materialconfig::MaterialConfig,
        sceneconfig::{EnvironmentConfig, EnvironmentMisHeuristic, ObjectConfig},
    },
    render::{
        CameraLightingImages, Render, framebuffer::Framebuffer, integrator::debug::PathTermination,
    },
    scene::Scene,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

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
    config.scene.environment_samples = 3;
    config.scene.max_diffuse_bounces = 4;
    config
}

fn identical(a: &Framebuffer, b: &Framebuffer) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    assert_eq!(a.pixels.len(), b.pixels.len());
    assert!(a.pixels.iter().zip(&b.pixels).all(|(a, b)| {
        a.iter()
            .zip(b.iter())
            .all(|(a, b)| a.to_bits() == b.to_bits())
    }));
}
fn identical_images(a: &CameraLightingImages, b: &CameraLightingImages) {
    identical(&a.direct, &b.direct);
    identical(&a.caustics, &b.caustics);
    identical(&a.combined, &b.combined);
}

#[test]
fn debug_path_defaults_false_roundtrips_and_rejects_non_bool_values() {
    let config = config();
    assert!(!config.render.debug_path);
    let mut json: serde_json::Value = serde_json::from_str(&config.to_json().unwrap()).unwrap();
    json["render"].as_object_mut().unwrap().remove("debug_path");
    assert!(
        !Config::from_json(&json.to_string())
            .unwrap()
            .render
            .debug_path
    );
    json["render"]["debug_path"] = true.into();
    let loaded = Config::from_json(&json.to_string()).unwrap();
    assert!(loaded.render.debug_path);
    assert_eq!(
        Config::from_json(&loaded.to_json().unwrap()).unwrap(),
        loaded
    );
    for value in [
        serde_json::json!(null),
        serde_json::json!(1),
        serde_json::json!("true"),
    ] {
        json["render"]["debug_path"] = value;
        assert!(Config::from_json(&json.to_string()).is_err());
    }
}

#[test]
fn debug_observes_same_rgb_xyz_paths_and_repeats_across_threads_materials_and_environments() {
    let directory = TemporaryDirectory::new();
    let hdr = directory.hdr();
    for material in [
        MaterialConfig::Dielectric { ior: 1.52 },
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
                if let ObjectConfig::Prism { material: m, .. } = &mut config.scene.objects[0] {
                    *m = material.clone();
                }
                let scene = Scene::from_config(&config.scene).unwrap();
                let renderer = Render::new(config.render.clone()).unwrap();
                let lighting = renderer.render_lighting(&scene).unwrap();
                let ordinary = renderer
                    .render_camera_glass_components(&scene, &lighting, config.render.camera_spp, 42)
                    .unwrap();
                assert!(ordinary.path_debug.is_none());
                let mut reference = None;
                for threads in [1, 2, 4, 1] {
                    config.render.thread_count = Some(threads);
                    config.render.debug_path = true;
                    let renderer = Render::new(config.render.clone()).unwrap();
                    let photons = renderer.render_lighting(&scene).unwrap();
                    identical(&photons.direct, &lighting.direct);
                    identical(&photons.caustics, &lighting.caustics);
                    let images = renderer
                        .render_camera_glass_components(
                            &scene,
                            &photons,
                            config.render.camera_spp,
                            42,
                        )
                        .unwrap();
                    identical_images(&images, &ordinary);
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
                            .all(|p| p.terminations.values().sum::<u64>() == p.prism_samples)
                    );
                    assert!(
                        stats
                            .pixels
                            .iter()
                            .any(|p| p.prism_samples > 0 && p.prism_samples < p.samples)
                    );
                    assert!(stats.pixels.iter().any(|p| {
                        p.terminations
                            .contains_key(&PathTermination::EnvironmentEscape)
                    }));
                    let sum = |f: fn(
                        &spectral_caustics::render::integrator::debug::PathDebugPixel,
                    ) -> u64| stats.pixels.iter().map(f).sum::<u64>();
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

#[test]
fn command_line_writes_aovs_without_exposure_and_protects_primary_filename() {
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
        "camera_combined.png",
        "constant_ior.png",
    ];
    let ordinary: Vec<_> = ordinary_names
        .iter()
        .map(|name| fs::read(directory.0.join(name)).unwrap())
        .collect();
    config.render.debug_path = true;
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
        image::open(directory.0.join("camera_combined.png"))
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

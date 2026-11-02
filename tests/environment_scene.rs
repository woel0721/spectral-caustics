use nalgebra::{Vector2, Vector3};
use rand::{Rng, SeedableRng, rngs::StdRng};
use rayon::prelude::*;
use spectral_caustics::{
    config::{
        cameraconfig::CameraConfig,
        materialconfig::MaterialConfig,
        rendersetting::RenderSetting,
        sceneconfig::{EnvironmentConfig, EnvironmentMisHeuristic, ObjectConfig, SceneConfig},
    },
    environment::{Environment, EnvironmentMap},
    material::ScatterType,
    render::{
        Render, SpectralLighting, framebuffer::Framebuffer, render_camera_glass,
        render_camera_glass_components, utils::linear_srgb_to_xyz,
    },
    scene::Scene,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

struct TemporaryHdr(PathBuf);

impl TemporaryHdr {
    fn new(uniform: bool) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "environment-scene-{}-{stamp}-{}.hdr",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // 手写 4x2 RGBE，宽度小于 8 使用非 RLE 像素；没有外部资源或编码器依赖。
        // 顶行四列分别为 [2,1,0.5]、[2,4,1]、[2,4,8]、[16,4,8]，底行翻倍。
        let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n".to_vec();
        for row in 0..2 {
            for mut pixel in [
                [128, 64, 32, 130],
                [64, 128, 32, 131],
                [32, 64, 128, 132],
                [128, 32, 64, 133],
            ] {
                if uniform {
                    pixel = [128, 64, 32, 132]; // [8,4,2]，全方向恒定。
                } else {
                    pixel[3] += row;
                }
                bytes.extend_from_slice(&pixel);
            }
        }
        fs::write(&path, bytes).unwrap();
        Self(path)
    }

    fn config(&self) -> EnvironmentConfig {
        EnvironmentConfig {
            map_path: Some(self.0.clone()),
            ..Default::default()
        }
    }
}

impl Drop for TemporaryHdr {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn scene_config(environment: EnvironmentConfig) -> SceneConfig {
    SceneConfig {
        environment: Some(environment),
        ground_checkerboard: None,
        camera: CameraConfig {
            position: [0.0, 0.0, 3.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 10.0,
        },
        objects: vec![ObjectConfig::Ground {
            height_m: 1.0,
            material: MaterialConfig::Lambertian { albedo: [0.8; 3] },
        }],
        ..Default::default()
    }
}

fn assert_rgb(actual: Vector3<f64>, expected: Vector3<f64>) {
    assert!(
        (actual - expected).norm() < 1e-11,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn gradient_without_map_matches_the_original_world_z_formula() {
    let config = EnvironmentConfig {
        horizon_linear_rgb: [1.0, 2.0, 3.0],
        zenith_linear_rgb: [3.0, 4.0, 5.0],
        ..Default::default()
    };
    for rotation_degrees in [0.0, 90.0, -450.0] {
        let environment = Environment::try_from(&EnvironmentConfig {
            rotation_degrees,
            ..config.clone()
        })
        .unwrap();
        for direction in [
            Vector3::x(),
            Vector3::y(),
            Vector3::z(),
            -Vector3::z(),
            Vector3::new(3.0_f64.sqrt(), 0.0, 1.0),
            Vector3::new(-3.0, 4.0, 5.0),
        ] {
            let t = (direction / direction.norm()).z.clamp(0.0, 1.0);
            let original = Vector3::from(config.horizon_linear_rgb) * (1.0 - t)
                + Vector3::from(config.zenith_linear_rgb) * t;
            assert_eq!(environment.radiance(direction), Some(original));
        }
    }
}

#[test]
fn map_takes_precedence_and_zero_rotation_matches_environment_map() {
    let hdr = TemporaryHdr::new(false);
    let map = EnvironmentMap::load(&hdr.0).unwrap();
    let environment = Environment::try_from(&EnvironmentConfig {
        horizon_linear_rgb: [100.0; 3],
        zenith_linear_rgb: [200.0; 3],
        ..hdr.config()
    })
    .unwrap();
    for direction in [
        Vector3::x(),
        -Vector3::x(),
        Vector3::y(),
        -Vector3::y(),
        Vector3::z(),
        -Vector3::z(),
        Vector3::new(1.0, 2.0, 3.0),
    ] {
        assert_rgb(
            environment.radiance(direction).unwrap(),
            map.radiance(direction).unwrap(),
        );
    }
    assert_rgb(
        environment.radiance(Vector3::x()).unwrap(),
        Vector3::new(3.0, 6.0, 6.75),
    );
}

#[test]
fn intensity_scales_both_sources_without_clipping_hdr() {
    let hdr = TemporaryHdr::new(false);
    for map_path in [None, Some(hdr.0.clone())] {
        let config = EnvironmentConfig {
            map_path,
            ..Default::default()
        };
        let original = Environment::try_from(&config)
            .unwrap()
            .radiance(Vector3::x())
            .unwrap();
        for intensity in [0.0, 0.25, 2.0] {
            let environment = Environment::try_from(&EnvironmentConfig {
                intensity,
                ..config.clone()
            })
            .unwrap();
            assert_rgb(
                environment.radiance(Vector3::x()).unwrap(),
                original * intensity,
            );
        }
    }
}

#[test]
fn positive_ninety_degree_rotation_uses_the_inverse_world_z_transform() {
    let hdr = TemporaryHdr::new(false);
    let map = EnvironmentMap::load(&hdr.0).unwrap();
    for rotation_degrees in [90.0, 450.0, -270.0] {
        let environment = Environment::try_from(&EnvironmentConfig {
            rotation_degrees,
            ..hdr.config()
        })
        .unwrap();
        for (world, local) in [
            (Vector3::x(), -Vector3::y()),
            (Vector3::y(), Vector3::x()),
            (-Vector3::x(), Vector3::y()),
            (-Vector3::y(), -Vector3::x()),
            (Vector3::z(), Vector3::z()),
            (-Vector3::z(), -Vector3::z()),
        ] {
            assert_rgb(
                environment.radiance(world).unwrap(),
                map.radiance(local).unwrap(),
            );
        }
        // 世界 +X 查贴图 -Y，独立核对数值，防止旋转符号或 UV 约定改变。
        assert_rgb(
            environment.radiance(Vector3::x()).unwrap(),
            Vector3::new(3.0, 3.75, 1.125),
        );
    }
}

#[test]
fn rotation_handles_extreme_direction_scales_and_rejects_invalid_directions() {
    let hdr = TemporaryHdr::new(false);
    let environment = Environment::try_from(&EnvironmentConfig {
        rotation_degrees: 45.0,
        ..hdr.config()
    })
    .unwrap();
    let expected = environment.radiance(Vector3::repeat(1.0)).unwrap();
    for scale in [f64::MAX, 1e-300, f64::from_bits(1)] {
        assert_rgb(
            environment.radiance(Vector3::repeat(scale)).unwrap(),
            expected,
        );
    }
    assert!(environment.radiance(Vector3::zeros()).is_none());
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for axis in 0..3 {
            let mut direction = Vector3::repeat(1.0);
            direction[axis] = value;
            assert!(environment.radiance(direction).is_none());
        }
    }
    assert!(
        Environment::try_from(&EnvironmentConfig {
            rotation_degrees: f64::MAX,
            ..hdr.config()
        })
        .unwrap()
        .radiance(Vector3::x())
        .is_some()
    );
}

#[test]
fn intensity_overflow_and_invalid_runtime_intensity_return_none() {
    let hdr = TemporaryHdr::new(false);
    for map_path in [None, Some(hdr.0.clone())] {
        let mut environment = Environment::try_from(&EnvironmentConfig {
            map_path,
            horizon_linear_rgb: [2.0; 3],
            intensity: f64::MAX,
            ..Default::default()
        })
        .unwrap();
        assert!(environment.radiance(Vector3::x()).is_none());
        for intensity in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            environment.intensity = intensity;
            assert!(!environment.is_valid());
            assert!(environment.radiance(Vector3::x()).is_none());
        }
    }
}

#[test]
fn missing_or_corrupt_hdr_returns_a_scene_construction_error() {
    let hdr = TemporaryHdr::new(false);
    let config = scene_config(hdr.config());
    fs::remove_file(&hdr.0).unwrap();
    let error = Scene::from_config(&config).err().unwrap().to_string();
    assert!(error.contains("scene.environment.map_path"));
    assert!(error.contains(hdr.0.to_str().unwrap()));
    fs::write(&hdr.0, b"not an HDR").unwrap();
    assert!(Scene::from_config(&config).is_err());
}

#[test]
fn scene_queries_cached_hdr_after_its_source_file_is_deleted() {
    let hdr = TemporaryHdr::new(false);
    let scene = Scene::from_config(&scene_config(hdr.config())).unwrap();
    fs::remove_file(&hdr.0).unwrap();
    for _ in 0..3 {
        assert_rgb(
            scene.background_radiance(Vector3::z()).unwrap(),
            Vector3::new(2.0, 4.0, 4.5),
        );
        assert_rgb(
            scene.background_radiance(-Vector3::z()).unwrap(),
            Vector3::new(4.0, 8.0, 9.0),
        );
    }
    assert!(Scene::from_config(&scene_config(hdr.config())).is_err());
}

#[test]
fn camera_misses_return_rotated_scaled_hdr_background() {
    let hdr = TemporaryHdr::new(false);
    let map = EnvironmentMap::load(&hdr.0).unwrap();
    let mut scene = Scene::from_config(&scene_config(EnvironmentConfig {
        rotation_degrees: 90.0,
        intensity: 2.0,
        ..hdr.config()
    }))
    .unwrap();
    scene.objects.clear();
    fs::remove_file(&hdr.0).unwrap();
    // pixel_seed(0, 0, CAMERA_STREAM=0) = 0，独立推导新逐像素流的首条路径。
    let mut rng = StdRng::seed_from_u64(0);
    let offset = Vector2::new(rng.random::<f64>(), rng.random::<f64>());
    let direction = scene
        .camera
        .ray_for_pixel_sample(0, 0, 1, 1, offset)
        .unwrap()
        .direction;
    let local = Vector3::new(direction.y, -direction.x, direction.z);
    let expected = map.radiance(local).unwrap() * 2.0;
    assert_rgb(
        render_camera_glass(&scene, 1, 1, 1, 0).unwrap().pixels[0],
        linear_srgb_to_xyz(expected).unwrap(),
    );
}

#[test]
fn glass_reflection_and_transmission_query_hdr_in_the_final_direction() {
    let hdr = TemporaryHdr::new(false);
    let map = EnvironmentMap::load(&hdr.0).unwrap();
    let mut config = scene_config(hdr.config());
    config.objects = vec![ObjectConfig::Ground {
        height_m: 1.0,
        material: MaterialConfig::Dielectric { ior: 1.5 },
    }];
    let scene = Scene::from_config(&config).unwrap();
    fs::remove_file(&hdr.0).unwrap();
    let mut checked = [false; 2];
    let mut expected = Vector3::zeros();
    let mut rng = StdRng::seed_from_u64(0);
    for _ in 0..256 {
        let offset = Vector2::new(rng.random::<f64>(), rng.random::<f64>());
        let ray = scene
            .camera
            .ray_for_pixel_sample(0, 0, 1, 1, offset)
            .unwrap();
        let hit = scene.intersect(&ray).unwrap();
        let material = scene.material(hit.material_id.unwrap()).unwrap();
        let sample = material
            .sample(-ray.direction, hit.normal, &mut rng)
            .unwrap();
        let index = usize::from(sample.event == ScatterType::SpecularTransmission);
        expected += map
            .radiance(sample.wi)
            .unwrap()
            .component_mul(&sample.weight)
            / 256.0;
        checked[index] = true;
    }
    assert_eq!(checked, [true; 2]);
    assert_rgb(
        render_camera_glass(&scene, 1, 1, 256, 0).unwrap().pixels[0],
        linear_srgb_to_xyz(expected).unwrap(),
    );
}

#[test]
fn all_camera_entries_integrate_hdr_with_lambertian_without_changing_photon_lighting() {
    let hdr = TemporaryHdr::new(true);
    let mut config = scene_config(EnvironmentConfig {
        intensity: 0.25,
        ..hdr.config()
    });
    let renderer = Render::new(RenderSetting {
        width: 4,
        height: 4,
        samples_per_pixel_axis: 1,
        ..Default::default()
    })
    .unwrap();
    let mut solar_config = config.clone();
    solar_config.environment = None;
    let solar = Scene::from_config(&solar_config).unwrap();
    let with_environment = Scene::from_config(&config).unwrap();
    let lighting_before = renderer.render_lighting(&solar).unwrap();
    let lighting_after = renderer.render_lighting(&with_environment).unwrap();
    assert_eq!(lighting_before.direct.pixels, lighting_after.direct.pixels);
    assert_eq!(
        lighting_before.caustics.pixels,
        lighting_after.caustics.pixels
    );
    assert_eq!(
        lighting_before.sun_irradiance_xyz,
        lighting_after.sun_irradiance_xyz
    );
    assert_eq!(
        renderer.render(&solar).unwrap().pixels,
        renderer.render(&with_environment).unwrap().pixels
    );

    config.light.total_flux = 0.0;
    let mut scene = Scene::from_config(&config).unwrap();
    fs::remove_file(&hdr.0).unwrap();
    let lighting = renderer.render_lighting(&scene).unwrap();
    let expected = linear_srgb_to_xyz(Vector3::new(8.0, 4.0, 2.0) * (0.8 * 0.25)).unwrap();
    // 混合采样即使在均匀 HDRI 下也有方差；按解析期望进行统计验收。
    for samples in [1024, 4096] {
        scene.environment_samples = samples;
        let components = render_camera_glass_components(&scene, &lighting, 3, 2, 8, 42).unwrap();
        assert!(
            components
                .caustics
                .pixels
                .iter()
                .all(|p| *p == Vector3::zeros())
        );
        let images = [
            render_camera_glass(&scene, 3, 2, 8, 42).unwrap(),
            components.direct,
            components.combined,
        ];
        for image in images {
            for xyz in image.pixels {
                assert!(
                    (xyz - expected).norm() < expected.norm() * 0.06,
                    "{xyz:?} != {expected:?}, environment_samples={samples}"
                );
            }
        }
    }
}

#[test]
fn hdr_camera_components_add_up_for_background_diffuse_and_glass() {
    let hdr = TemporaryHdr::new(false);
    for view in 0..3 {
        let mut config = scene_config(EnvironmentConfig {
            rotation_degrees: 90.0,
            intensity: 0.5,
            ..hdr.config()
        });
        config.environment_samples = 3;
        config.ground_checkerboard = Some(Default::default());
        if view == 0 {
            config.camera.target = [0.0, 0.0, 4.0];
        }
        let mut scene = Scene::from_config(&config).unwrap();
        if view == 2 {
            scene.add_with_material(
                Box::new(spectral_caustics::geometry::ground::Ground::new(2.0).unwrap()),
                Box::new(spectral_caustics::material::dielectric::Dielectric::new(1.5).unwrap()),
            );
        }
        let mut lighting = SpectralLighting {
            direct: Framebuffer::new(1, 1),
            caustics: Framebuffer::new(1, 1),
            receiver: scene.receiver,
            light_wi: Vector3::z(),
            sun_irradiance_xyz: Vector3::new(0.2, 0.3, 0.4),
        };
        // 这张 sampled direct 不能参与相机解析直射。
        lighting.direct.pixels.fill(Vector3::repeat(100.0));
        lighting.caustics.pixels.fill(Vector3::new(0.4, 0.2, 0.1));
        for (spp, seed) in [(1, 0), (16, 42), (7, 123)] {
            let images =
                render_camera_glass_components(&scene, &lighting, 7, 5, spp, seed).unwrap();
            for ((direct, caustics), combined) in images
                .direct
                .pixels
                .iter()
                .zip(&images.caustics.pixels)
                .zip(&images.combined.pixels)
            {
                assert_rgb(*direct + caustics, *combined);
            }
            if view == 0 {
                assert!(
                    images
                        .caustics
                        .pixels
                        .iter()
                        .all(|p| *p == Vector3::zeros())
                );
                let background = render_camera_glass(&scene, 7, 5, spp, seed).unwrap();
                assert_eq!(images.direct.pixels, background.pixels);
            }
        }
    }
}

#[test]
fn environment_direction_samples_apply_forward_rotation_and_inverse_pdf() {
    let hdr = TemporaryHdr::new(false);
    let map = EnvironmentMap::load(&hdr.0).unwrap();
    let environments: Vec<_> = [0.0, 90.0, 37.0, -450.0, 450.0]
        .into_iter()
        .map(|rotation_degrees| {
            (
                rotation_degrees,
                Environment::try_from(&EnvironmentConfig {
                    rotation_degrees,
                    ..hdr.config()
                })
                .unwrap(),
            )
        })
        .collect();
    // 分布、像素和旋转均已缓存；后续采样不依赖源文件。
    fs::remove_file(&hdr.0).unwrap();
    for (degrees, environment) in environments {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let mut local_rng = StdRng::seed_from_u64(42);
        let mut world_rng = StdRng::seed_from_u64(42);
        for _ in 0..2048 {
            let local = map.sample_direction(&mut local_rng).unwrap();
            let world = environment.sample_direction(&mut world_rng).unwrap();
            let expected = Vector3::new(
                cos * local.direction.x - sin * local.direction.y,
                sin * local.direction.x + cos * local.direction.y,
                local.direction.z,
            );
            assert!((world.direction - expected).norm() < 1e-14);
            assert!((world.direction.norm() - 1.0).abs() < 1e-14);
            assert!(world.pdf.is_finite() && world.pdf > 0.0);
            assert!((world.pdf - local.pdf).abs() < 1e-14);
            assert_eq!(environment.pdf_direction(world.direction), Some(world.pdf));
            assert_rgb(
                environment.radiance(world.direction).unwrap(),
                map.radiance(local.direction).unwrap(),
            );
        }
        // 独立查询轴向附近、接缝两侧和两极。分段常数 PDF 在像素边界可不连续，
        // 非极点方向稍偏离边界，避免浮点旋转把查询移到相邻像素。
        for local in [
            Vector3::new(1.0, 0.013, 0.019),
            Vector3::new(0.013, 1.0, -0.019),
            Vector3::new(0.013, -1.0, 0.019),
            Vector3::new(-1.0, 0.013, -0.019),
            Vector3::new(-1.0, -0.013, 0.019),
            Vector3::z(),
            -Vector3::z(),
        ] {
            let world = Vector3::new(
                cos * local.x - sin * local.y,
                sin * local.x + cos * local.y,
                local.z,
            );
            assert_eq!(environment.pdf_direction(world), map.pdf_direction(local));
        }
    }
}

#[test]
fn gradient_direction_sampling_is_uniform_on_the_full_sphere() {
    let environments: Vec<_> = [0.0, 90.0, -450.0]
        .into_iter()
        .map(|rotation_degrees| {
            Environment::try_from(&EnvironmentConfig {
                rotation_degrees,
                horizon_linear_rgb: [100.0, 1.0, 0.0],
                zenith_linear_rgb: [0.0, 2.0, 50.0],
                ..Default::default()
            })
            .unwrap()
        })
        .collect();
    let mut rng = StdRng::seed_from_u64(7);
    let mut mean = Vector3::zeros();
    let mut north_count = 0;
    for _ in 0..8192 {
        let sample = environments[0].sample_direction(&mut rng).unwrap();
        assert!((sample.direction.norm() - 1.0).abs() < 1e-14);
        assert_eq!(sample.pdf, 1.0 / (4.0 * std::f64::consts::PI));
        for environment in &environments {
            assert_eq!(
                environment.pdf_direction(sample.direction),
                Some(sample.pdf)
            );
        }
        north_count += usize::from(sample.direction.z > 0.0);
        mean += sample.direction;
    }
    assert!((mean / 8192.0).norm() < 0.03);
    assert!((north_count as f64 / 8192.0 - 0.5).abs() < 0.02);
    // 绕 Z 的旋转对渐变无影响，方向采样不受旋转影响。
    for environment in &environments {
        let mut rng = StdRng::seed_from_u64(42);
        let mut replay = StdRng::seed_from_u64(42);
        for _ in 0..64 {
            assert_eq!(
                environment.sample_direction(&mut rng),
                environments[0].sample_direction(&mut replay)
            );
        }
    }
}

#[test]
fn environment_pdf_rejects_invalid_directions_and_handles_extreme_finite_scales() {
    let hdr = TemporaryHdr::new(false);
    for map_path in [None, Some(hdr.0.clone())] {
        let environment = Environment::try_from(&EnvironmentConfig {
            map_path,
            rotation_degrees: 37.0,
            ..Default::default()
        })
        .unwrap();
        assert!(environment.pdf_direction(Vector3::zeros()).is_none());
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for axis in 0..3 {
                let mut direction = Vector3::repeat(1.0);
                direction[axis] = value;
                assert!(environment.pdf_direction(direction).is_none());
            }
        }
        let expected = environment.pdf_direction(Vector3::repeat(1.0));
        for scale in [f64::MAX, 1e-300, f64::from_bits(1)] {
            assert_eq!(environment.pdf_direction(Vector3::repeat(scale)), expected);
        }
        for direction in [Vector3::x(), -Vector3::x(), Vector3::z(), -Vector3::z()] {
            assert!(environment.pdf_direction(direction).unwrap() > 0.0);
        }
    }
}

#[test]
fn direction_pdf_is_independent_of_intensity_and_invalid_environments_preserve_rng() {
    let hdr = TemporaryHdr::new(false);
    for map_path in [None, Some(hdr.0.clone())] {
        let mut environment = Environment::try_from(&EnvironmentConfig {
            map_path,
            rotation_degrees: 37.0,
            ..Default::default()
        })
        .unwrap();
        let mut reference_rng = StdRng::seed_from_u64(42);
        let reference = environment.sample_direction(&mut reference_rng).unwrap();
        let radiance = environment.radiance(reference.direction).unwrap();
        for intensity in [0.0, 0.5, 2.0, f64::MAX] {
            environment.intensity = intensity;
            assert_eq!(
                environment.sample_direction(&mut StdRng::seed_from_u64(42)),
                Some(reference)
            );
            assert_eq!(
                environment.pdf_direction(reference.direction),
                Some(reference.pdf)
            );
            if intensity <= 2.0 {
                assert_rgb(
                    environment.radiance(reference.direction).unwrap(),
                    radiance * intensity,
                );
            }
        }
        let mut rng = StdRng::seed_from_u64(42);
        let mut untouched = rng.clone();
        for intensity in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            environment.intensity = intensity;
            assert!(environment.sample_direction(&mut rng).is_none());
            assert!(environment.pdf_direction(Vector3::z()).is_none());
        }
        environment.intensity = 1.0;
        environment.horizon_color.x = f64::NAN;
        assert!(environment.sample_direction(&mut rng).is_none());
        assert!(environment.pdf_direction(Vector3::z()).is_none());
        assert_eq!(rng.random::<u64>(), untouched.random::<u64>());
    }
}

#[test]
fn shared_environment_sampling_is_deterministic_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Environment>();
    assert_send_sync::<EnvironmentMap>();
    let hdr = TemporaryHdr::new(false);
    let environments: Vec<_> = [None, Some(hdr.0.clone())]
        .into_iter()
        .map(|map_path| {
            Environment::try_from(&EnvironmentConfig {
                map_path,
                rotation_degrees: 37.0,
                ..Default::default()
            })
            .unwrap()
        })
        .collect();
    fs::remove_file(&hdr.0).unwrap();
    for environment in &environments {
        let sample_stream = |seed| {
            let mut rng = StdRng::seed_from_u64(seed);
            (0..128)
                .map(|_| {
                    let sample = environment.sample_direction(&mut rng).unwrap();
                    assert_eq!(
                        environment.pdf_direction(sample.direction),
                        Some(sample.pdf)
                    );
                    sample
                })
                .collect::<Vec<_>>()
        };
        let serial: Vec<_> = (0..16).map(sample_stream).collect();
        for threads in [1, 2, 4] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let parallel = pool.install(|| {
                (0..16)
                    .into_par_iter()
                    .map(sample_stream)
                    .collect::<Vec<_>>()
            });
            assert_eq!(parallel, serial);
        }
    }
}

#[test]
fn mixed_hdri_camera_is_thread_independent_and_preserves_caustics() {
    let hdr = TemporaryHdr::new(false);
    let config = SceneConfig {
        environment: Some(EnvironmentConfig {
            rotation_degrees: 37.0,
            ..hdr.config()
        }),
        environment_samples: 3,
        ..Default::default()
    };
    let mut scene = Scene::from_config(&config).unwrap();
    let without_environment = Scene::from_config(&SceneConfig {
        environment: None,
        ..config
    })
    .unwrap();
    fs::remove_file(&hdr.0).unwrap();
    let settings = RenderSetting {
        width: 16,
        height: 12,
        samples_per_pixel_axis: 1,
        thread_count: Some(1),
        ..Default::default()
    };
    let renderer = Render::new(settings.clone()).unwrap();
    let lighting = renderer.render_lighting(&scene).unwrap();
    let solar_lighting = renderer.render_lighting(&without_environment).unwrap();
    assert_eq!(lighting.direct.pixels, solar_lighting.direct.pixels);
    assert_eq!(lighting.caustics.pixels, solar_lighting.caustics.pixels);
    assert_eq!(
        lighting.sun_irradiance_xyz,
        solar_lighting.sun_irradiance_xyz
    );
    for heuristic in [
        EnvironmentMisHeuristic::Balance,
        EnvironmentMisHeuristic::Power,
    ] {
        scene.environment_mis_heuristic = heuristic;
        for samples in [1, 2, 3, 4, 8] {
            scene.environment_samples = samples;
            for seed in [0, 42] {
                let baseline = renderer
                    .render_camera_glass_components(&scene, &lighting, 16, seed)
                    .unwrap();
                let solar = renderer
                    .render_camera_glass_components(&without_environment, &solar_lighting, 16, seed)
                    .unwrap();
                assert_eq!(baseline.caustics.pixels, solar.caustics.pixels);
                assert_ne!(baseline.direct.pixels, solar.direct.pixels);
                for threads in [1, 2, 4] {
                    let parallel = Render::new(RenderSetting {
                        thread_count: Some(threads),
                        ..settings.clone()
                    })
                    .unwrap();
                    for _ in 0..2 {
                        let images = parallel
                            .render_camera_glass_components(&scene, &lighting, 16, seed)
                            .unwrap();
                        assert_eq!(images.direct.pixels, baseline.direct.pixels);
                        assert_eq!(images.caustics.pixels, baseline.caustics.pixels);
                        assert_eq!(images.combined.pixels, baseline.combined.pixels);
                        for ((direct, caustics), combined) in images
                            .direct
                            .pixels
                            .iter()
                            .zip(&images.caustics.pixels)
                            .zip(&images.combined.pixels)
                        {
                            assert!((direct + caustics - combined).norm() < 1e-12);
                        }
                    }
                }
            }
        }
    }
}

//! 相机积分器的原有单元测试。
use super::*;
use super::{
    direct::camera_lighting_setup,
    environment::environment_lighting,
    path::trace_camera_ray as trace_camera_path,
    rng::{CAMERA_STREAM, DIFFUSE_STREAM, ENVIRONMENT_SEED_OFFSET, ENVIRONMENT_STREAM, pixel_seed},
    types::CameraPathRngs,
};
use crate::{
    camera::Camera,
    config::{
        cameraconfig::CameraConfig,
        materialconfig::MaterialConfig,
        sceneconfig::{EnvironmentConfig, ObjectConfig, ParallelLightConfig, SceneConfig},
    },
    geometry::{ground::Ground, hit::Hit, object::Hittable, prism::Prism, triangle::Triangle},
    material::{GlassEvent, GlassSample, Material, dielectric::Dielectric, lambertian::Lambertian},
    render::utils::xyz_to_linear_srgb,
};
use crate::{
    geometry::ray::Ray,
    material::{BsdfValue, EvaluateMode, MaterialSample},
    render::{SpectralLighting, framebuffer::Framebuffer, utils::linear_srgb_to_xyz},
    scene::{GroundReceiver, Scene},
};
use nalgebra::{Vector2, Vector3};
use rand::{SeedableRng, rngs::StdRng};
use std::f64::consts::PI;

#[test]
fn pixel_seeds_separate_pixels_and_random_streams() {
    for base_seed in [0, 42, u64::MAX] {
        let mut seeds = std::collections::HashSet::new();
        for index in 0..1024 {
            for stream in [CAMERA_STREAM, ENVIRONMENT_STREAM, DIFFUSE_STREAM] {
                assert!(seeds.insert(pixel_seed(base_seed, index, stream)));
            }
        }
    }
}

fn ground_scene() -> Scene {
    Scene::from_config(&SceneConfig {
        environment: None, // 隔离历史太阳/阴影测试的输入。
        ground_checkerboard: None,
        light: ParallelLightConfig {
            direction: [0.0, 0.0, -1.0],
            ..Default::default()
        },
        camera: CameraConfig {
            position: [0.0, 0.0, 2.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 45.0,
        },
        objects: vec![ObjectConfig::Ground {
            height_m: 0.0,
            material: MaterialConfig::Lambertian {
                albedo: [0.2, 0.5, 0.8],
            },
        }],
        ..SceneConfig::default()
    })
    .unwrap()
}

fn neutral_ground_scene(rho: f64) -> Scene {
    Scene::from_config(&SceneConfig {
        environment: None,
        ground_checkerboard: None,
        light: ParallelLightConfig {
            direction: [0.0, 0.0, -1.0],
            ..Default::default()
        },
        camera: CameraConfig {
            position: [0.0, 0.0, 2.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 45.0,
        },
        objects: vec![ObjectConfig::Ground {
            height_m: 0.0,
            material: MaterialConfig::Lambertian { albedo: [rho; 3] },
        }],
        ..SceneConfig::default()
    })
    .unwrap()
}

fn uniform_ground_map(xyz: Vector3<f64>) -> Framebuffer {
    let mut map = Framebuffer::new(1, 1);
    assert!(map.set(0, 0, xyz));
    map
}

fn estimate_environment(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    seed: u64,
) -> Vector3<f64> {
    environment_lighting(scene, hit, material, wo, &mut StdRng::seed_from_u64(seed)).unwrap()
}

#[test]
fn constant_environment_recovers_albedo_for_every_surface_orientation_and_sample_count() {
    let mut scene = Scene::new();
    let sky = Vector3::new(0.1, 0.2, 4.0);
    scene.environment = Some(
        crate::scene::Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: sky.into(),
            zenith_linear_rgb: sky.into(),
            ..Default::default()
        })
        .unwrap(),
    );
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let diffuse = Lambertian::new(albedo).unwrap();
    for samples in [1, 16, 1024] {
        scene.environment_samples = samples;
        for normal in [
            Vector3::z(),
            Vector3::x(),
            -Vector3::z(),
            Vector3::new(0.6, 0.0, 0.8),
        ] {
            let hit = Hit {
                normal: normal * 3.0,
                ..Hit::default()
            };
            let actual = estimate_environment(&scene, &hit, &diffuse, normal, 42);
            assert!((actual - albedo.component_mul(&sky)).norm() < 1e-10);
            assert_eq!(
                estimate_environment(&scene, &hit, &diffuse, -normal, 42),
                Vector3::zeros()
            );
            assert_eq!(
                estimate_environment(&scene, &hit, &TestGlass(None), normal, 42),
                Vector3::zeros()
            );
        }
    }
    let environment = scene.environment;
    let mut scene = ground_scene();
    scene.environment = environment;
    scene.light.total_flux = 0.0;
    for samples in [1, 8] {
        scene.environment_samples = samples;
        for spp in [1, 8] {
            let image = render_camera_glass(&scene, 3, 2, spp, 42).unwrap();
            for xyz in image.pixels {
                assert!(
                    (xyz_to_linear_srgb(xyz).unwrap() - albedo.component_mul(&sky)).norm() < 1e-12
                );
            }
        }
    }
}

#[test]
fn gradient_environment_converges_to_the_cosine_weighted_integral() {
    let mut scene = Scene::new(); // 空场景，所有环境方向可见。
    let sky = scene.environment.as_ref().unwrap();
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let material = Lambertian::new(albedo).unwrap();
    let hit = Hit {
        normal: Vector3::z(),
        ..Hit::default()
    };
    // 余弦分布下 E[wi.z]=2/3；直接积分渐变可得到独立解析参考值。
    let expected =
        albedo.component_mul(&(sky.horizon_color / 3.0 + sky.zenith_color * (2.0 / 3.0)));
    let mut errors = [0.0; 2];
    for (index, samples) in [1, 256].into_iter().enumerate() {
        scene.environment_samples = samples;
        for seed in 0..64 {
            let estimate = estimate_environment(&scene, &hit, &material, Vector3::z(), seed);
            errors[index] += (estimate - expected).norm_squared() / 64.0;
        }
    }
    assert!(errors[1] < errors[0] / 32.0, "MSE: {errors:?}");
    scene.environment_samples = 65536;
    let estimate = estimate_environment(&scene, &hit, &material, Vector3::z(), 42);
    assert!((estimate - expected).norm() < 0.001);
    assert_eq!(
        estimate,
        estimate_environment(&scene, &hit, &material, Vector3::z(), 42)
    );
}

#[test]
fn environment_visibility_keeps_blocked_samples_in_the_denominator() {
    struct HalfPlane(Ground);
    impl Hittable for HalfPlane {
        fn intersect(&self, ray: &Ray) -> Option<Hit> {
            let hit = self.0.intersect(ray)?;
            (hit.coords.y > 0.0).then_some(hit)
        }
    }
    let mut scene = Scene::new();
    let li = Vector3::new(0.25, 0.5, 1.0);
    scene.environment = Some(
        crate::scene::Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: li.into(),
            zenith_linear_rgb: li.into(),
            ..Default::default()
        })
        .unwrap(),
    );
    scene.environment_samples = 32768;
    scene.add(Box::new(Ground::new(0.0).unwrap())); // 起点偏移必须避免地面自遮挡。
    scene.add(Box::new(HalfPlane(Ground::new(1.0).unwrap())));
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let material = Lambertian::new(albedo).unwrap();
    let hit = Hit {
        normal: Vector3::z(),
        ..Hit::default()
    };
    let actual = estimate_environment(&scene, &hit, &material, Vector3::z(), 42);
    let expected = albedo.component_mul(&li) * 0.5;
    assert!((actual - expected).norm() < 0.01);
    scene.add(Box::new(Ground::new(2.0).unwrap()));
    assert_eq!(
        estimate_environment(&scene, &hit, &material, Vector3::z(), 42),
        Vector3::zeros()
    );
}

#[test]
fn opaque_visibility_blocks_sky_below_a_glass_plane() {
    let mut scene = neutral_ground_scene(0.8);
    scene.environment = Scene::new().environment;
    scene.add_with_material(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(TestGlass(Some(transmission(-Vector3::z(), 0.25, 0.5)))),
    );
    let lighting = split_ground_map(&scene);
    let render = |component| {
        render_camera_glass_with_lighting(&scene, &lighting, 3, 2, 8, 42, component).unwrap()
    };
    let direct = render(GroundLightingComponent::Direct);
    let combined = render(GroundLightingComponent::Combined);
    assert_eq!(direct.pixels, combined.pixels);
    let expected = Vector3::zeros(); // 向上的环境射线全部被无限玻璃面遮挡。
    for xyz in direct.pixels {
        assert!((xyz_to_linear_srgb(xyz).unwrap() - expected).norm() < 1e-12);
    }
    assert!(
        render(GroundLightingComponent::Caustics)
            .pixels
            .iter()
            .all(|p| *p == Vector3::zeros())
    );
}

#[test]
fn glass_misses_use_the_final_direction_for_sky_gradient() {
    let mut scene = glass_background_scene();
    scene.environment = Scene::new().environment;
    let sky = scene.environment.as_ref().unwrap();
    for (direction, weight, expected) in [
        (Vector3::z(), 0.5, sky.zenith_color),
        (-Vector3::z(), 0.25, sky.horizon_color),
    ] {
        scene.objects.clear();
        scene.add_with_material(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(TestGlass(Some(transmission(direction, 0.5, weight)))),
        );
        assert!((trace_center_pixel(&scene, 42) - expected * weight).norm() < 1e-12);
    }
}

#[test]
fn checkerboard_is_applied_once_after_glass_with_sky_and_caustics() {
    let mut scene = neutral_ground_scene(0.8);
    scene.environment = Scene::new().environment;
    scene.ground_checkerboard = Scene::new().ground_checkerboard;
    scene.camera = Camera::from_config(&CameraConfig {
        position: [0.75, 0.25, 2.0],
        target: [0.75, 0.25, 0.0],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 0.001,
    })
    .unwrap();
    scene.add_with_material(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(TestGlass(Some(transmission(-Vector3::z(), 0.25, 0.5)))),
    );
    let mut lighting = split_ground_map(&scene);
    let e = Vector3::new(0.1, 0.2, 0.4);
    lighting.caustics.pixels.fill(e);
    let expected_sky = Vector3::zeros(); // 二值可见性：上方玻璃面遮挡所有环境方向。
    let expected_caustics = e * (0.8 / PI * 0.25 * 0.5);
    let images = render_camera_glass_components(&scene, &lighting, 1, 1, 8, 42).unwrap();
    assert!((images.direct.pixels[0] - expected_sky).norm() < 1e-12);
    assert!((images.caustics.pixels[0] - expected_caustics).norm() < 1e-12);
    assert!((images.combined.pixels[0] - expected_sky - expected_caustics).norm() < 1e-12);
    for (component, expected) in [
        (GroundLightingComponent::Direct, expected_sky),
        (GroundLightingComponent::Caustics, expected_caustics),
        (
            GroundLightingComponent::Combined,
            expected_sky + expected_caustics,
        ),
    ] {
        let image =
            render_camera_glass_with_lighting(&scene, &lighting, 1, 1, 8, 42, component).unwrap();
        assert!((image.pixels[0] - expected).norm() < 1e-12);
    }
}

#[test]
fn camera_rejects_invalid_runtime_sky_parameters() {
    let mut scene = ground_scene();
    for samples in [0, usize::MAX] {
        scene.environment_samples = samples;
        assert!(render_camera_lit(&scene, 2, 1).is_err());
        assert!(render_camera_glass(&scene, 1, 1, 2, 42).is_err());
    }
    scene.environment_samples = 1;
    for channel in 0..2 {
        for value in [-1.0, f64::NAN, f64::INFINITY] {
            scene.environment = Scene::new().environment;
            let sky = scene.environment.as_mut().unwrap();
            match channel {
                0 => sky.horizon_color.x = value,
                _ => sky.zenith_color.x = value,
            }
            assert!(render_camera_lit(&scene, 1, 1).is_err());
            assert!(render_camera_glass(&scene, 1, 1, 1, 42).is_err());
        }
    }
    let mut scene = neutral_ground_scene(1.0);
    scene.environment = Scene::new().environment;
    scene.environment.as_mut().unwrap().horizon_color = Vector3::repeat(f64::MAX);
    scene.environment.as_mut().unwrap().zenith_color = Vector3::repeat(f64::MAX);
    // 有限 HDR 参数也可能在颜色空间变换中溢出，必须返回错误。
    assert!(render_camera_lit(&scene, 1, 1).is_err());
}

fn split_ground_map(scene: &Scene) -> SpectralLighting {
    super::super::caustics::render_spectral_lighting(
        scene,
        scene.prism_id,
        scene.ground_id.unwrap(),
        &crate::config::rendersetting::RenderSetting {
            width: 16,
            height: 12,
            samples_per_pixel_axis: 2,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn camera_components_trace_once_and_keep_glass_and_environment_rng_independent() {
    // 全局事件记录按采样顺序比较，固定单工作线程以免不同像素的记录交错。
    // 多线程像素一致性与专用池接入由 config_render 集成测试覆盖。
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(check_camera_components_rng);
}

fn check_camera_components_rng() {
    use std::sync::{Arc, Mutex};

    type Event = (Vector3<f64>, Vector3<f64>, GlassEvent, f64);
    struct RecordingGlass {
        glass: Dielectric,
        events: Arc<Mutex<Vec<Event>>>,
    }
    impl Material for RecordingGlass {
        fn evaluate(
            &self,
            wi: Vector3<f64>,
            wo: Vector3<f64>,
            normal: Vector3<f64>,
            mode: EvaluateMode,
        ) -> BsdfValue {
            self.glass.evaluate(wi, wo, normal, mode)
        }
        fn sample(
            &self,
            wo: Vector3<f64>,
            normal: Vector3<f64>,
            rng: &mut dyn rand::RngCore,
        ) -> Option<MaterialSample> {
            let sample = self.glass.sample(wo, normal, rng)?;
            let MaterialSample::Glass(glass) = sample else {
                unreachable!()
            };
            self.events
                .lock()
                .unwrap()
                .push((wo, glass.wi, glass.event, glass.weight));
            Some(sample)
        }
        fn is_delta(&self) -> bool {
            true
        }
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut scene = neutral_ground_scene(0.8);
    scene.environment = Scene::new().environment;
    scene.environment_samples = 3;
    scene.add_with_material(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(RecordingGlass {
            glass: Dielectric::new(1.5).unwrap(),
            events: events.clone(),
        }),
    );
    let mut lighting = split_ground_map(&scene);
    lighting.caustics.pixels.fill(Vector3::new(0.1, 0.2, 0.4));
    let (width, height, spp, seed) = (4, 3, 32, 42);
    let mut reference_events = None;
    for component in [
        GroundLightingComponent::Direct,
        GroundLightingComponent::Caustics,
        GroundLightingComponent::Combined,
    ] {
        events.lock().unwrap().clear();
        render_camera_glass_with_lighting(&scene, &lighting, width, height, spp, seed, component)
            .unwrap();
        let actual = events.lock().unwrap().clone();
        assert_eq!(actual.len(), width * height * spp);
        if let Some(reference) = &reference_events {
            assert_eq!(&actual, reference);
        } else {
            reference_events = Some(actual);
        }
    }
    let reference_events = reference_events.unwrap();
    assert!(
        reference_events
            .iter()
            .any(|e| e.2 == GlassEvent::Reflection)
    );
    assert!(
        reference_events
            .iter()
            .any(|e| e.2 == GlassEvent::Transmission)
    );
    events.lock().unwrap().clear();
    let images =
        render_camera_glass_components(&scene, &lighting, width, height, spp, seed).unwrap();
    // 三张图共计只产生 N 次玻璃采样，而三个旧调用共计 3N 次。
    assert_eq!(*events.lock().unwrap(), reference_events);
    for environment_samples in [1, 7] {
        scene.environment_samples = environment_samples;
        events.lock().unwrap().clear();
        let changed =
            render_camera_glass_components(&scene, &lighting, width, height, spp, seed).unwrap();
        assert_eq!(*events.lock().unwrap(), reference_events);
        assert_eq!(changed.caustics.pixels, images.caustics.pixels);
    }
    scene.environment = None;
    events.lock().unwrap().clear();
    let without_environment =
        render_camera_glass_components(&scene, &lighting, width, height, spp, seed).unwrap();
    assert_eq!(*events.lock().unwrap(), reference_events);
    assert_eq!(without_environment.caustics.pixels, images.caustics.pixels);
}

#[test]
fn non_ground_lambertian_sun_and_environment_only_enter_direct() {
    let mut scene = neutral_ground_scene(0.8);
    let sky = Vector3::new(0.1, 0.2, 0.4);
    scene.environment = Some(
        crate::environment::Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: sky.into(),
            zenith_linear_rgb: sky.into(),
            ..Default::default()
        })
        .unwrap(),
    );
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    scene.add_with_material(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(Lambertian::new(albedo).unwrap()),
    );
    let mut lighting = split_ground_map(&scene);
    lighting.caustics.pixels.fill(Vector3::repeat(100.0));
    let expected_rgb = albedo
        .component_mul(&(xyz_to_linear_srgb(lighting.sun_irradiance_xyz).unwrap() / PI + sky));
    let expected_xyz = linear_srgb_to_xyz(expected_rgb).unwrap();
    let images = render_camera_glass_components(&scene, &lighting, 3, 2, 8, 42).unwrap();
    for ((direct, caustics), combined) in images
        .direct
        .pixels
        .iter()
        .zip(&images.caustics.pixels)
        .zip(&images.combined.pixels)
    {
        assert!((direct - expected_xyz).norm() < 1e-12);
        assert_eq!(*caustics, Vector3::zeros());
        assert_eq!(direct, combined);
    }
}

#[test]
fn analytic_direct_is_continuous_across_receiver_and_window_without_double_counting() {
    let mut scene = neutral_ground_scene(0.8);
    scene.receiver.min_m = Vector2::repeat(-0.1);
    scene.receiver.max_m = Vector2::repeat(0.1);
    // 斜射光同样以水平窗口 Phi/A 为水平地面辐照度。
    scene.light.direction = Vector3::new(-3.0, 0.0, -4.0);
    let mut lighting = split_ground_map(&scene);
    assert!(
        lighting
            .caustics
            .pixels
            .iter()
            .all(|p| *p == Vector3::zeros())
    );
    // sampled direct 图绝不能被相机当作第二份直射累加。
    lighting.direct.pixels.fill(Vector3::repeat(100.0));
    let expected = lighting.sun_irradiance_xyz * lighting.light_wi.z * (0.8 / PI);
    for x in [
        -2.25, // 发射窗口外，但仍在有限地面内。
        -0.10000001,
        -0.09999999,
        0.0,
        0.09999999,
        0.10000001,
        2.25,
    ] {
        scene.camera = Camera::from_config(&CameraConfig {
            position: [x, 0.0, 2.0],
            target: [x, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 1.0,
        })
        .unwrap();
        for spp in [1, 8] {
            let direct = render_camera_glass_with_lighting(
                &scene,
                &lighting,
                3,
                2,
                spp,
                42,
                GroundLightingComponent::Direct,
            )
            .unwrap();
            let combined = render_camera_glass_with_lighting(
                &scene,
                &lighting,
                3,
                2,
                spp,
                42,
                GroundLightingComponent::Combined,
            )
            .unwrap();
            let images = render_camera_glass_components(&scene, &lighting, 3, 2, spp, 42).unwrap();
            assert_eq!(images.direct.pixels, direct.pixels);
            assert_eq!(images.combined.pixels, direct.pixels);
            assert!(
                images
                    .caustics
                    .pixels
                    .iter()
                    .all(|p| *p == Vector3::zeros())
            );
            assert_eq!(direct.pixels, combined.pixels);
            for xyz in direct.pixels {
                assert!((xyz - expected).norm() < 1e-12, "x={x}: {xyz:?}");
            }
        }
    }
}

#[test]
fn changing_receiver_does_not_change_analytic_sun_at_the_same_world_positions() {
    let mut scene = neutral_ground_scene(0.8);
    let mut reference = None;
    for (min, max) in [(-0.01, 0.01), (-2.0, 2.0), (-20.0, 40.0)] {
        scene.receiver.min_m = Vector2::repeat(min);
        scene.receiver.max_m = Vector2::repeat(max);
        let lighting = split_ground_map(&scene);
        let image = render_camera_glass_with_lighting(
            &scene,
            &lighting,
            5,
            3,
            8,
            42,
            GroundLightingComponent::Combined,
        )
        .unwrap();
        if let Some(pixels) = &reference {
            assert_eq!(&image.pixels, pixels);
        } else {
            reference = Some(image.pixels);
        }
    }
}

#[test]
fn glass_shadows_only_direct_and_camera_throughput_still_weights_caustics() {
    let mut scene = neutral_ground_scene(0.8);
    scene.add_with_material(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(TestGlass(Some(transmission(-Vector3::z(), 0.25, 0.5)))),
    );
    let mut lighting = split_ground_map(&scene);
    // 给地面一个已知的焦散 E；无限玻璃面会遮挡每一条向上的 shadow ray。
    let e = Vector3::new(0.1, 0.2, 1.0);
    lighting.caustics.pixels.fill(e);
    let direct = render_camera_glass_with_lighting(
        &scene,
        &lighting,
        3,
        2,
        8,
        42,
        GroundLightingComponent::Direct,
    )
    .unwrap();
    let caustics = render_camera_glass_with_lighting(
        &scene,
        &lighting,
        3,
        2,
        8,
        42,
        GroundLightingComponent::Caustics,
    )
    .unwrap();
    let combined = render_camera_glass_with_lighting(
        &scene,
        &lighting,
        3,
        2,
        8,
        42,
        GroundLightingComponent::Combined,
    )
    .unwrap();
    assert!(direct.pixels.iter().all(|p| p.norm() == 0.0));
    assert_eq!(combined.pixels, caustics.pixels);
    for xyz in caustics.pixels {
        assert!((xyz - e * (0.8 / PI) * 0.5).norm() < 1e-12);
    }
}

#[test]
fn split_camera_rejects_invalid_metadata_maps_and_non_gray_ground() {
    let scene = neutral_ground_scene(0.8);
    let mut lighting = split_ground_map(&scene);
    let render = |s: &Scene, l: &SpectralLighting| {
        assert!(render_camera_glass_components(s, l, 1, 1, 1, 42).is_err());
        render_camera_glass_with_lighting(s, l, 1, 1, 1, 42, GroundLightingComponent::Combined)
    };
    assert!(render(&ground_scene(), &lighting).is_err());
    lighting.sun_irradiance_xyz.x = f64::NAN;
    assert!(render(&scene, &lighting).is_err());
    lighting.sun_irradiance_xyz.x = 0.5;
    lighting.light_wi = Vector3::repeat(0.0);
    assert!(render(&scene, &lighting).is_err());
    lighting.light_wi = Vector3::z();
    lighting.caustics.pixels[0].x = -1.0;
    assert!(render(&scene, &lighting).is_err());
    let lighting = split_ground_map(&scene);
    for (width, height, spp) in [
        (0, 1, 1),
        (1, 0, 1),
        (1, 1, 0),
        (usize::MAX, 2, 1),
        (2, 1, usize::MAX),
    ] {
        assert!(render_camera_glass_components(&scene, &lighting, width, height, spp, 42).is_err());
    }
}

#[test]
fn ground_irradiance_sampling_preserves_world_axes_centers_and_bilinear_weights() {
    let receiver = GroundReceiver {
        min_m: Vector2::new(-2.0, -1.0),
        max_m: Vector2::new(2.0, 1.0),
    };
    let mut map = Framebuffer::new(2, 2);
    let unit = Vector3::new(1.0, 10.0, 100.0);
    for (index, pixel) in map.pixels.iter_mut().enumerate() {
        *pixel = unit * (index + 1) as f64;
    }
    for (position, scale) in [
        (Vector3::new(-1.0, 0.5, 0.0), 1.0),
        (Vector3::new(1.0, 0.5, 0.0), 2.0),
        (Vector3::new(-1.0, -0.5, 0.0), 3.0),
        (Vector3::new(1.0, -0.5, 0.0), 4.0),
        (Vector3::zeros(), 2.5),
        (Vector3::new(-0.5, 0.25, 7.0), 1.75),
        (Vector3::new(-2.0, -1.0, 0.0), 3.0),
    ] {
        let actual = sample_ground_irradiance(&map, &receiver, position).unwrap();
        assert!((actual - unit * scale).norm() < 1e-12);
    }
    for position in [
        Vector3::new(2.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(-2.01, 0.0, 0.0),
    ] {
        assert_eq!(
            sample_ground_irradiance(&map, &receiver, position),
            Some(Vector3::zeros())
        );
    }
    let uniform = uniform_ground_map(unit);
    assert_eq!(
        sample_ground_irradiance(&uniform, &receiver, Vector3::new(-2.0, -1.0, 0.0)),
        Some(unit)
    );
    let wide = Framebuffer {
        width: 3,
        height: 2,
        pixels: (1..=6).map(|v| Vector3::repeat(f64::from(v))).collect(),
    };
    assert_eq!(
        sample_ground_irradiance(&wide, &receiver, Vector3::zeros()),
        Some(Vector3::repeat(3.5))
    );
}

#[test]
fn ground_irradiance_sampling_rejects_invalid_input_and_pixel_data() {
    let mut scene = neutral_ground_scene(0.8);
    let mut map = uniform_ground_map(Vector3::repeat(1.0));
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(sample_ground_irradiance(&map, &scene.receiver, Vector3::repeat(value)).is_none());
    }
    for value in [-1.0, f64::NAN, f64::INFINITY] {
        map.pixels[0].x = value;
        assert!(sample_ground_irradiance(&map, &scene.receiver, Vector3::zeros()).is_none());
    }
    map.pixels[0].x = 1.0;
    scene.receiver.max_m.x = scene.receiver.min_m.x;
    assert!(sample_ground_irradiance(&map, &scene.receiver, Vector3::zeros()).is_none());
    scene.receiver.max_m.x = f64::INFINITY;
    assert!(sample_ground_irradiance(&map, &scene.receiver, Vector3::zeros()).is_none());
    let receiver = neutral_ground_scene(0.8).receiver;
    map.pixels.clear();
    assert!(sample_ground_irradiance(&map, &receiver, Vector3::zeros()).is_none());
    map.width = 0;
    assert!(sample_ground_irradiance(&map, &receiver, Vector3::zeros()).is_none());
}

#[test]
fn neutral_lambertian_converts_irradiance_to_radiance_without_changing_chromaticity() {
    let e = Vector3::new(2.0, 3.0, 4.0);
    for rho in [0.0, 0.2, 0.8, 1.0] {
        assert!((lambertian_radiance(e, rho).unwrap() - e * rho / PI).norm() < 1e-14);
    }
    assert_eq!(
        lambertian_radiance(Vector3::zeros(), 0.8),
        Some(Vector3::zeros())
    );
    assert!(
        lambertian_radiance(Vector3::repeat(f64::MAX), 1.0)
            .unwrap()
            .iter()
            .all(|v| v.is_finite())
    );
    for rho in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
        assert!(lambertian_radiance(e, rho).is_none());
    }
    for value in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(lambertian_radiance(Vector3::repeat(value), 0.8).is_none());
    }
}

#[test]
fn ground_map_supplies_complete_radiance_and_keeps_glass_path_weights() {
    let mut scene = neutral_ground_scene(0.8);
    // 此 XYZ 超出 sRGB 色域，计算过程也必须保留，不能提前裁剪成 RGB。
    let e = Vector3::new(0.1, 0.2, 1.0);
    let map = uniform_ground_map(e);
    let expected = e * (0.8 / PI);
    for spp in [1, 8] {
        let image = render_camera_glass_with_ground(&scene, &map, 3, 2, spp, 42).unwrap();
        for xyz in image.pixels {
            assert!((xyz - expected).norm() < 1e-12);
        }
    }
    let black = uniform_ground_map(Vector3::zeros());
    assert!(
        render_camera_glass_with_ground(&scene, &black, 3, 2, 4, 42)
            .unwrap()
            .pixels
            .iter()
            .all(|p| p.norm() == 0.0)
    );
    // 无限玻璃面会挡住原来的 shadow ray；已积分的 E 不应再次被该阴影清零。
    scene.add_with_material(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(TestGlass(Some(transmission(-Vector3::z(), 0.25, 0.5)))),
    );
    let image = render_camera_glass_with_ground(&scene, &map, 3, 2, 8, 42).unwrap();
    for xyz in image.pixels {
        assert!((xyz - expected * 0.5).norm() < 1e-12);
    }
}

#[test]
fn ground_map_only_shades_bound_ground_front_side_and_returns_zero_outside_receiver() {
    let mut scene = neutral_ground_scene(0.8);
    let map = uniform_ground_map(Vector3::repeat(1.0));
    scene.add_with_material(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(Lambertian::new(Vector3::new(0.1, 0.2, 0.3)).unwrap()),
    );
    assert_eq!(
        render_camera_glass_with_ground(&scene, &map, 3, 2, 4, 42)
            .unwrap()
            .pixels,
        render_camera_glass(&scene, 3, 2, 4, 42).unwrap().pixels
    );
    let mut scene = neutral_ground_scene(0.8);
    for (position, target) in [
        ([0.0, 0.0, -2.0], [0.0; 3]),
        ([2.25, 0.0, 2.0], [2.25, 0.0, 0.0]), // receiver 外、有限地面内。
    ] {
        scene.camera = Camera::from_config(&CameraConfig {
            position,
            target,
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 10.0,
        })
        .unwrap();
        assert!(
            render_camera_glass_with_ground(&scene, &map, 1, 1, 8, 42)
                .unwrap()
                .pixels
                .iter()
                .all(|p| p.norm() == 0.0)
        );
    }
}

#[test]
fn ground_renderer_rejects_invalid_maps_bindings_and_colored_ground() {
    let map = uniform_ground_map(Vector3::repeat(1.0));
    assert!(render_camera_glass_with_ground(&ground_scene(), &map, 1, 1, 1, 42).is_err());
    let mut scene = neutral_ground_scene(0.8);
    for id in [None, Some(usize::MAX)] {
        scene.ground_id = id;
        assert!(render_camera_glass_with_ground(&scene, &map, 1, 1, 1, 42).is_err());
    }
    let scene = neutral_ground_scene(0.8);
    for value in [-1.0, f64::NAN, f64::INFINITY] {
        let mut invalid = uniform_ground_map(Vector3::zeros());
        invalid.pixels[0].x = value;
        assert!(render_camera_glass_with_ground(&scene, &invalid, 1, 1, 1, 42).is_err());
    }
    let invalid = Framebuffer {
        width: 2,
        height: 1,
        pixels: vec![Vector3::zeros()],
    };
    assert!(render_camera_glass_with_ground(&scene, &invalid, 1, 1, 1, 42).is_err());
    assert!(render_camera_glass_with_ground(&scene, &map, 1, 1, 0, 42).is_err());
    let mut scene = Scene::new();
    scene.ground_id = Some(scene.add(Box::new(Ground::new(0.0).unwrap())));
    assert!(render_camera_glass_with_ground(&scene, &map, 1, 1, 1, 42).is_err());
    let mut scene = Scene::new();
    scene.ground_id = Some(scene.add_with_material(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    ));
    assert!(render_camera_glass_with_ground(&scene, &map, 1, 1, 1, 42).is_err());
}

fn glass_background_scene() -> Scene {
    let mut scene = Scene::new();
    scene.environment = None;
    scene.camera = Camera::from_config(&CameraConfig {
        position: [0.0, 0.0, 3.0],
        target: [0.0; 3],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 45.0,
    })
    .unwrap();
    scene.background_color = Vector3::new(0.2, 0.4, 0.8);
    scene
}

struct TestGlass(Option<MaterialSample>);

impl Material for TestGlass {
    fn evaluate(
        &self,
        _wi: Vector3<f64>,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
        _mode: EvaluateMode,
    ) -> BsdfValue {
        panic!("玻璃路径应使用采样记录，不能直接求普通 BSDF");
    }

    fn sample(
        &self,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
        _rng: &mut dyn rand::RngCore,
    ) -> Option<MaterialSample> {
        self.0
    }

    fn is_delta(&self) -> bool {
        true
    }
}

fn transmission(wi: Vector3<f64>, probability: f64, weight: f64) -> MaterialSample {
    MaterialSample::Glass(GlassSample {
        wi,
        event: GlassEvent::Transmission,
        probability,
        weight,
    })
}

fn trace_center_pixel(scene: &Scene, seed: u64) -> Vector3<f64> {
    let (light_wi, light) = camera_lighting_setup(scene, 1, 1).unwrap();
    let ray = scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap();
    trace_camera_path(
        scene,
        ray,
        &mut CameraPathRngs {
            camera: &mut StdRng::seed_from_u64(seed),
            environment: &mut StdRng::seed_from_u64(seed ^ ENVIRONMENT_SEED_OFFSET),
            diffuse: &mut StdRng::seed_from_u64(seed ^ DIFFUSE_STREAM),
        },
        light_wi,
        light,
        None,
    )
    .unwrap()
    .select(GroundLightingComponent::Combined)
}

#[test]
fn camera_spp_averages_independent_paths_and_is_reproducible() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct AlternatingGlass(Arc<AtomicUsize>);
    impl Material for AlternatingGlass {
        fn evaluate(
            &self,
            _wi: Vector3<f64>,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
            mode: EvaluateMode,
        ) -> BsdfValue {
            match mode {
                EvaluateMode::Rgb => BsdfValue::Rgb(Vector3::zeros()),
                EvaluateMode::Spectral { .. } => BsdfValue::Spectral(0.0),
            }
        }
        fn sample(
            &self,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
            _rng: &mut dyn rand::RngCore,
        ) -> Option<MaterialSample> {
            let index = self.0.fetch_add(1, Ordering::Relaxed);
            Some(MaterialSample::Glass(GlassSample {
                wi: Vector3::z(),
                event: GlassEvent::Reflection,
                probability: 0.5,
                weight: if index & 1 == 0 { 0.25 } else { 1.75 },
            }))
        }
        fn is_delta(&self) -> bool {
            true
        }
    }
    for (spp, expected_weight) in [(1, 0.25), (3, 0.75), (4, 1.0), (64, 1.0)] {
        let count = Arc::new(AtomicUsize::new(0));
        let mut scene = glass_background_scene();
        scene.add_with_material(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(AlternatingGlass(count.clone())),
        );
        let image = render_camera_glass(&scene, 1, 1, spp, 42).unwrap();
        let rgb = xyz_to_linear_srgb(image.pixels[0]).unwrap();
        assert!((rgb - scene.background_color * expected_weight).norm() < 1e-12);
        assert_eq!(count.load(Ordering::Relaxed), spp);
    }
    let mut scene = glass_background_scene();
    scene.add_with_material(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let reference = render_camera_glass(&scene, 16, 8, 64, 42).unwrap();
    assert_eq!(
        reference.pixels,
        render_camera_glass(&scene, 16, 8, 64, 42).unwrap().pixels
    );
    assert_ne!(
        reference.pixels,
        render_camera_glass(&scene, 16, 8, 64, 43).unwrap().pixels
    );
}

#[test]
fn jittered_camera_filters_half_pixel_coverage_in_linear_light() {
    struct LeftHalfPlane;
    impl Hittable for LeftHalfPlane {
        fn intersect(&self, ray: &Ray) -> Option<Hit> {
            let hit = Ground::new(0.0).unwrap().intersect(ray)?;
            (hit.coords.x < 0.0).then_some(hit)
        }
    }
    let mut scene = glass_background_scene();
    scene.background_color = Vector3::repeat(1.0);
    // 左半像素命中无材质黑色表面，右半像素看到白色背景，面积均值应为线性 0.5。
    scene.add(Box::new(LeftHalfPlane));
    let image = render_camera_glass(&scene, 1, 1, 4096, 42).unwrap();
    let rgb = xyz_to_linear_srgb(image.pixels[0]).unwrap();
    assert!((rgb - Vector3::repeat(0.5)).norm() < 0.03);
    assert_eq!(
        image.pixels,
        render_camera_glass(&scene, 1, 1, 4096, 42).unwrap().pixels
    );
}

#[test]
fn camera_spp_preserves_deterministic_brightness_and_rejects_invalid_counts() {
    let scene = ground_scene();
    let reference = render_camera_lit(&scene, 3, 2).unwrap();
    for spp in [1, 3, 64] {
        let image = render_camera_glass(&scene, 3, 2, spp, 42).unwrap();
        for (actual, expected) in image.pixels.iter().zip(&reference.pixels) {
            assert!((actual - expected).norm() < 1e-12);
        }
    }
    assert!(render_camera_glass(&scene, 1, 1, 0, 42).is_err());
    assert!(render_camera_glass(&scene, 2, 1, usize::MAX, 42).is_err());
}

#[test]
fn camera_spp_reduces_error_against_analytic_fresnel_expectation() {
    use crate::material::dielectric::fresnel_dielectric;
    let mut scene = glass_background_scene();
    scene.add_with_material(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let error = |spp| {
        let image = render_camera_glass(&scene, 32, 16, spp, 42).unwrap();
        let mut sum = 0.0;
        for y in 0..16 {
            for x in 0..32 {
                let mut f = 0.0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let offset =
                            Vector2::new((f64::from(sx) + 0.5) / 4.0, (f64::from(sy) + 0.5) / 4.0);
                        let ray = scene
                            .camera
                            .ray_for_pixel_sample(x, y, 32, 16, offset)
                            .unwrap();
                        f += fresnel_dielectric(-ray.direction.z, 1.0, 1.5).unwrap() / 16.0;
                    }
                }
                let expected = scene.background_color * (f + (1.0 - f) * (4.0 / 9.0));
                let rgb = xyz_to_linear_srgb(*image.get(x, y).unwrap()).unwrap();
                sum += (rgb - expected).norm_squared();
            }
        }
        sum / (32.0 * 16.0)
    };
    let single = error(1);
    let multiple = error(64);
    assert!(
        multiple < single / 8.0,
        "1 SPP MSE={single}, 64 SPP MSE={multiple}"
    );
}

#[test]
fn glass_renderer_matches_direct_lighting_without_glass_and_respects_unbound_hits() {
    let mut scene = ground_scene();
    for seed in [0, 42] {
        assert_eq!(
            render_camera_glass(&scene, 3, 2, 1, seed).unwrap().pixels,
            render_camera_lit(&scene, 3, 2).unwrap().pixels
        );
    }
    scene.add(Box::new(Ground::new(1.0).unwrap()));
    assert_eq!(
        render_camera_glass(&scene, 1, 1, 1, 42).unwrap().pixels,
        vec![Vector3::zeros()]
    );
    scene.objects.clear();
    scene.background_color = Vector3::new(0.1, 0.3, 0.7);
    assert_eq!(
        render_camera_glass(&scene, 3, 2, 1, 42).unwrap().pixels,
        render_camera_lit(&scene, 3, 2).unwrap().pixels
    );
}

#[test]
fn glass_renderer_weights_background_on_both_sides_and_repeats_seeded_paths() {
    let glass = Dielectric::new(1.5).unwrap();
    for (position, target) in [
        ([0.0, 0.0, 3.0], [0.0; 3]),
        ([0.0; 3], [0.0, 0.0, 1.0]),
        ([-3.0_f64.sqrt(), 0.0, 0.0], [0.0, 0.0, 1.0]),
    ] {
        let mut scene = glass_background_scene();
        scene.camera = Camera::from_config(&CameraConfig {
            position,
            target,
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 45.0,
        })
        .unwrap();
        scene.add_with_material(Box::new(Ground::new(1.0).unwrap()), Box::new(glass.clone()));
        let mut saw_reflection = false;
        let mut saw_transmission = false;
        for seed in 0..64 {
            let wo = -scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap().direction;
            let MaterialSample::Glass(sample) = glass
                .sample(wo, Vector3::z(), &mut StdRng::seed_from_u64(seed))
                .unwrap()
            else {
                panic!("必须是玻璃采样");
            };
            saw_reflection |= sample.event == GlassEvent::Reflection;
            saw_transmission |= sample.event == GlassEvent::Transmission;
            let rgb = trace_center_pixel(&scene, seed);
            assert!((rgb - scene.background_color * sample.weight).norm() < 1e-12);
        }
        assert!(saw_reflection);
        if position[0] == 0.0 {
            assert!(saw_transmission);
        }
    }
    let mut scene = glass_background_scene();
    scene.add_with_material(Box::new(Ground::new(1.0).unwrap()), Box::new(glass));
    let reference = render_camera_glass(&scene, 32, 16, 1, 42).unwrap();
    assert_eq!(
        reference.pixels,
        render_camera_glass(&scene, 32, 16, 1, 42).unwrap().pixels
    );
    assert_ne!(
        reference.pixels,
        render_camera_glass(&scene, 32, 16, 1, 43).unwrap().pixels
    );
}

#[test]
fn glass_renderer_crosses_both_slab_interfaces_without_self_intersection() {
    struct Bottom(Ground);
    impl Hittable for Bottom {
        fn intersect(&self, ray: &Ray) -> Option<Hit> {
            self.0.intersect(ray).map(|mut hit| {
                hit.normal = -hit.normal;
                hit
            })
        }
    }
    let mut scene = glass_background_scene();
    scene.add_with_material(
        Box::new(Ground::new(1.5).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    scene.add_with_material(
        Box::new(Bottom(Ground::new(1.0).unwrap())),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    // 入射/出射的 eta² 权重互为倒数；内部反射后离开也应恢复背景。
    for seed in 0..64 {
        let rgb = trace_center_pixel(&scene, seed);
        assert!((rgb - scene.background_color).norm() < 1e-12);
    }
}

#[test]
fn glass_renderer_reaches_diffuse_light_and_retains_binary_shadows() {
    let mut scene = ground_scene();
    scene.add_with_material(
        Box::new(
            Triangle::new(
                Vector3::new(-0.5, -0.5, 1.0),
                Vector3::new(0.5, -0.5, 1.0),
                Vector3::new(0.0, 0.5, 1.0),
            )
            .unwrap(),
        ),
        Box::new(TestGlass(Some(transmission(-Vector3::z(), 0.25, 0.5)))),
    );
    // 斜向 shadow ray 绕过有限玻璃面；概率与余弦不能再乘到吞吐量上。
    scene.light.direction = Vector3::new(0.8, 0.0, -0.6);
    let rgb = trace_center_pixel(&scene, 42);
    let expected = Vector3::new(0.2, 0.5, 0.8) * (0.5 * 0.6 * 0.5 / PI);
    assert!((rgb - expected).norm() < 1e-12);
    // 向上的 shadow ray 被玻璃遮挡，仍按现有的二值硬阴影处理。
    scene.light.direction = -Vector3::z();
    assert_eq!(trace_center_pixel(&scene, 42), Vector3::zeros());
}

#[test]
fn glass_renderer_allows_eight_events_and_truncates_the_ninth_glass_hit() {
    for count in [8, 9] {
        let mut scene = glass_background_scene();
        scene.camera = Camera::from_config(&CameraConfig {
            position: [0.0, 0.0, 12.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 45.0,
        })
        .unwrap();
        for height in 1..=count {
            scene.add_with_material(
                Box::new(Ground::new(f64::from(height)).unwrap()),
                Box::new(Dielectric::new(1.0).unwrap()),
            );
        }
        let expected = if count == 8 {
            scene.background_color
        } else {
            Vector3::zeros()
        };
        let rgb = xyz_to_linear_srgb(render_camera_glass(&scene, 1, 1, 1, 42).unwrap().pixels[0])
            .unwrap();
        assert!((rgb - expected).norm() < 1e-12);
    }
}

#[test]
fn glass_renderer_handles_failed_samples_and_rejects_invalid_values() {
    for sample in [None, Some(transmission(-Vector3::z(), 1.0, 0.0))] {
        let mut scene = glass_background_scene();
        scene.add_with_material(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(TestGlass(sample)),
        );
        assert_eq!(
            render_camera_glass(&scene, 1, 1, 1, 42).unwrap().pixels,
            vec![Vector3::zeros()]
        );
    }
    let mut invalid = vec![MaterialSample::Continuous { wi: -Vector3::z() }];
    for value in [f64::NAN, f64::INFINITY, -1.0] {
        invalid.push(transmission(-Vector3::z(), 1.0, value));
        invalid.push(transmission(-Vector3::z(), value, 1.0));
    }
    for probability in [0.0, 1.1] {
        invalid.push(transmission(-Vector3::z(), probability, 1.0));
    }
    for wi in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
    ] {
        invalid.push(transmission(wi, 1.0, 1.0));
    }
    for sample in invalid {
        let mut scene = glass_background_scene();
        scene.add_with_material(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(TestGlass(Some(sample))),
        );
        assert!(render_camera_glass(&scene, 1, 1, 1, 42).is_err());
    }
    let mut scene = glass_background_scene();
    for height in [1.0, 1.5] {
        scene.add_with_material(
            Box::new(Ground::new(height).unwrap()),
            Box::new(TestGlass(Some(transmission(-Vector3::z(), 1.0, 1e308)))),
        );
    }
    assert!(render_camera_glass(&scene, 1, 1, 1, 42).is_err());
    for (width, height) in [
        (0, 1),
        (1, 0),
        (usize::MAX, 2),
        (u32::MAX as usize, u32::MAX as usize),
    ] {
        assert!(render_camera_glass(&scene, width, height, 1, 42).is_err());
    }
    scene.light.direction = Vector3::zeros();
    assert!(render_camera_glass(&scene, 1, 1, 1, 42).is_err());
}

#[test]
fn direct_lambertian_matches_albedo_light_and_one_cosine_without_clipping() {
    let mut scene = ground_scene();
    for (direction, cosine) in [
        (Vector3::new(0.0, 0.0, -5.0), 1.0),
        (Vector3::new(-3.0, 0.0, -4.0), 0.8),
        (Vector3::z(), 0.0),
        (Vector3::x(), 0.0),
    ] {
        scene.light.direction = direction;
        for flux in [0.0, 8.0, 64.0] {
            scene.light.total_flux = flux;
            let image = render_camera_lit(&scene, 3, 2).unwrap();
            assert_eq!((image.width, image.height), (3, 2));
            // 孔径为 4×4 米，Lambertian 为 albedo/π；只乘一次 cosθ。
            let expected = Vector3::new(0.2, 0.5, 0.8) * (flux / 16.0 * cosine / PI);
            for xyz in &image.pixels {
                assert!((xyz_to_linear_srgb(*xyz).unwrap() - expected).norm() < 1e-12);
            }
        }
    }
    // 保持总通量，孔径面积减半使当前相对方向光强度加倍。
    scene.light.direction = -Vector3::z();
    scene.light.total_flux = 8.0;
    scene.light.aperture_max_m.x = 0.0;
    let image = render_camera_lit(&scene, 1, 1).unwrap();
    let rgb = xyz_to_linear_srgb(*image.get(0, 0).unwrap()).unwrap();
    assert!((rgb - Vector3::new(0.2, 0.5, 0.8) / PI).norm() < 1e-12);
}

#[test]
fn shadow_ray_blocks_light_through_glass_without_hiding_the_ground_from_camera() {
    let mut config = SceneConfig {
        environment: None,
        ground_checkerboard: None,
        light: ParallelLightConfig {
            direction: [0.0, 0.0, -1.0],
            ..Default::default()
        },
        ..SceneConfig::default()
    };
    // 相机位于 3 cm 间隙内，只有向上的 shadow ray 命中玻璃。
    config.camera.position = [1.5, 0.0, 0.02];
    config.camera.target = [0.25, 0.0, 0.0];
    config.camera.up = [0.0, 1.0, 0.0];
    config.background_linear_rgb = [0.2, 0.3, 0.4];
    for reverse in [false, true] {
        if reverse {
            config.objects.reverse();
        }
        let scene = Scene::from_config(&config).unwrap();
        let ray = scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap();
        let hit = scene.intersect(&ray).unwrap();
        assert_eq!(hit.object_id, scene.ground_id);
        assert!((hit.coords - Vector3::new(0.25, 0.0, 0.0)).norm() < 1e-12);
        assert_eq!(
            render_camera_lit(&scene, 1, 1).unwrap().pixels,
            vec![Vector3::zeros()]
        );
    }
    // 同样的受光地面移出玻璃投影，恢复原有直接光照。
    config.camera.target = [1.5, 0.0, 0.0];
    let scene = Scene::from_config(&config).unwrap();
    let image = render_camera_lit(&scene, 1, 1).unwrap();
    let rgb = xyz_to_linear_srgb(*image.get(0, 0).unwrap()).unwrap();
    assert!((rgb - Vector3::repeat(0.4 / PI)).norm() < 1e-12);
}

#[test]
fn shadow_ray_ignores_surfaces_behind_it_and_checks_directional_light_to_infinity() {
    let mut scene = ground_scene();
    scene.camera = Camera::from_config(&CameraConfig {
        position: [3.0, 0.0, 2.0],
        target: [0.25, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 45.0,
    })
    .unwrap();
    let reference = render_camera_lit(&scene, 1, 1).unwrap();
    assert!(reference.pixels[0].norm() > 0.0);
    scene.add(Box::new(Ground::new(-1.0).unwrap()));
    assert_eq!(
        render_camera_lit(&scene, 1, 1).unwrap().pixels,
        reference.pixels
    );

    // 遮挡物在配置的发射高度以上，方向光仍需检测它；无材质也会遮光。
    let mut vertices = SceneConfig::default()
        .objects
        .into_iter()
        .find_map(|object| {
            if let ObjectConfig::Prism { vertices_m, .. } = object {
                Some(vertices_m.map(Vector3::from))
            } else {
                None
            }
        })
        .unwrap();
    for vertex in &mut vertices {
        vertex.z += 3.0;
    }
    scene.add(Box::new(Prism::from_vertices(vertices).unwrap()));
    assert_eq!(
        render_camera_lit(&scene, 1, 1).unwrap().pixels,
        vec![Vector3::zeros()]
    );
}

#[test]
fn camera_lighting_uses_nearest_material_and_linear_background() {
    let mut config = SceneConfig {
        environment: None,
        ground_checkerboard: None,
        light: ParallelLightConfig {
            direction: [0.0, 0.0, -1.0],
            ..Default::default()
        },
        ..SceneConfig::default()
    };
    config.camera.position = [0.0, -4.0, 2.0];
    config.camera.target = [0.0, 0.0, 1.0];
    config.camera.fov_y_degrees = 90.0;
    config.background_linear_rgb = [0.1, 0.2, 0.3];
    let scene = Scene::from_config(&config).unwrap();
    let image = render_camera_lit(&scene, 5, 3).unwrap();
    let rgb_at = |x, y| xyz_to_linear_srgb(*image.get(x, y).unwrap()).unwrap();
    assert!((rgb_at(2, 0) - Vector3::new(0.1, 0.2, 0.3)).norm() < 1e-12);
    assert_eq!(rgb_at(2, 1), Vector3::zeros()); // 最近命中理想玻璃，不能显示后方地面。
    assert!((rgb_at(2, 2) - Vector3::repeat(0.4 / PI)).norm() < 1e-12);
    config.objects.reverse();
    let scene = Scene::from_config(&config).unwrap();
    assert_eq!(
        render_camera_lit(&scene, 5, 3).unwrap().pixels,
        image.pixels
    );

    let mut scene = ground_scene();
    scene.background_color = Vector3::repeat(1.0);
    // 未绑定材质的最近表面保留黑色，而不是背景或后方材质。
    scene.add(Box::new(Ground::new(1.0).unwrap()));
    assert_eq!(
        render_camera_lit(&scene, 1, 1).unwrap().pixels,
        vec![Vector3::zeros()]
    );

    let mut scene = ground_scene();
    scene.camera = Camera::from_config(&CameraConfig {
        position: [0.0, 0.0, -1.0],
        target: [0.0; 3],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 45.0,
    })
    .unwrap();
    assert_eq!(
        render_camera_lit(&scene, 1, 1).unwrap().pixels,
        vec![Vector3::zeros()]
    );
}

#[test]
fn camera_lighting_rejects_invalid_dimensions_and_light() {
    let mut scene = ground_scene();
    for (width, height) in [
        (0, 1),
        (1, 0),
        (usize::MAX, 1),
        (u32::MAX as usize, u32::MAX as usize),
    ] {
        assert!(render_camera_lit(&scene, width, height).is_err());
    }
    for direction in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
        Vector3::repeat(f64::MAX),
    ] {
        scene.light.direction = direction;
        assert!(render_camera_lit(&scene, 1, 1).is_err());
    }
    scene.light.direction = -Vector3::z();
    for flux in [-1.0, f64::NAN, f64::INFINITY] {
        scene.light.total_flux = flux;
        assert!(render_camera_lit(&scene, 1, 1).is_err());
    }
    scene.light.total_flux = 8.0;
    scene.light.aperture_max_m = scene.light.aperture_min_m;
    assert!(render_camera_lit(&scene, 1, 1).is_err());
    scene.light.aperture_min_m.fill(0.0);
    scene.light.aperture_max_m.fill(1e-150);
    scene.light.total_flux = f64::MAX;
    assert!(render_camera_lit(&scene, 1, 1).is_err());
}

#[test]
fn camera_lighting_rejects_invalid_normals_and_bsdf_results() {
    struct TestSurface(Vector3<f64>);
    impl Hittable for TestSurface {
        fn intersect(&self, _ray: &Ray) -> Option<Hit> {
            Some(Hit {
                normal: self.0,
                distance: 0.5,
                ..Hit::default()
            })
        }
    }
    for normal in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::MAX),
    ] {
        let mut scene = ground_scene();
        scene.add_with_material(
            Box::new(TestSurface(normal)),
            Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
        );
        assert!(render_camera_lit(&scene, 1, 1).is_err());
    }
    struct TestMaterial(BsdfValue);
    impl Material for TestMaterial {
        fn evaluate(
            &self,
            _wi: Vector3<f64>,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
            _mode: EvaluateMode,
        ) -> BsdfValue {
            self.0
        }
    }
    for value in [
        BsdfValue::Rgb(Vector3::repeat(f64::NAN)),
        BsdfValue::Spectral(0.5),
    ] {
        let mut scene = ground_scene();
        scene.add_with_material(
            Box::new(TestSurface(Vector3::z() * 3.0)),
            Box::new(TestMaterial(value)),
        );
        assert!(render_camera_lit(&scene, 1, 1).is_err());
    }
}

#[test]
fn shared_ray_offset_preserves_original_bits_and_caller_validation() {
    use crate::geometry::ray::INTERSECTION_EPSILON;
    for position in [
        Vector3::zeros(),
        Vector3::repeat(-0.0),
        Vector3::new(1.0, -2.0, 3.0),
        Vector3::repeat(f64::MAX),
    ] {
        for normal in [
            Vector3::z(),
            -Vector3::z(),
            Vector3::new(1.0, 2.0, 3.0).normalize(),
        ] {
            for side in [-1.0, 1.0] {
                let expected = position + normal * (side * 10.0 * INTERSECTION_EPSILON);
                let actual = offset_ray_origin(position, normal, side);
                assert_eq!(actual.map(f64::to_bits), expected.map(f64::to_bits));
            }
            let old_shadow_origin = position + normal * (10.0 * INTERSECTION_EPSILON);
            assert_eq!(
                offset_ray_origin(position, normal, 1.0).map(f64::to_bits),
                old_shadow_origin.map(f64::to_bits)
            );
        }
    }
    // Invalid input is still rejected by Ray::new at the original call sites.
    assert!(
        Ray::new(
            offset_ray_origin(Vector3::repeat(f64::NAN), Vector3::z(), 1.0),
            Vector3::z()
        )
        .is_none()
    );
    assert!(
        Ray::new(
            offset_ray_origin(Vector3::zeros(), Vector3::repeat(f64::INFINITY), -1.0),
            Vector3::z()
        )
        .is_none()
    );
    assert!(
        Ray::new(
            offset_ray_origin(Vector3::zeros(), Vector3::z(), 1.0),
            Vector3::zeros()
        )
        .is_none()
    );
}

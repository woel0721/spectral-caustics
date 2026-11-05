use crate::geometry::hit::GeometryHit;
// 相机积分器的原有单元测试。
use super::lighting::camera_lighting_setup;
use super::lighting::environment::environment_lighting;
use super::test_support::evaluate_path;
use super::types::CameraPathRngs;
use super::*;
use crate::camera::Camera;
use crate::caustics::GroundReceiver;
use crate::caustics::SpectralLighting;
use crate::color::linear_srgb_to_xyz;
use crate::color::xyz_to_linear_srgb;
use crate::config::RenderJob;
use crate::config::camera::CameraConfig;
use crate::config::light::ParallelLightConfig;
use crate::config::material::MaterialConfig;
use crate::config::scene::EnvironmentConfig;
use crate::config::scene::ObjectConfig;
use crate::config::scene::SceneConfig;
use crate::film::Framebuffer;
use crate::geometry::ground::Ground;
use crate::geometry::hit::Hit;
use crate::geometry::object::Hittable;
use crate::geometry::prism::Prism;
use crate::geometry::ray::Ray;
use crate::geometry::triangle::Triangle;
use crate::material::BsdfSample;
use crate::material::Material;
use crate::material::ScatterType;
use crate::material::dielectric::Dielectric;
use crate::material::lambertian::Lambertian;
use crate::sampling::rng::CAMERA_STREAM;
use crate::sampling::rng::DIFFUSE_STREAM;
use crate::sampling::rng::ENVIRONMENT_STREAM;
use crate::sampling::rng::pixel_seed;
use nalgebra::Vector2;
use nalgebra::Vector3;
use rand::SeedableRng;
use rand::rngs::StdRng;
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

fn ground_scene() -> RenderJob {
    RenderJob::from_config(&SceneConfig {
        environment: None, // 隔离历史太阳/阴影测试的输入。
        ground_checkerboard: None,
        light: ParallelLightConfig {
            direction: [0.0, 0.0, -1.0],
            ..Default::default()
        }
        .into(),
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

fn neutral_ground_scene(rho: f64) -> RenderJob {
    RenderJob::from_config(&SceneConfig {
        environment: None,
        ground_checkerboard: None,
        light: ParallelLightConfig {
            direction: [0.0, 0.0, -1.0],
            ..Default::default()
        }
        .into(),
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
    let mut map = Framebuffer::new(1, 1).unwrap();
    assert!(map.set(0, 0, xyz));
    map
}

fn estimate_environment(
    scene: &RenderJob,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    seed: u64,
) -> Vector3<f64> {
    environment_lighting(
        scene,
        hit,
        material,
        wo,
        &mut StdRng::seed_from_u64(seed),
        crate::integrator::camera::transport::Rgb::default(),
        crate::optics::dispersion::IorMode::Constant,
    )
    .unwrap()
}

#[test]
fn constant_environment_recovers_albedo_for_every_surface_orientation_and_sample_count() {
    let mut scene = RenderJob::new();
    let sky = Vector3::new(0.1, 0.2, 4.0);
    scene.scene.environment = Some(
        crate::environment::Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: sky.into(),
            zenith_linear_rgb: sky.into(),
            ..Default::default()
        })
        .unwrap(),
    );
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let diffuse = Lambertian::new(albedo).unwrap();
    for samples in [1, 16, 1024] {
        scene.camera_options.environment_samples = samples;
        for normal in [
            Vector3::z(),
            Vector3::x(),
            -Vector3::z(),
            Vector3::new(0.6, 0.0, 0.8),
        ] {
            let hit = Hit {
                normal: normal * 3.0,
                coords: nalgebra::Vector3::zeros(),
                distance: 1.0,
                object_id: 0,
                material_id: 0,
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
    let environment = scene.scene.environment;
    let mut scene = ground_scene();
    scene.scene.environment = environment;
    scene.scene.light.as_parallel_mut().unwrap().total_flux = 0.0;
    for samples in [1, 8] {
        scene.camera_options.environment_samples = samples;
        for spp in [1, 8] {
            let image = crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 3,
                    height: 2,
                    camera_spp: spp,
                    seed: 42,
                    ..Default::default()
                },
                None,
            )
            .map(|images| images.combined)
            .unwrap();
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
    let mut scene = RenderJob::new(); // 空场景，所有环境方向可见。
    let sky = scene.scene.environment.as_ref().unwrap();
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let material = Lambertian::new(albedo).unwrap();
    let hit = Hit {
        normal: Vector3::z(),
        coords: nalgebra::Vector3::zeros(),
        distance: 1.0,
        object_id: 0,
        material_id: 0,
    };
    // 余弦分布下 E[wi.z]=2/3；直接积分渐变可得到独立解析参考值。
    let expected =
        albedo.component_mul(&(sky.horizon_color() / 3.0 + sky.zenith_color() * (2.0 / 3.0)));
    let mut errors = [0.0; 2];
    for (index, samples) in [1, 256].into_iter().enumerate() {
        scene.camera_options.environment_samples = samples;
        for seed in 0..64 {
            let estimate = estimate_environment(&scene, &hit, &material, Vector3::z(), seed);
            errors[index] += (estimate - expected).norm_squared() / 64.0;
        }
    }
    assert!(errors[1] < errors[0] / 32.0, "MSE: {errors:?}");
    scene.camera_options.environment_samples = 65536;
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
        fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
            let hit = self.0.intersect(ray)?;
            (hit.coords.y > 0.0).then_some(hit)
        }
    }
    let mut scene = RenderJob::new();
    let li = Vector3::new(0.25, 0.5, 1.0);
    scene.scene.environment = Some(
        crate::environment::Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: li.into(),
            zenith_linear_rgb: li.into(),
            ..Default::default()
        })
        .unwrap(),
    );
    scene.camera_options.environment_samples = 32768;
    scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    ); // 起点偏移必须避免地面自遮挡。
    scene.scene.add(
        Box::new(HalfPlane(Ground::new(1.0).unwrap())),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let material = Lambertian::new(albedo).unwrap();
    let hit = Hit {
        normal: Vector3::z(),
        coords: nalgebra::Vector3::zeros(),
        distance: 1.0,
        object_id: 0,
        material_id: 0,
    };
    let actual = estimate_environment(&scene, &hit, &material, Vector3::z(), 42);
    let expected = albedo.component_mul(&li) * 0.5;
    assert!((actual - expected).norm() < 0.01);
    scene.scene.add(
        Box::new(Ground::new(2.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    assert_eq!(
        estimate_environment(&scene, &hit, &material, Vector3::z(), 42),
        Vector3::zeros()
    );
}

#[test]
fn opaque_visibility_blocks_sky_below_a_glass_plane() {
    let mut scene = neutral_ground_scene(0.8);
    scene.scene.environment = RenderJob::new().scene.environment;
    scene.scene.add(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(TestGlass(Some(transmission(-Vector3::z(), 0.5)))),
    );
    let lighting = split_ground_map(&scene);
    let images = crate::integrator::camera::integrate_rgb(
        &scene,
        &crate::config::render::RenderSetting {
            width: 3,
            height: 2,
            camera_spp: 8,
            seed: 42,
            ..Default::default()
        },
        Some(&lighting),
    )
    .unwrap();
    let (direct, combined) = (images.direct, images.combined);
    assert_eq!(direct.pixels, combined.pixels);
    let expected = Vector3::zeros(); // 向上的环境射线全部被无限玻璃面遮挡。
    for xyz in direct.pixels {
        assert!((xyz_to_linear_srgb(xyz).unwrap() - expected).norm() < 1e-12);
    }
    assert!(
        images
            .caustics
            .pixels
            .iter()
            .all(|p| *p == Vector3::zeros())
    );
}

#[test]
fn glass_misses_use_the_final_direction_for_sky_gradient() {
    let mut scene = glass_background_scene();
    scene.scene.environment = RenderJob::new().scene.environment;
    let sky = scene.scene.environment.as_ref().unwrap();
    for (direction, weight, expected) in [
        (Vector3::z(), 0.5, sky.zenith_color()),
        (-Vector3::z(), 0.25, sky.horizon_color()),
    ] {
        scene.scene.objects.clear();
        scene.scene.add(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(TestGlass(Some(transmission(direction, weight)))),
        );
        assert!((trace_center_pixel(&scene, 42) - expected * weight).norm() < 1e-12);
    }
}

#[test]
fn checkerboard_is_applied_once_after_glass_with_sky_and_caustics() {
    let mut scene = neutral_ground_scene(0.8);
    scene.scene.environment = RenderJob::new().scene.environment;
    scene.ground_checkerboard = RenderJob::new().ground_checkerboard;
    scene.camera = Camera::from_config(&CameraConfig {
        position: [0.75, 0.25, 2.0],
        target: [0.75, 0.25, 0.0],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 0.001,
    })
    .unwrap();
    scene.scene.add(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(TestGlass(Some(transmission(-Vector3::z(), 0.5)))),
    );
    let mut lighting = split_ground_map(&scene);
    let e = Vector3::new(0.1, 0.2, 0.4);
    lighting.caustics.pixels.fill(e);
    let expected_sky = Vector3::zeros(); // 二值可见性：上方玻璃面遮挡所有环境方向。
    let expected_caustics = e * (0.8 / PI * 0.25 * 0.5);
    let images = crate::integrator::camera::integrate_rgb(
        &scene,
        &crate::config::render::RenderSetting {
            width: 1,
            height: 1,
            camera_spp: 8,
            seed: 42,
            ..Default::default()
        },
        Some(&lighting),
    )
    .unwrap();
    assert!((images.direct.pixels[0] - expected_sky).norm() < 1e-12);
    assert!((images.caustics.pixels[0] - expected_caustics).norm() < 1e-12);
    assert!((images.combined.pixels[0] - expected_sky - expected_caustics).norm() < 1e-12);
}

#[test]
fn camera_rejects_invalid_runtime_sky_parameters() {
    let mut scene = ground_scene();
    for samples in [0, usize::MAX] {
        scene.camera_options.environment_samples = samples;
        assert!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 2,
                    height: 1,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .is_err()
        );
        assert!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 1,
                    height: 1,
                    camera_spp: 2,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .is_err()
        );
    }
    scene.camera_options.environment_samples = 1;
    for channel in 0..2 {
        for value in [-1.0, f64::NAN, f64::INFINITY] {
            scene.scene.environment = RenderJob::new().scene.environment;
            let sky = scene.scene.environment.as_mut().unwrap();
            let (mut horizon, mut zenith) = (sky.horizon_color(), sky.zenith_color());
            if channel == 0 {
                horizon.x = value;
            } else {
                zenith.x = value;
            }
            assert!(sky.set_gradient(horizon, zenith).is_err());
        }
    }
    let mut scene = neutral_ground_scene(1.0);
    scene.scene.environment = RenderJob::new().scene.environment;
    {
        let env = scene.scene.environment.as_mut().unwrap();
        env.set_gradient(Vector3::repeat(f64::MAX), env.zenith_color())
            .unwrap();
    }
    {
        let env = scene.scene.environment.as_mut().unwrap();
        env.set_gradient(env.horizon_color(), Vector3::repeat(f64::MAX))
            .unwrap();
    }
    // 有限 HDR 参数也可能在颜色空间变换中溢出，必须返回错误。
    assert!(
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 1,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .is_err()
    );
}

fn split_ground_map(scene: &RenderJob) -> SpectralLighting {
    crate::integrator::photon::integrate_xyz(
        scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.unwrap(),
        &crate::config::render::RenderSetting {
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
    use std::sync::Arc;
    use std::sync::Mutex;

    type Event = (Vector3<f64>, Vector3<f64>, ScatterType, Vector3<f64>);
    struct RecordingGlass {
        glass: Dielectric,
        events: Arc<Mutex<Vec<Event>>>,
    }
    impl Material for RecordingGlass {
        fn evaluate_rgb(
            &self,
            wi: Vector3<f64>,
            wo: Vector3<f64>,
            normal: Vector3<f64>,
        ) -> Vector3<f64> {
            self.glass.evaluate_rgb(wi, wo, normal)
        }
        fn sample(
            &self,
            wo: Vector3<f64>,
            normal: Vector3<f64>,
            rng: &mut dyn rand::RngCore,
        ) -> Option<BsdfSample> {
            let sample = self.glass.sample(wo, normal, rng)?;
            let glass = sample;
            self.events
                .lock()
                .unwrap()
                .push((wo, glass.wi, glass.event, glass.weight));
            Some(sample)
        }
        fn has_non_delta(&self) -> bool {
            false
        }
        fn has_delta(&self) -> bool {
            true
        }
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut scene = neutral_ground_scene(0.8);
    scene.scene.environment = RenderJob::new().scene.environment;
    scene.camera_options.environment_samples = 3;
    scene.scene.add(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(RecordingGlass {
            glass: Dielectric::new(1.5).unwrap(),
            events: events.clone(),
        }),
    );
    let mut lighting = split_ground_map(&scene);
    lighting.caustics.pixels.fill(Vector3::new(0.1, 0.2, 0.4));
    let (width, height, spp, seed) = (4, 3, 32, 42);
    let images = crate::integrator::camera::integrate_rgb(
        &scene,
        &crate::config::render::RenderSetting {
            width,
            height,
            camera_spp: spp,
            seed,
            ..Default::default()
        },
        Some(&lighting),
    )
    .unwrap();
    let reference_events = events.lock().unwrap().clone();
    assert_eq!(reference_events.len(), width * height * spp);
    assert!(
        reference_events
            .iter()
            .any(|e| e.2 == ScatterType::SpecularReflection)
    );
    assert!(
        reference_events
            .iter()
            .any(|e| e.2 == ScatterType::SpecularTransmission)
    );
    for environment_samples in [1, 7] {
        scene.camera_options.environment_samples = environment_samples;
        events.lock().unwrap().clear();
        let changed = crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width,
                height,
                camera_spp: spp,
                seed,
                ..Default::default()
            },
            Some(&lighting),
        )
        .unwrap();
        assert_eq!(*events.lock().unwrap(), reference_events);
        assert_eq!(changed.caustics.pixels, images.caustics.pixels);
    }
    scene.scene.environment = None;
    events.lock().unwrap().clear();
    let without_environment = crate::integrator::camera::integrate_rgb(
        &scene,
        &crate::config::render::RenderSetting {
            width,
            height,
            camera_spp: spp,
            seed,
            ..Default::default()
        },
        Some(&lighting),
    )
    .unwrap();
    assert_eq!(*events.lock().unwrap(), reference_events);
    assert_eq!(without_environment.caustics.pixels, images.caustics.pixels);
}

#[test]
fn non_ground_lambertian_sun_and_environment_only_enter_direct() {
    let mut scene = neutral_ground_scene(0.8);
    let sky = Vector3::new(0.1, 0.2, 0.4);
    scene.scene.environment = Some(
        crate::environment::Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: sky.into(),
            zenith_linear_rgb: sky.into(),
            ..Default::default()
        })
        .unwrap(),
    );
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    scene.scene.add(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(Lambertian::new(albedo).unwrap()),
    );
    let mut lighting = split_ground_map(&scene);
    lighting.caustics.pixels.fill(Vector3::repeat(100.0));
    let expected_rgb = albedo
        .component_mul(&(xyz_to_linear_srgb(lighting.sun_irradiance_xyz).unwrap() / PI + sky));
    let expected_xyz = linear_srgb_to_xyz(expected_rgb).unwrap();
    let images = crate::integrator::camera::integrate_rgb(
        &scene,
        &crate::config::render::RenderSetting {
            width: 3,
            height: 2,
            camera_spp: 8,
            seed: 42,
            ..Default::default()
        },
        Some(&lighting),
    )
    .unwrap();
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
    scene.photon.receiver.min_m = Vector2::repeat(-0.1);
    scene.photon.receiver.max_m = Vector2::repeat(0.1);
    // 斜射光同样以水平窗口 Phi/A 为水平地面辐照度。
    scene.scene.light.as_parallel_mut().unwrap().direction = Vector3::new(-3.0, 0.0, -4.0);
    let mut lighting = split_ground_map(&scene);
    assert!(
        lighting
            .caustics
            .pixels
            .iter()
            .all(|p| *p == Vector3::zeros())
    );
    // sampled direct 图绝不能被相机当作第二份直射累加。
    lighting
        .direct
        .as_mut()
        .unwrap()
        .pixels
        .fill(Vector3::repeat(100.0));
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
            let images = crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 3,
                    height: 2,
                    camera_spp: spp,
                    seed: 42,
                    ..Default::default()
                },
                Some(&lighting),
            )
            .unwrap();
            assert_eq!(images.combined.pixels, images.direct.pixels);
            assert!(
                images
                    .caustics
                    .pixels
                    .iter()
                    .all(|p| *p == Vector3::zeros())
            );
            for xyz in images.direct.pixels {
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
        scene.photon.receiver.min_m = Vector2::repeat(min);
        scene.photon.receiver.max_m = Vector2::repeat(max);
        let lighting = split_ground_map(&scene);
        let image = crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 5,
                height: 3,
                camera_spp: 8,
                seed: 42,
                ..Default::default()
            },
            Some(&lighting),
        )
        .map(|images| images.combined)
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
    scene.scene.add(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(TestGlass(Some(transmission(-Vector3::z(), 0.5)))),
    );
    let mut lighting = split_ground_map(&scene);
    // 给地面一个已知的焦散 E；无限玻璃面会遮挡每一条向上的 shadow ray。
    let e = Vector3::new(0.1, 0.2, 1.0);
    lighting.caustics.pixels.fill(e);
    let images = crate::integrator::camera::integrate_rgb(
        &scene,
        &crate::config::render::RenderSetting {
            width: 3,
            height: 2,
            camera_spp: 8,
            seed: 42,
            ..Default::default()
        },
        Some(&lighting),
    )
    .unwrap();
    let (direct, caustics, combined) = (images.direct, images.caustics, images.combined);
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
    let render = |s: &RenderJob, l: &SpectralLighting| {
        crate::integrator::camera::integrate_rgb(
            s,
            &crate::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 1,
                seed: 42,
                ..Default::default()
            },
            Some(l),
        )
        .map(|images| images.combined)
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
        assert!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width,
                    height,
                    camera_spp: spp,
                    seed: 42,
                    ..Default::default()
                },
                Some(&lighting)
            )
            .is_err()
        );
    }
}

#[test]
fn ground_irradiance_sampling_preserves_world_axes_centers_and_bilinear_weights() {
    let receiver = GroundReceiver {
        min_m: Vector2::new(-2.0, -1.0),
        max_m: Vector2::new(2.0, 1.0),
    };
    let mut map = Framebuffer::new(2, 2).unwrap();
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
        assert!(
            sample_ground_irradiance(&map, &scene.photon.receiver, Vector3::repeat(value))
                .is_none()
        );
    }
    for value in [-1.0, f64::NAN, f64::INFINITY] {
        map.pixels[0].x = value;
        assert!(sample_ground_irradiance(&map, &scene.photon.receiver, Vector3::zeros()).is_none());
    }
    map.pixels[0].x = 1.0;
    scene.photon.receiver.max_m.x = scene.photon.receiver.min_m.x;
    assert!(sample_ground_irradiance(&map, &scene.photon.receiver, Vector3::zeros()).is_none());
    scene.photon.receiver.max_m.x = f64::INFINITY;
    assert!(sample_ground_irradiance(&map, &scene.photon.receiver, Vector3::zeros()).is_none());
    let receiver = neutral_ground_scene(0.8).photon.receiver;
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

fn glass_background_scene() -> RenderJob {
    let mut scene = RenderJob::new();
    scene.scene.environment = None;
    scene.camera = Camera::from_config(&CameraConfig {
        position: [0.0, 0.0, 3.0],
        target: [0.0; 3],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 45.0,
    })
    .unwrap();
    scene.scene.background_color = Vector3::new(0.2, 0.4, 0.8);
    scene
}

struct TestGlass(Option<BsdfSample>);

impl Material for TestGlass {
    fn evaluate_rgb(
        &self,
        _wi: Vector3<f64>,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
    ) -> Vector3<f64> {
        panic!("玻璃路径应使用采样记录，不能直接求普通 BSDF");
    }

    fn sample(
        &self,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
        _rng: &mut dyn rand::RngCore,
    ) -> Option<BsdfSample> {
        self.0
    }

    fn has_non_delta(&self) -> bool {
        false
    }
    fn has_delta(&self) -> bool {
        true
    }
}

fn transmission(wi: Vector3<f64>, weight: f64) -> BsdfSample {
    BsdfSample {
        wi,
        event: ScatterType::SpecularTransmission,
        pdf: None,
        weight: Vector3::repeat(weight),
    }
}

fn trace_center_pixel(scene: &RenderJob, seed: u64) -> Vector3<f64> {
    let (light_wi, light) = camera_lighting_setup(scene, 1, 1).unwrap();
    let ray = scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap();
    evaluate_path(
        scene,
        ray,
        &mut CameraPathRngs {
            camera: &mut StdRng::seed_from_u64(seed),
            environment: &mut StdRng::seed_from_u64(pixel_seed(seed, 0, ENVIRONMENT_STREAM)),
            diffuse: &mut StdRng::seed_from_u64(seed ^ DIFFUSE_STREAM),
        },
        light_wi,
        light,
        None,
    )
    .unwrap()
    .direct
}

#[test]
fn camera_spp_averages_independent_paths_and_is_reproducible() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    struct AlternatingGlass(Arc<AtomicUsize>);
    impl Material for AlternatingGlass {
        fn evaluate_rgb(
            &self,
            _wi: Vector3<f64>,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
        ) -> Vector3<f64> {
            Vector3::zeros()
        }
        fn evaluate_spectral(
            &self,
            _wi: Vector3<f64>,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
            _wavelength_nm: f64,
        ) -> f64 {
            0.0
        }
        fn sample(
            &self,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
            _rng: &mut dyn rand::RngCore,
        ) -> Option<BsdfSample> {
            let index = self.0.fetch_add(1, Ordering::Relaxed);
            Some(BsdfSample {
                wi: Vector3::z(),
                event: ScatterType::SpecularReflection,
                pdf: None,
                weight: Vector3::repeat(if index & 1 == 0 { 0.25 } else { 1.75 }),
            })
        }
        fn has_non_delta(&self) -> bool {
            false
        }
        fn has_delta(&self) -> bool {
            true
        }
    }
    for (spp, expected_weight) in [(1, 0.25), (3, 0.75), (4, 1.0), (64, 1.0)] {
        let count = Arc::new(AtomicUsize::new(0));
        let mut scene = glass_background_scene();
        scene.scene.add(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(AlternatingGlass(count.clone())),
        );
        let image = crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: spp,
                seed: 42,
                ..Default::default()
            },
            None,
        )
        .map(|images| images.combined)
        .unwrap();
        let rgb = xyz_to_linear_srgb(image.pixels[0]).unwrap();
        assert!((rgb - scene.scene.background_color * expected_weight).norm() < 1e-12);
        assert_eq!(count.load(Ordering::Relaxed), spp);
    }
    let mut scene = glass_background_scene();
    scene.scene.add(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let reference = crate::integrator::camera::integrate_rgb(
        &scene,
        &crate::config::render::RenderSetting {
            width: 16,
            height: 8,
            camera_spp: 64,
            seed: 42,
            ..Default::default()
        },
        None,
    )
    .map(|images| images.combined)
    .unwrap();
    assert_eq!(
        reference.pixels,
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 16,
                height: 8,
                camera_spp: 64,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .unwrap()
        .pixels
    );
    assert_ne!(
        reference.pixels,
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 16,
                height: 8,
                camera_spp: 64,
                seed: 43,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .unwrap()
        .pixels
    );
}

#[test]
fn jittered_camera_filters_half_pixel_coverage_in_linear_light() {
    struct LeftHalfPlane;
    impl Hittable for LeftHalfPlane {
        fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
            let hit = Ground::new(0.0).unwrap().intersect(ray)?;
            (hit.coords.x < 0.0).then_some(hit)
        }
    }
    let mut scene = glass_background_scene();
    scene.scene.background_color = Vector3::repeat(1.0);
    // 左半像素命中无材质黑色表面，右半像素看到白色背景，面积均值应为线性 0.5。
    scene.scene.add(
        Box::new(LeftHalfPlane),
        Box::new(crate::material::lambertian::Lambertian::new(nalgebra::Vector3::zeros()).unwrap()),
    );
    let image = crate::integrator::camera::integrate_rgb(
        &scene,
        &crate::config::render::RenderSetting {
            width: 1,
            height: 1,
            camera_spp: 4096,
            seed: 42,
            ..Default::default()
        },
        None,
    )
    .map(|images| images.combined)
    .unwrap();
    let rgb = xyz_to_linear_srgb(image.pixels[0]).unwrap();
    assert!((rgb - Vector3::repeat(0.5)).norm() < 0.03);
    assert_eq!(
        image.pixels,
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 4096,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .unwrap()
        .pixels
    );
}

#[test]
fn camera_spp_preserves_deterministic_brightness_and_rejects_invalid_counts() {
    let scene = ground_scene();
    let white =
        xyz_to_linear_srgb(Vector3::new(0.9501416822353593, 1.0, 1.087976622255359)).unwrap();
    let expected =
        linear_srgb_to_xyz(Vector3::new(0.2, 0.5, 0.8).component_mul(&white) * (0.5 / PI)).unwrap();
    for spp in [1, 3, 64] {
        let image = crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 3,
                height: 2,
                camera_spp: spp,
                seed: 42,
                ..Default::default()
            },
            None,
        )
        .map(|images| images.combined)
        .unwrap();
        for actual in &image.pixels {
            assert!((actual - expected).norm() < 1e-12);
        }
    }
    assert!(
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 0,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .is_err()
    );
    assert!(
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 2,
                height: 1,
                camera_spp: usize::MAX,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .is_err()
    );
}

#[test]
fn camera_spp_reduces_error_against_analytic_fresnel_expectation() {
    use crate::optics::interface::fresnel_dielectric;
    let mut scene = glass_background_scene();
    scene.scene.add(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let error = |spp| {
        let image = crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 32,
                height: 16,
                camera_spp: spp,
                seed: 42,
                ..Default::default()
            },
            None,
        )
        .map(|images| images.combined)
        .unwrap();
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
                let expected = scene.scene.background_color * (f + (1.0 - f) * (4.0 / 9.0));
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
        scene
            .scene
            .add(Box::new(Ground::new(1.0).unwrap()), Box::new(glass.clone()));
        let mut saw_reflection = false;
        let mut saw_transmission = false;
        for seed in 0..64 {
            let wo = -scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap().direction;
            let sample = glass
                .sample(wo, Vector3::z(), &mut StdRng::seed_from_u64(seed))
                .unwrap();
            saw_reflection |= sample.event == ScatterType::SpecularReflection;
            saw_transmission |= sample.event == ScatterType::SpecularTransmission;
            let rgb = trace_center_pixel(&scene, seed);
            assert!(
                (rgb - scene.scene.background_color.component_mul(&sample.weight)).norm() < 1e-12
            );
        }
        assert!(saw_reflection);
        if position[0] == 0.0 {
            assert!(saw_transmission);
        }
    }
    let mut scene = glass_background_scene();
    scene
        .scene
        .add(Box::new(Ground::new(1.0).unwrap()), Box::new(glass));
    let reference = crate::integrator::camera::integrate_rgb(
        &scene,
        &crate::config::render::RenderSetting {
            width: 32,
            height: 16,
            camera_spp: 1,
            seed: 42,
            ..Default::default()
        },
        None,
    )
    .map(|images| images.combined)
    .unwrap();
    assert_eq!(
        reference.pixels,
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 32,
                height: 16,
                camera_spp: 1,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .unwrap()
        .pixels
    );
    assert_ne!(
        reference.pixels,
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 32,
                height: 16,
                camera_spp: 1,
                seed: 43,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .unwrap()
        .pixels
    );
}

#[test]
fn glass_renderer_crosses_both_slab_interfaces_without_self_intersection() {
    struct Bottom(Ground);
    impl Hittable for Bottom {
        fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
            self.0.intersect(ray).map(|mut hit| {
                hit.normal = -hit.normal;
                hit
            })
        }
    }
    let mut scene = glass_background_scene();
    scene.scene.add(
        Box::new(Ground::new(1.5).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    scene.scene.add(
        Box::new(Bottom(Ground::new(1.0).unwrap())),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    // 入射/出射的 eta² 权重互为倒数；内部反射后离开也应恢复背景。
    for seed in 0..64 {
        let rgb = trace_center_pixel(&scene, seed);
        assert!((rgb - scene.scene.background_color).norm() < 1e-12);
    }
}

#[test]
fn glass_renderer_reaches_diffuse_light_and_retains_binary_shadows() {
    let mut scene = ground_scene();
    scene.scene.add(
        Box::new(
            Triangle::new(
                Vector3::new(-0.5, -0.5, 1.0),
                Vector3::new(0.5, -0.5, 1.0),
                Vector3::new(0.0, 0.5, 1.0),
            )
            .unwrap(),
        ),
        Box::new(TestGlass(Some(transmission(-Vector3::z(), 0.5)))),
    );
    // 斜向 shadow ray 绕过有限玻璃面；概率与余弦不能再乘到吞吐量上。
    scene.scene.light.as_parallel_mut().unwrap().direction = Vector3::new(0.8, 0.0, -0.6);
    let rgb = trace_center_pixel(&scene, 42);
    let expected = Vector3::new(0.2, 0.5, 0.8) * (0.5 * 0.5 / PI);
    assert!((rgb - expected).norm() < 1e-12);
    // 向上的 shadow ray 被玻璃遮挡，仍按现有的二值硬阴影处理。
    scene.scene.light.as_parallel_mut().unwrap().direction = -Vector3::z();
    assert_eq!(trace_center_pixel(&scene, 42), Vector3::zeros());
}

#[test]
fn glass_renderer_respects_the_delta_event_budget() {
    for count in [
        super::path::MAX_DELTA_EVENTS,
        super::path::MAX_DELTA_EVENTS + 1,
    ] {
        let mut scene = glass_background_scene();
        scene.camera = Camera::from_config(&CameraConfig {
            position: [0.0, 0.0, super::path::MAX_DELTA_EVENTS as f64 + 4.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 45.0,
        })
        .unwrap();
        for height in 1..=count {
            scene.scene.add(
                Box::new(Ground::new(height as f64).unwrap()),
                Box::new(Dielectric::new(1.0).unwrap()),
            );
        }
        let expected = if count == crate::integrator::camera::path::MAX_DELTA_EVENTS {
            scene.scene.background_color
        } else {
            Vector3::zeros()
        };
        let rgb = xyz_to_linear_srgb(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 1,
                    height: 1,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None,
            )
            .map(|images| images.combined)
            .unwrap()
            .pixels[0],
        )
        .unwrap();
        assert!((rgb - expected).norm() < 1e-12);
    }
}

#[test]
fn glass_renderer_handles_failed_samples_and_rejects_invalid_values() {
    for sample in [None, Some(transmission(-Vector3::z(), 0.0))] {
        let mut scene = glass_background_scene();
        scene.scene.add(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(TestGlass(sample)),
        );
        assert_eq!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 1,
                    height: 1,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .unwrap()
            .pixels,
            vec![Vector3::zeros()]
        );
    }
    let mut invalid = vec![BsdfSample {
        wi: -Vector3::z(),
        weight: Vector3::repeat(0.8),
        pdf: Some(1.0 / PI),
        event: crate::material::ScatterType::DiffuseReflection,
    }];
    for value in [f64::NAN, f64::INFINITY, -1.0] {
        invalid.push(transmission(-Vector3::z(), value));
    }
    let mut wrong_pdf = transmission(-Vector3::z(), 1.0);
    wrong_pdf.pdf = Some(1.0);
    invalid.push(wrong_pdf);
    for wi in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
    ] {
        invalid.push(transmission(wi, 1.0));
    }
    for sample in invalid {
        let mut scene = glass_background_scene();
        scene.scene.add(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(TestGlass(Some(sample))),
        );
        assert!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 1,
                    height: 1,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .is_err()
        );
    }
    let mut scene = glass_background_scene();
    for height in [1.0, 1.5] {
        scene.scene.add(
            Box::new(Ground::new(height).unwrap()),
            Box::new(TestGlass(Some(transmission(-Vector3::z(), 1e308)))),
        );
    }
    assert!(
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 1,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .is_err()
    );
    for (width, height) in [
        (0, 1),
        (1, 0),
        (usize::MAX, 2),
        (u32::MAX as usize, u32::MAX as usize),
    ] {
        assert!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width,
                    height,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .is_err()
        );
    }
    scene.scene.light.as_parallel_mut().unwrap().direction = Vector3::zeros();
    assert!(
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 1,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .is_err()
    );
}

#[test]
fn direct_lambertian_matches_albedo_light_and_one_cosine_without_clipping() {
    let mut scene = ground_scene();
    let white =
        xyz_to_linear_srgb(Vector3::new(0.9501416822353593, 1.0, 1.087976622255359)).unwrap();
    for (direction, cosine) in [
        (Vector3::new(0.0, 0.0, -5.0), 1.0),
        (Vector3::new(-3.0, 0.0, -4.0), 0.8),
    ] {
        scene.scene.light.as_parallel_mut().unwrap().direction = direction;
        for flux in [0.0, 8.0, 64.0] {
            scene.scene.light.as_parallel_mut().unwrap().total_flux = flux;
            let image = crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 3,
                    height: 2,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None,
            )
            .map(|images| images.combined)
            .unwrap();
            assert_eq!((image.width, image.height), (3, 2));
            // 孔径为 4×4 米，Lambertian 为 albedo/π；只乘一次 cosθ。
            let expected = Vector3::new(0.2, 0.5, 0.8).component_mul(&white)
                * (flux / (16.0 * cosine) * cosine / PI);
            for xyz in &image.pixels {
                assert!((xyz_to_linear_srgb(*xyz).unwrap() - expected).norm() < 1e-12);
            }
        }
    }
    // 保持总通量，孔径面积减半使当前相对方向光强度加倍。
    scene.scene.light.as_parallel_mut().unwrap().direction = -Vector3::z();
    scene.scene.light.as_parallel_mut().unwrap().total_flux = 8.0;
    scene
        .scene
        .light
        .as_parallel_mut()
        .unwrap()
        .aperture_max_m
        .x = 0.0;
    let image = crate::integrator::camera::integrate_rgb(
        &scene,
        &crate::config::render::RenderSetting {
            width: 1,
            height: 1,
            camera_spp: 1,
            seed: 42,
            ..Default::default()
        },
        None,
    )
    .map(|images| images.combined)
    .unwrap();
    let rgb = xyz_to_linear_srgb(*image.get(0, 0).unwrap()).unwrap();
    assert!((rgb - Vector3::new(0.2, 0.5, 0.8).component_mul(&white) / PI).norm() < 1e-12);
}

#[test]
fn shadow_ray_blocks_light_through_glass_without_hiding_the_ground_from_camera() {
    let mut config = SceneConfig {
        environment: None,
        ground_checkerboard: None,
        light: ParallelLightConfig {
            direction: [0.0, 0.0, -1.0],
            ..Default::default()
        }
        .into(),
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
        let scene = RenderJob::from_config(&config).unwrap();
        let ray = scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap();
        let hit = scene.scene.intersect(&ray).unwrap();
        assert_eq!(Some(hit.object_id), scene.photon.ground_id);
        assert!((hit.coords - Vector3::new(0.25, 0.0, 0.0)).norm() < 1e-12);
        assert_eq!(
            vec![linear_srgb_to_xyz(trace_center_pixel(&scene, 42)).unwrap()],
            vec![Vector3::zeros()]
        );
    }
    // 同样的受光地面移出玻璃投影，恢复原有直接光照。
    config.camera.target = [1.5, 0.0, 0.0];
    let scene = RenderJob::from_config(&config).unwrap();
    let rgb = trace_center_pixel(&scene, 42);
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
    let reference = trace_center_pixel(&scene, 42);
    assert!(reference.norm() > 0.0);
    scene.scene.add(
        Box::new(Ground::new(-1.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    assert_eq!(trace_center_pixel(&scene, 42), reference);

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
    scene.scene.add(
        Box::new(Prism::from_vertices(vertices).unwrap()),
        Box::new(crate::material::dielectric::Dielectric::new(1.52).unwrap()),
    );
    assert_eq!(trace_center_pixel(&scene, 42), Vector3::zeros());
}

#[test]
fn back_facing_surfaces_are_black() {
    let mut scene = ground_scene();
    scene.camera = Camera::from_config(&CameraConfig {
        position: [0.0, 0.0, -1.0],
        target: [0.0; 3],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 45.0,
    })
    .unwrap();
    assert_eq!(
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 1,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .unwrap()
        .pixels,
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
        assert!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width,
                    height,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .is_err()
        );
    }
    for direction in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
        Vector3::repeat(f64::MAX),
    ] {
        scene.scene.light.as_parallel_mut().unwrap().direction = direction;
        assert!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 1,
                    height: 1,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .is_err()
        );
    }
    scene.scene.light.as_parallel_mut().unwrap().direction = -Vector3::z();
    for flux in [-1.0, f64::NAN, f64::INFINITY] {
        scene.scene.light.as_parallel_mut().unwrap().total_flux = flux;
        assert!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 1,
                    height: 1,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .is_err()
        );
    }
    scene.scene.light.as_parallel_mut().unwrap().total_flux = 8.0;
    scene.scene.light.as_parallel_mut().unwrap().aperture_max_m =
        scene.scene.light.as_parallel().unwrap().aperture_min_m;
    assert!(
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 1,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .is_err()
    );
    scene
        .scene
        .light
        .as_parallel_mut()
        .unwrap()
        .aperture_min_m
        .fill(0.0);
    scene
        .scene
        .light
        .as_parallel_mut()
        .unwrap()
        .aperture_max_m
        .fill(1e-150);
    scene.scene.light.as_parallel_mut().unwrap().total_flux = f64::MAX;
    assert!(
        crate::integrator::camera::integrate_rgb(
            &scene,
            &crate::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 1,
                seed: 42,
                ..Default::default()
            },
            None
        )
        .map(|images| images.combined)
        .is_err()
    );
}

#[test]
fn camera_lighting_rejects_invalid_normals_and_bsdf_results() {
    struct TestSurface(Vector3<f64>);
    impl Hittable for TestSurface {
        fn intersect(&self, _ray: &Ray) -> Option<GeometryHit> {
            Some(GeometryHit {
                normal: self.0,
                distance: 0.5,
                coords: nalgebra::Vector3::zeros(),
            })
        }
    }
    for normal in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::MAX),
    ] {
        let mut scene = ground_scene();
        scene.scene.add(
            Box::new(TestSurface(normal)),
            Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
        );
        assert!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 1,
                    height: 1,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .is_err()
        );
    }
    struct TestMaterial(Vector3<f64>);
    impl Material for TestMaterial {
        fn evaluate_rgb(
            &self,
            _wi: Vector3<f64>,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
        ) -> Vector3<f64> {
            self.0
        }
    }
    {
        let value = Vector3::repeat(f64::NAN);
        let mut scene = ground_scene();
        scene.scene.add(
            Box::new(TestSurface(Vector3::z() * 3.0)),
            Box::new(TestMaterial(value)),
        );
        assert!(
            crate::integrator::camera::integrate_rgb(
                &scene,
                &crate::config::render::RenderSetting {
                    width: 1,
                    height: 1,
                    camera_spp: 1,
                    seed: 42,
                    ..Default::default()
                },
                None
            )
            .map(|images| images.combined)
            .is_err()
        );
    }
}

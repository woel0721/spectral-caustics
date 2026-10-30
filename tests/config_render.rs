use nalgebra::Vector3;
use rand::{SeedableRng, rngs::StdRng};
use spectral_caustics::{
    config::{
        Config,
        materialconfig::MaterialConfig,
        sceneconfig::{ObjectConfig, SceneConfig},
    },
    geometry::{ground::Ground, ray::Ray},
    material::{
        BsdfValue, EvaluateMode, GlassEvent, Material, MaterialSample,
        dielectric::fresnel_dielectric,
    },
    optics::{iormode::IorMode, spectrum::rgb_reflectance},
    render::{
        GroundLightingComponent, Render, caustics::ground_to_pixel, framebuffer::Framebuffer,
        render_camera_glass, render_camera_glass_with_ground, render_camera_glass_with_lighting,
        render_camera_lit, utils::linear_srgb_to_xyz,
    },
    scene::Scene,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

struct TemporaryDirectory(PathBuf);

#[test]
fn camera_spp_from_json_controls_camera_paths_without_changing_photon_rendering() {
    let mut config = small_config();
    config.scene.camera.position = [0.0, 0.0, 3.0];
    config.scene.camera.target = [0.0; 3];
    config.scene.camera.up = [0.0, 1.0, 0.0];
    config.scene.background_linear_rgb = [0.2, 0.4, 0.8];
    config.scene.objects = vec![ObjectConfig::Ground {
        height_m: 1.0,
        material: MaterialConfig::Dielectric { ior: 1.5 },
    }];
    let camera_image = |config: &Config| {
        let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
        render_camera_glass(
            &Scene::from_config(&loaded.scene).unwrap(),
            loaded.render.width,
            loaded.render.height,
            loaded.render.camera_spp,
            loaded.render.seed,
        )
        .unwrap()
    };
    config.render.camera_spp = 1;
    let single = camera_image(&config);
    let photons = render(&config);
    config.render.camera_spp = 64;
    let multiple = camera_image(&config);
    assert_ne!(single.pixels, multiple.pixels);
    assert_eq!(photons.pixels, render(&config).pixels);
    assert_eq!(multiple.pixels, camera_image(&config).pixels);
}

impl TemporaryDirectory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "config-render-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn small_config() -> Config {
    let mut config = Config::default();
    config.scene.environment = None; // 原有太阳/光子回归测试显式关闭天空。
    config.scene.ground_checkerboard = None;
    config.scene.light.direction = [0.0, 0.0, -1.0];
    config.render.width = 24;
    config.render.height = 12;
    config.render.samples_per_pixel_axis = 2;
    config.render.thread_count = Some(2);
    config
}

fn render(config: &Config) -> Framebuffer {
    let scene = Scene::from_config(&config.scene).unwrap();
    Render::new(config.render.clone())
        .unwrap()
        .render(&scene)
        .unwrap()
}

fn total_y(image: &Framebuffer) -> f64 {
    image.pixels.iter().map(|v| v.y).sum()
}

#[test]
fn sky_from_json_is_added_once_in_all_camera_entries_without_changing_sun_or_photons() {
    let mut config = small_config();
    config
        .scene
        .objects
        .retain(|object| matches!(object, ObjectConfig::Ground { .. }));
    config.scene.camera.position = [0.0, 0.0, 2.0];
    config.scene.camera.target = [0.0; 3];
    config.scene.camera.up = [0.0, 1.0, 0.0];
    config.scene.light.direction = [-3.0, 0.0, -4.0];
    let renderer = Render::new(config.render.clone()).unwrap();
    let unlit = Scene::from_config(&config.scene).unwrap();
    let mut lighting = renderer.render_lighting(&unlit).unwrap();
    let photons = renderer.render(&unlit).unwrap();
    // 解析太阳不能再叠加这张已包含太阳的采样直射图。
    lighting.direct.pixels.fill(Vector3::repeat(100.0));
    config.scene.environment = Some(spectral_caustics::config::sceneconfig::EnvironmentConfig {
        horizon_linear_rgb: [0.03, 0.04, 0.06],
        zenith_linear_rgb: [0.03, 0.04, 0.06],
    });
    let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
    let scene = Scene::from_config(&loaded.scene).unwrap();
    assert_eq!(photons.pixels, renderer.render(&scene).unwrap().pixels);
    let sky_lighting = renderer.render_lighting(&scene).unwrap();
    assert_eq!(sky_lighting.sun_irradiance_xyz, lighting.sun_irradiance_xyz);
    assert_eq!(sky_lighting.caustics.pixels, lighting.caustics.pixels);
    let render_entries = |scene: &Scene| {
        [
            render_camera_lit(scene, 3, 2).unwrap(),
            render_camera_glass(scene, 3, 2, 8, 42).unwrap(),
            render_camera_glass_with_ground(scene, &photons, 3, 2, 8, 42).unwrap(),
            render_camera_glass_with_lighting(
                scene,
                &lighting,
                3,
                2,
                8,
                42,
                GroundLightingComponent::Combined,
            )
            .unwrap(),
        ]
    };
    let expected_sky = linear_srgb_to_xyz(scene.environment.unwrap().horizon_color * 0.8).unwrap();
    let baseline = render_entries(&unlit);
    let with_sky = render_entries(&scene);
    for (entry, (before, after)) in baseline.iter().zip(&with_sky).enumerate() {
        for (before, after) in before.pixels.iter().zip(&after.pixels) {
            assert!(
                (after - before - expected_sky).norm() < 1e-12,
                "entry {entry}"
            );
        }
    }
    let expected_combined =
        lighting.sun_irradiance_xyz * lighting.light_wi.z * (0.8 / std::f64::consts::PI)
            + expected_sky;
    for xyz in &with_sky[3].pixels {
        assert!((xyz - expected_combined).norm() < 1e-12);
    }
}

#[test]
fn spectral_irradiance_flows_through_gray_ground_camera_paths_and_png_output() {
    let config = small_config();
    let scene = Scene::from_config(&config.scene).unwrap();
    let renderer = Render::new(config.render.clone()).unwrap();
    // 第一遍是未曝光的 XYZ 辐照度图；相机分辨率独立于这张图。
    let ground = renderer.render(&scene).unwrap();
    let camera = |map: &Framebuffer| {
        render_camera_glass_with_ground(&scene, map, 18, 10, 8, config.render.seed).unwrap()
    };
    let image = camera(&ground);
    assert_eq!((image.width, image.height), (18, 10));
    assert_eq!(image.pixels, camera(&ground).pixels);
    let zero = Framebuffer::new(ground.width, ground.height);
    let no_ground = camera(&zero);
    assert!(
        image
            .pixels
            .iter()
            .zip(&no_ground.pixels)
            .any(|(full, black)| full.y > black.y + 1e-9)
    );
    let half = Framebuffer {
        width: ground.width,
        height: ground.height,
        pixels: ground.pixels.iter().map(|p| p * 0.5).collect(),
    };
    let half_image = camera(&half);
    for ((full, half), background) in image
        .pixels
        .iter()
        .zip(&half_image.pixels)
        .zip(&no_ground.pixels)
    {
        assert!((half - (background + (full - background) * 0.5)).norm() < 1e-12);
    }
    let directory = TemporaryDirectory::new();
    let path = directory.0.join("camera_glass_with_ground.png");
    renderer.save(&image, &path).unwrap();
    let saved = image::open(path).unwrap();
    assert_eq!((saved.width(), saved.height()), (18, 10));
    assert_eq!(saved.color(), image::ColorType::Rgb8);
}

#[test]
fn split_lighting_flows_through_fresnel_camera_paths_and_three_debug_pngs() {
    let mut config = small_config();
    config.scene.environment = Some(Default::default());
    config.scene.ground_checkerboard = Some(Default::default());
    let scene = Scene::from_config(&config.scene).unwrap();
    let renderer = Render::new(config.render.clone()).unwrap();
    let lighting = renderer.render_lighting(&scene).unwrap();
    let legacy = renderer.render(&scene).unwrap();
    for (actual, expected) in lighting
        .combined()
        .unwrap()
        .pixels
        .iter()
        .zip(&legacy.pixels)
    {
        assert!((actual - expected).norm() < 1e-12);
    }
    let camera = |component| {
        render_camera_glass_with_lighting(
            &scene,
            &lighting,
            24,
            16,
            16,
            config.render.seed,
            component,
        )
        .unwrap()
    };
    let direct = camera(GroundLightingComponent::Direct);
    let caustics = camera(GroundLightingComponent::Caustics);
    let combined = camera(GroundLightingComponent::Combined);
    let mut solar_scene = Scene::from_config(&config.scene).unwrap();
    solar_scene.environment = None;
    solar_scene.environment_samples = 7;
    assert_eq!(
        caustics.pixels,
        render_camera_glass_with_lighting(
            &solar_scene,
            &lighting,
            24,
            16,
            16,
            config.render.seed,
            GroundLightingComponent::Caustics,
        )
        .unwrap()
        .pixels
    );
    assert_eq!(
        combined.pixels,
        camera(GroundLightingComponent::Combined).pixels
    );
    assert!(caustics.pixels.iter().any(|p| p.y > 1e-6));
    for ((d, c), sum) in direct
        .pixels
        .iter()
        .zip(&caustics.pixels)
        .zip(&combined.pixels)
    {
        assert!((sum - d - c).norm() < 1e-12);
    }
    let directory = TemporaryDirectory::new();
    for (name, image) in [
        ("direct", direct),
        ("caustics", caustics),
        ("combined", combined),
    ] {
        let path = directory.0.join(format!("{name}.png"));
        renderer.save(&image, &path).unwrap();
        let saved = image::open(path).unwrap();
        assert_eq!((saved.width(), saved.height()), (24, 16));
        assert_eq!(saved.color(), image::ColorType::Rgb8);
    }
}

#[test]
fn checkerboard_modulates_all_camera_lighting_once_without_changing_irradiance() {
    let mut config = small_config();
    config.scene.environment = Some(Default::default());
    config
        .scene
        .objects
        .retain(|object| matches!(object, ObjectConfig::Ground { .. }));
    config.scene.camera.up = [0.0, 1.0, 0.0];
    config.scene.camera.fov_y_degrees = 0.001;
    let renderer = Render::new(config.render.clone()).unwrap();
    let reference = Scene::from_config(&config.scene).unwrap();
    let photons = renderer.render(&reference).unwrap();
    let mut lighting = renderer.render_lighting(&reference).unwrap();
    let ground_map = Framebuffer {
        width: 1,
        height: 1,
        pixels: vec![Vector3::new(0.1, 0.2, 0.4)],
    };
    lighting.caustics.pixels.fill(ground_map.pixels[0]);
    let entries = |scene: &Scene| {
        [
            render_camera_lit(scene, 2, 2).unwrap(),
            render_camera_glass(scene, 2, 2, 8, 42).unwrap(),
            render_camera_glass_with_ground(scene, &ground_map, 2, 2, 8, 42).unwrap(),
            render_camera_glass_with_lighting(
                scene,
                &lighting,
                2,
                2,
                8,
                42,
                GroundLightingComponent::Direct,
            )
            .unwrap(),
            render_camera_glass_with_lighting(
                scene,
                &lighting,
                2,
                2,
                8,
                42,
                GroundLightingComponent::Caustics,
            )
            .unwrap(),
            render_camera_glass_with_lighting(
                scene,
                &lighting,
                2,
                2,
                8,
                42,
                GroundLightingComponent::Combined,
            )
            .unwrap(),
        ]
    };
    for (x, y, scale) in [
        (0.25, 0.25, 1.0),
        (0.75, 0.25, 0.25),
        (-0.25, 0.25, 0.25),
        (-0.25, -0.25, 1.0),
    ] {
        config.scene.camera.position = [x, y, 2.0];
        config.scene.camera.target = [x, y, 0.0];
        config.scene.ground_checkerboard = None;
        let before = entries(&Scene::from_config(&config.scene).unwrap());
        config.scene.ground_checkerboard = Some(Default::default());
        // 经 JSON 往返再建场景，纹理必须进入实际相机着色流程。
        let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
        let scene = Scene::from_config(&loaded.scene).unwrap();
        let after = entries(&scene);
        for (entry, (before, after)) in before.iter().zip(&after).enumerate() {
            for (before, after) in before.pixels.iter().zip(&after.pixels) {
                assert!(
                    (after - before * scale).norm() < 1e-12,
                    "entry {entry} at ({x},{y})"
                );
            }
        }
        for ((direct, caustics), combined) in after[3]
            .pixels
            .iter()
            .zip(&after[4].pixels)
            .zip(&after[5].pixels)
        {
            assert!((combined - direct - caustics).norm() < 1e-12);
        }
        assert_eq!(renderer.render(&scene).unwrap().pixels, photons.pixels);
    }
}

#[test]
fn finite_ground_keeps_lighting_beyond_receiver_and_shows_sky_beyond_its_edge() {
    let mut config = small_config();
    config
        .scene
        .objects
        .retain(|object| matches!(object, ObjectConfig::Ground { .. }));
    config.scene.environment = Some(spectral_caustics::config::sceneconfig::EnvironmentConfig {
        horizon_linear_rgb: [0.03, 0.04, 0.06],
        zenith_linear_rgb: [0.03, 0.04, 0.06],
    });
    config.scene.ground_checkerboard = Some(Default::default());
    config.scene.camera.up = [0.0, 1.0, 0.0];
    config.scene.camera.fov_y_degrees = 0.001;
    let renderer = Render::new(config.render.clone()).unwrap();
    let mut lighting = renderer
        .render_lighting(&Scene::from_config(&config.scene).unwrap())
        .unwrap();
    let e_caustics = Vector3::new(0.1, 0.2, 0.4);
    lighting.caustics.pixels.fill(e_caustics);
    let environment = config.scene.environment.as_ref().unwrap();
    let sky = linear_srgb_to_xyz(Vector3::from(environment.horizon_linear_rgb) * 0.8).unwrap();
    let background = linear_srgb_to_xyz(Vector3::from(environment.horizon_linear_rgb)).unwrap();
    let sun = lighting.sun_irradiance_xyz * (lighting.light_wi.z * 0.8 / std::f64::consts::PI);
    for (x, expected_direct, expected_caustics) in [
        (
            1.75,
            (sun + sky) * 0.25,
            e_caustics * (0.8 / std::f64::consts::PI * 0.25),
        ),
        (2.25, sun + sky, Vector3::zeros()),
        (2.75, background, Vector3::zeros()),
    ] {
        config.scene.camera.position = [x, 0.25, 2.0];
        config.scene.camera.target = [x, 0.25, 0.0];
        let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
        let scene = Scene::from_config(&loaded.scene).unwrap();
        let ray = scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap();
        assert_eq!(scene.intersect(&ray).is_some(), x < 2.5);
        for (component, expected) in [
            (GroundLightingComponent::Direct, expected_direct),
            (GroundLightingComponent::Caustics, expected_caustics),
            (
                GroundLightingComponent::Combined,
                expected_direct + expected_caustics,
            ),
        ] {
            let image =
                render_camera_glass_with_lighting(&scene, &lighting, 1, 1, 8, 42, component)
                    .unwrap();
            assert!(
                (image.pixels[0] - expected).norm() < 1e-12,
                "x={x}, {component:?}"
            );
        }
        if x > 2.5 {
            for image in [
                render_camera_lit(&scene, 1, 1).unwrap(),
                render_camera_glass(&scene, 1, 1, 8, 42).unwrap(),
                render_camera_glass_with_ground(&scene, &lighting.caustics, 1, 1, 8, 42).unwrap(),
            ] {
                assert!((image.pixels[0] - background).norm() < 1e-12);
            }
        }
    }
}

#[test]
fn ground_larger_than_receiver_preserves_photon_energy_maps() {
    let mut config = small_config();
    config.scene.light.direction = [0.45, 0.0, -1.0];
    let scene = Scene::from_config(&config.scene).unwrap();
    let mut infinite_reference = Scene::from_config(&config.scene).unwrap();
    infinite_reference.objects[scene.ground_id.unwrap()].geometry =
        Box::new(Ground::new(0.0).unwrap());
    let renderer = Render::new(config.render.clone()).unwrap();
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        assert_eq!(
            renderer.render_mode(&scene, mode).unwrap().pixels,
            renderer
                .render_mode(&infinite_reference, mode)
                .unwrap()
                .pixels
        );
    }
    let finite = renderer.render_lighting(&scene).unwrap();
    let infinite = renderer.render_lighting(&infinite_reference).unwrap();
    assert_eq!(finite.direct.pixels, infinite.direct.pixels);
    assert_eq!(finite.caustics.pixels, infinite.caustics.pixels);
    assert_eq!(finite.sun_irradiance_xyz, infinite.sun_irradiance_xyz);
}

#[test]
fn traced_prism_caustics_light_the_geometric_glass_shadow_independently() {
    let mut config = small_config();
    config.render.width = 48;
    config.render.height = 36;
    let mut scene = Scene::from_config(&config.scene).unwrap();
    let renderer = Render::new(config.render.clone()).unwrap();
    let lighting = renderer.render_lighting(&scene).unwrap();
    let span = lighting.receiver.max_m - lighting.receiver.min_m;
    let mut shadow_point = None;
    for y in 0..lighting.caustics.height {
        for x in 0..lighting.caustics.width {
            if lighting.caustics.get(x, y).unwrap().y <= 1e-6 {
                continue;
            }
            let p = Vector3::new(
                lighting.receiver.min_m.x
                    + (x as f64 + 0.5) / lighting.caustics.width as f64 * span.x,
                lighting.receiver.max_m.y
                    - (y as f64 + 0.5) / lighting.caustics.height as f64 * span.y,
                1e-5,
            );
            let shadow = Ray::new(p, lighting.light_wi).unwrap();
            if scene
                .intersect(&shadow)
                .is_some_and(|hit| hit.object_id == scene.prism_id)
            {
                shadow_point = Some(p);
                break;
            }
        }
        if shadow_point.is_some() {
            break;
        }
    }
    let p = shadow_point.expect("应有经折射的焦散照亮棱镜的几何阴影");
    // 相机放在玻璃底面下方，以便单独检查同一地面位置的直射/焦散。
    scene.camera = spectral_caustics::camera::Camera::from_config(
        &spectral_caustics::config::cameraconfig::CameraConfig {
            position: [p.x, p.y, 0.5],
            target: [p.x, p.y, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 0.001,
        },
    )
    .unwrap();
    let camera = |component| {
        render_camera_glass_with_lighting(&scene, &lighting, 1, 1, 8, 42, component).unwrap()
    };
    let direct = camera(GroundLightingComponent::Direct);
    let caustics = camera(GroundLightingComponent::Caustics);
    let combined = camera(GroundLightingComponent::Combined);
    assert_eq!(direct.pixels[0], Vector3::zeros());
    assert!(caustics.pixels[0].y > 1e-6);
    assert_eq!(combined.pixels, caustics.pixels);
}

#[test]
fn json_camera_generates_rays_that_hit_the_configured_scene() {
    let mut config = small_config();
    // 这条已知主光线指向棱镜内部，独立于展示场景的新构图。
    config.scene.camera.position = [3.0, -5.0, 3.0];
    config.scene.camera.target = [0.0, 0.0, 1.3];
    config.render.width = 5;
    config.render.height = 3;
    // JSON 往返后，由真实的 Scene 构建相机，再用主光线查询现有几何。
    let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
    let scene = Scene::from_config(&loaded.scene).unwrap();
    let ray = scene
        .camera
        .ray_for_pixel(2, 1, loaded.render.width, loaded.render.height)
        .unwrap();
    assert_eq!(ray.origin, Vector3::from(loaded.scene.camera.position));
    assert_eq!(scene.intersect(&ray).unwrap().object_id, scene.prism_id);

    config.scene.camera.target = [1.5, 0.0, 0.0];
    let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
    let ground_view = Scene::from_config(&loaded.scene).unwrap();
    let ground_ray = ground_view
        .camera
        .ray_for_pixel(2, 1, loaded.render.width, loaded.render.height)
        .unwrap();
    let hit = ground_view.intersect(&ground_ray).unwrap();
    assert_eq!(hit.object_id, ground_view.ground_id);
    assert!((hit.coords - Vector3::new(1.5, 0.0, 0.0)).norm() < 1e-12);
    assert!((ray.direction - ground_ray.direction).norm() > 0.1);
    // 保持相机姿态不变，只增大 FOV，边角光线与观察方向的夹角应增大。
    let narrow = ground_view.camera.ray_for_pixel(0, 0, 5, 3).unwrap();
    config.scene.camera.fov_y_degrees = 70.0;
    let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
    let wide_view = Scene::from_config(&loaded.scene).unwrap();
    let wide = wide_view.camera.ray_for_pixel(0, 0, 5, 3).unwrap();
    assert!(
        wide.direction.dot(&ground_ray.direction) < narrow.direction.dot(&ground_ray.direction)
    );
    let wide_center = wide_view.camera.ray_for_pixel(2, 1, 5, 3).unwrap();
    assert!((wide_center.direction - ground_ray.direction).norm() < 1e-12);
}

#[test]
fn json_materials_build_runtime_values_and_follow_the_hit_object() {
    for reverse in [false, true] {
        let mut config = small_config();
        if let ObjectConfig::Prism { material, .. } = &mut config.scene.objects[0] {
            *material = MaterialConfig::Dielectric { ior: 1.33 };
        }
        if let ObjectConfig::Ground { material, .. } = &mut config.scene.objects[1] {
            *material = MaterialConfig::Lambertian {
                albedo: [0.2, 0.3, 0.4],
            };
        }
        if reverse {
            config.scene.objects.reverse();
        }
        let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
        let mut scene = Scene::from_config(&loaded.scene).unwrap();
        for (x, expected_id, expected_bsdf, expected_ior) in [
            (0.25, scene.prism_id, Vector3::zeros(), Some(1.33)),
            (
                1.5,
                scene.ground_id,
                Vector3::new(0.2, 0.3, 0.4) / std::f64::consts::PI,
                None,
            ),
        ] {
            let ray = Ray::new(Vector3::new(x, 0.2, 3.0), -Vector3::z()).unwrap();
            let hit = scene.intersect(&ray).unwrap();
            assert_eq!(hit.object_id, expected_id);
            let material = scene.material(hit.material_id.unwrap()).unwrap();
            assert_eq!(material.ior(), expected_ior);
            assert_eq!(material.is_delta(), expected_ior.is_some());
            // 材质通过 dyn Material 分派；在命中法线的正半球求 BSDF。
            assert_eq!(
                material.evaluate(hit.normal, -ray.direction, hit.normal, EvaluateMode::Rgb),
                BsdfValue::Rgb(expected_bsdf)
            );
            for wavelength_nm in [450.0, 550.0, 650.0] {
                let expected_spectral = if material.is_delta() {
                    0.0
                } else {
                    rgb_reflectance(Vector3::new(0.2, 0.3, 0.4), wavelength_nm).unwrap()
                        / std::f64::consts::PI
                };
                assert_eq!(
                    material.evaluate(
                        hit.normal,
                        -ray.direction,
                        hit.normal,
                        EvaluateMode::Spectral { wavelength_nm },
                    ),
                    BsdfValue::Spectral(expected_spectral)
                );
            }
            let mut rng = StdRng::seed_from_u64(42);
            if material.is_delta() {
                let wo = -ray.direction;
                let MaterialSample::Glass(sample) =
                    material.sample(wo, hit.normal, &mut rng).unwrap()
                else {
                    panic!("JSON 玻璃必须通过统一接口返回离散事件记录");
                };
                assert!((sample.wi.norm() - 1.0).abs() < 1e-12);
                let ior = expected_ior.unwrap();
                let f = fresnel_dielectric(wo.dot(&hit.normal), 1.0, ior).unwrap();
                match sample.event {
                    GlassEvent::Reflection => {
                        assert!(sample.wi.dot(&hit.normal) > 0.0);
                        assert_eq!(sample.probability, f);
                        assert_eq!(sample.weight, 1.0);
                    }
                    GlassEvent::Transmission => {
                        assert!(sample.wi.dot(&hit.normal) < 0.0);
                        assert_eq!(sample.probability, 1.0 - f);
                        assert_eq!(sample.weight, (1.0 / ior).powi(2));
                    }
                }
                assert_eq!(material.pdf(hit.normal, hit.normal, hit.normal), 0.0);
            } else {
                let MaterialSample::Continuous { wi } =
                    material.sample(hit.normal, hit.normal, &mut rng).unwrap()
                else {
                    panic!("JSON Lambertian 必须通过统一接口返回连续方向");
                };
                assert!((wi.norm() - 1.0).abs() < 1e-12);
                let cosine = hit.normal.dot(&wi);
                let pdf = material.pdf(wi, hit.normal, hit.normal);
                assert!(cosine > 0.0 && pdf > 0.0);
                let BsdfValue::Rgb(f) =
                    material.evaluate(wi, hit.normal, hit.normal, EvaluateMode::Rgb)
                else {
                    panic!("RGB 模式必须返回 RGB BSDF");
                };
                let weight = f * (cosine / pdf);
                assert!((weight - Vector3::new(0.2, 0.3, 0.4)).norm() < 1e-12);
            }
            assert_eq!(
                scene.objects[hit.object_id.unwrap()].material_id(),
                hit.material_id
            );
        }
        assert!(scene.material(usize::MAX).is_none());

        // 原有 add 接口继续允许只添加几何；未绑定材质时不生成材质编号。
        let id = scene.add(Box::new(Ground::new(2.5).unwrap()));
        let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), -Vector3::z()).unwrap();
        let hit = scene.intersect(&ray).unwrap();
        assert_eq!(hit.object_id, Some(id));
        assert_eq!(hit.material_id, None);
    }
}

#[test]
fn scene_accepts_custom_materials_without_changing_geometry_or_a_material_enum() {
    struct CustomMaterial;
    impl Material for CustomMaterial {
        fn evaluate(
            &self,
            _wi: Vector3<f64>,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
            mode: EvaluateMode,
        ) -> BsdfValue {
            match mode {
                EvaluateMode::Rgb => BsdfValue::Rgb(Vector3::new(0.1, 0.2, 0.3)),
                EvaluateMode::Spectral { wavelength_nm } => {
                    // 自定义材质也可直接提供光谱 BSDF；这里设为常数 0.2 sr⁻¹。
                    BsdfValue::Spectral(if wavelength_nm.is_finite() && wavelength_nm > 0.0 {
                        0.2
                    } else {
                        0.0
                    })
                }
            }
        }
    }

    let mut scene = Scene::new();
    scene.add(Box::new(Ground::new(0.0).unwrap()));
    let id = scene.add_with_material(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(CustomMaterial),
    );
    let ray = Ray::new(Vector3::new(0.0, 0.0, 3.0), -Vector3::z()).unwrap();
    let hit = scene.intersect(&ray).unwrap();
    assert_eq!(hit.object_id, Some(id));
    assert_eq!(hit.material_id, Some(0)); // 几何编号与材质表编号可以不同。
    let material = scene.material(hit.material_id.unwrap()).unwrap();
    assert_eq!(
        material.evaluate(
            -ray.direction,
            -ray.direction,
            hit.normal,
            EvaluateMode::Rgb
        ),
        BsdfValue::Rgb(Vector3::new(0.1, 0.2, 0.3))
    );
    // 同一个 trait object 按 mode 分派，光谱变体保留标量 BSDF。
    assert_eq!(
        material.evaluate(
            -ray.direction,
            -ray.direction,
            hit.normal,
            EvaluateMode::Spectral {
                wavelength_nm: 550.0
            },
        ),
        BsdfValue::Spectral(0.2)
    );
    // 只提供 BSDF 求值的自定义材质可保留默认的“无采样器”行为。
    let mut rng = StdRng::seed_from_u64(42);
    assert!(
        material
            .sample(-ray.direction, hit.normal, &mut rng)
            .is_none()
    );
    assert_eq!(material.pdf(hit.normal, -ray.direction, hit.normal), 0.0);
    assert!(!material.is_delta());
    assert_eq!(material.ior(), None);
}

#[test]
fn runtime_scene_builds_configured_vertices_ground_light_receiver_and_ids() {
    let mut config = SceneConfig::default();
    if let ObjectConfig::Prism { vertices_m, .. } = &mut config.objects[0] {
        for vertex in vertices_m {
            vertex[0] += 0.75;
            vertex[1] += 0.2;
        }
    }
    config.objects[1] = ObjectConfig::Ground {
        height_m: -0.5,
        material: Default::default(),
    };
    config.objects.reverse();
    config.camera.fov_y_degrees = 65.0;
    config.background_linear_rgb = [0.1, 0.2, 0.3];
    config.light.height_m = 4.0;
    config.light.direction = [0.4, 0.0, -2.0];
    config.light.aperture_min_m = [-1.0, -0.5];
    config.light.aperture_max_m = [1.0, 0.5];
    config.receiver.min_m = [-3.0, -2.0];
    config.receiver.max_m = [3.0, 2.0];
    let scene = Scene::from_config(&config).unwrap();
    assert_eq!(scene.ground_id, Some(0));
    assert_eq!(scene.prism_id, Some(1));
    assert_eq!(scene.camera.fov_y_degrees(), 65.0);
    assert_eq!(scene.background_color, Vector3::new(0.1, 0.2, 0.3));
    assert_eq!(scene.light.height_m, 4.0);
    assert!((scene.light.direction.norm() - 1.0).abs() < 1e-12);
    assert_eq!(scene.light.aperture_min_m.x, -1.0);
    let ray = Ray::new(Vector3::new(1.0, 0.4, 4.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
    let hit = scene.intersect(&ray).unwrap();
    assert_eq!(hit.object_id, scene.prism_id);
    assert!((hit.coords - Vector3::new(1.0, 0.4, 1.4)).norm() < 1e-12);
    let outside = Ray::new(Vector3::new(-1.5, 0.0, 4.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
    assert_eq!(scene.intersect(&outside).unwrap().coords.z, -0.5);
    assert_eq!(scene.receiver.area(), 24.0);
    assert_eq!(
        ground_to_pixel(Vector3::new(-3.0, -2.0, -0.5), 12, 8, &scene.receiver),
        Some((-0.5, 7.5))
    );
    assert_eq!(
        ground_to_pixel(Vector3::new(3.0, 0.0, -0.5), 12, 8, &scene.receiver),
        None
    );
}

#[test]
fn aperture_direction_height_flux_receiver_and_sampling_control_energy_and_positions() {
    let mut config = small_config();
    config.scene.objects = vec![ObjectConfig::Ground {
        height_m: -0.5,
        material: Default::default(),
    }];
    config.scene.light.height_m = 4.0;
    config.scene.light.direction = [0.2, 0.0, -1.0];
    config.scene.light.aperture_min_m = [-1.0, -0.5];
    config.scene.light.aperture_max_m = [1.0, 0.5];
    config.scene.light.total_flux = 6.0;
    config.scene.receiver.min_m = [-3.0, -2.0];
    config.scene.receiver.max_m = [3.0, 2.0];
    config.render.seed = 7;
    config.render.max_spectral_blocks = 7;
    let image = render(&config);
    let expected = 6.0 / 24.0 * (24.0 * 12.0);
    assert!((total_y(&image) - expected).abs() < 1e-10);
    for count in [1, 3] {
        let mut changed = config.clone();
        changed.render.samples_per_pixel_axis = count;
        assert!((total_y(&render(&changed)) - expected).abs() < 1e-10);
    }
    let mut changed = config.clone();
    changed.scene.light.total_flux *= 2.0;
    for (pixel, doubled) in image.pixels.iter().zip(render(&changed).pixels) {
        assert!((pixel * 2.0 - doubled).norm() < 1e-12);
    }
    changed = config.clone();
    changed.scene.light.total_flux = 0.0;
    assert!(
        render(&changed)
            .pixels
            .iter()
            .all(|v| *v == Vector3::zeros())
    );
    changed = config.clone();
    changed.scene.receiver.min_m = [-6.0, -4.0];
    changed.scene.receiver.max_m = [6.0, 4.0];
    assert!((total_y(&render(&changed)) * 4.0 - expected).abs() < 1e-10);
    for light_change in 0..3 {
        changed = config.clone();
        match light_change {
            0 => changed.scene.light.direction = [0.0, 0.0, -1.0],
            1 => changed.scene.light.height_m = 3.0,
            _ => changed.scene.light.aperture_max_m = [1.5, 0.75],
        }
        assert_ne!(render(&changed).pixels, image.pixels);
    }
}

#[test]
fn mode_seed_wavelength_range_and_moved_prism_change_the_computed_image() {
    let config = small_config();
    let reference = render(&config);
    assert_eq!(render(&config).pixels, reference.pixels);
    let mut changed = config.clone();
    changed.render.mode = IorMode::Constant;
    assert_ne!(render(&changed).pixels, reference.pixels);
    changed = config.clone();
    changed.render.seed = 123;
    assert_ne!(render(&changed).pixels, reference.pixels);
    changed = config.clone();
    changed.render.wavelength_min_nm = 450.0;
    changed.render.wavelength_max_nm = 650.0;
    changed.render.wavelength_step_nm = 17.0;
    assert_ne!(render(&changed).pixels, reference.pixels);
    changed = config.clone();
    if let ObjectConfig::Prism { vertices_m, .. } = &mut changed.scene.objects[0] {
        for vertex in vertices_m {
            vertex[0] += 6.0;
        }
    }
    assert_ne!(render(&changed).pixels, reference.pixels);
}

#[test]
fn configured_thread_pool_and_block_counts_preserve_results_and_flux() {
    let mut config = small_config();
    config.render.thread_count = Some(1);
    let renderer = Render::new(config.render.clone()).unwrap();
    assert_eq!(renderer.thread_count(), 1);
    let reference = renderer
        .render(&Scene::from_config(&config.scene).unwrap())
        .unwrap();
    config.render.thread_count = Some(4);
    let renderer = Render::new(config.render.clone()).unwrap();
    assert_eq!(renderer.thread_count(), 4);
    assert_eq!(
        renderer
            .render(&Scene::from_config(&config.scene).unwrap())
            .unwrap()
            .pixels,
        reference.pixels
    );
    for blocks in [1, 5, 32] {
        config.render.max_spectral_blocks = blocks;
        let image = render(&config);
        for (actual, expected) in image.pixels.iter().zip(&reference.pixels) {
            assert!((actual - expected).norm() < 1e-12);
        }
        assert!((total_y(&image) - total_y(&reference)).abs() < 1e-10);
    }
}

#[test]
fn runtime_rejects_unsupported_counts_and_invalid_prism_geometry() {
    let original = SceneConfig::default();
    let mut config = original.clone();
    config.objects.clear();
    assert!(Scene::from_config(&config).is_err());
    config = original.clone();
    config.objects.push(config.objects[0].clone());
    assert!(Scene::from_config(&config).is_err());
    config = original.clone();
    config.objects.push(ObjectConfig::Ground {
        height_m: -1.0,
        material: Default::default(),
    });
    assert!(Scene::from_config(&config).is_err());
    config = original.clone();
    config.objects[0] = ObjectConfig::Prism {
        vertices_m: [[0.0; 3]; 6],
        material: Default::default(),
    };
    assert!(config.validate().is_ok()); // 数据校验与几何构建职责分开。
    assert!(Scene::from_config(&config).is_err());
    config = original.clone();
    if let ObjectConfig::Prism { vertices_m, .. } = &mut config.objects[0] {
        vertices_m.swap(0, 1);
    }
    assert!(Scene::from_config(&config).is_err());
    config = original;
    config.objects[1] = ObjectConfig::Ground {
        height_m: 3.0,
        material: Default::default(),
    };
    assert!(Scene::from_config(&config).is_err());
}

#[test]
fn configured_exposure_scales_linear_color_before_srgb_encoding() {
    let directory = TemporaryDirectory::new();
    let mut framebuffer = Framebuffer::new(1, 1);
    let white = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
    assert!(framebuffer.set(0, 0, white * 0.5));
    for (exposure, byte) in [(0.0, 0), (0.5, 137), (1.0, 188), (2.0, 255)] {
        let mut config = small_config();
        config.render.exposure = exposure;
        let path = directory.0.join("exposure.png");
        Render::new(config.render)
            .unwrap()
            .save(&framebuffer, &path)
            .unwrap();
        assert_eq!(
            image::open(path).unwrap().into_rgb8().get_pixel(0, 0).0,
            [byte; 3]
        );
    }
}

#[test]
fn executable_reads_json_from_argument_and_respects_output_size_mode_exposure_and_path() {
    let directory = TemporaryDirectory::new();
    let mut config = small_config();
    config.render.width = 12;
    config.render.height = 8;
    config.render.mode = IorMode::Constant;
    config.render.wavelength_min_nm = 390.0;
    config.render.wavelength_max_nm = 760.0;
    config.render.wavelength_step_nm = 17.0;
    config.render.output_path = PathBuf::from("nested/custom.png");
    let path = directory.0.join("config.json");
    config.save(&path).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_spectral-caustics"))
            .arg(&path)
            .current_dir(&directory.0)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Constant 主图"));
    let names = [
        "custom.png",
        "dispersion.png",
        "refracted_hits.png",
        "wavelength_450nm.png",
        "wavelength_550nm.png",
        "wavelength_650nm.png",
        "no_prism.png",
        "camera_debug.png",
        "camera_lit.png",
    ];
    for name in names {
        assert_eq!(
            image::open(directory.0.join("nested").join(name))
                .unwrap()
                .into_rgb8()
                .dimensions(),
            (12, 8)
        );
    }
    config.render.exposure = 0.0;
    config.render.output_path = PathBuf::from("nested/no_prism.png");
    config.save(&path).unwrap();
    let output = run();
    assert!(output.status.success(), "{:?}", output);
    for name in [
        "no_prism.png",
        "debug_no_prism.png",
        "dispersion.png",
        "camera_lit.png",
    ] {
        assert!(
            image::open(directory.0.join("nested").join(name))
                .unwrap()
                .into_rgb8()
                .pixels()
                .all(|p| p.0 == [0; 3])
        );
    }
    // 地面 +z 法线的显示颜色不随物理渲染曝光变黑。
    let debug = image::open(directory.0.join("nested/camera_debug.png"))
        .unwrap()
        .into_rgb8();
    assert!(debug.pixels().any(|p| p.0 == [128, 128, 255]));

    // 主图占用调试图文件名时，调试输出加前缀；向上看的相机应只看见背景。
    config.render.output_path = PathBuf::from("nested/camera_debug.png");
    config.scene.camera.position = [0.0, 0.0, 3.0];
    config.scene.camera.target = [0.0, 0.0, 4.0];
    config.scene.camera.up = [0.0, 1.0, 0.0];
    config.save(&path).unwrap();
    let output = run();
    assert!(output.status.success(), "{:?}", output);
    let primary = image::open(directory.0.join("nested/camera_debug.png"))
        .unwrap()
        .into_rgb8();
    assert!(primary.pixels().all(|p| p.0 == [0; 3]));
    let debug = image::open(directory.0.join("nested/debug_camera_debug.png"))
        .unwrap()
        .into_rgb8();
    assert_eq!(debug.dimensions(), (12, 8));
    assert!(debug.pixels().all(|p| p.0 == [25, 30, 45]));

    // 同名主图不能被相机光照图覆盖；背景按线性颜色和配置曝光输出。
    config.render.output_path = PathBuf::from("nested/camera_lit.png");
    config.render.exposure = 1.0;
    config.scene.background_linear_rgb = [0.0, 0.5, 0.0];
    config.save(&path).unwrap();
    let output = run();
    assert!(output.status.success(), "{:?}", output);
    let lit = image::open(directory.0.join("nested/debug_camera_lit.png"))
        .unwrap()
        .into_rgb8();
    assert_eq!(lit.dimensions(), (12, 8));
    assert!(lit.pixels().all(|p| p.0 == [0, 188, 0]));
    let primary = image::open(directory.0.join("nested/camera_lit.png"))
        .unwrap()
        .into_rgb8();
    assert!(primary.pixels().any(|p| p.0 != [0, 188, 0]));

    // 看向地面，用已知 albedo/光强/曝光验证完整 RGB→XYZ→sRGB PNG 链路。
    config.render.output_path = PathBuf::from("nested/custom.png");
    config.render.exposure = std::f64::consts::PI / 2.0;
    config.scene.camera.position = [1.5, 0.0, 2.0];
    config.scene.camera.target = [1.5, 0.0, 0.0];
    config.scene.camera.fov_y_degrees = 5.0;
    config.scene.light.total_flux = 16.0; // L_light = 16 / 16 = 1。
    if let ObjectConfig::Ground { material, .. } = &mut config.scene.objects[1] {
        *material = MaterialConfig::Lambertian {
            albedo: [1.0, 0.0, 0.0],
        };
    }
    config.save(&path).unwrap();
    let output = run();
    assert!(output.status.success(), "{:?}", output);
    let lit = image::open(directory.0.join("nested/camera_lit.png"))
        .unwrap()
        .into_rgb8();
    // (1/π) × 1 × cos(0) × (π/2) = 0.5，sRGB 编码为 188。
    assert!(lit.pixels().all(|p| p.0 == [188, 0, 0]));

    // 从侧面看玻璃下方的红色地面：主光线不经过玻璃，shadow ray 被玻璃挡住。
    config.scene.camera.position = [3.0, 0.0, 2.0];
    config.scene.camera.target = [0.25, 0.0, 0.0];
    config.scene.camera.fov_y_degrees = 1.0;
    config.save(&path).unwrap();
    let output = run();
    assert!(output.status.success(), "{:?}", output);
    let lit = image::open(directory.0.join("nested/camera_lit.png"))
        .unwrap()
        .into_rgb8();
    assert!(lit.pixels().all(|p| p.0 == [0; 3]));

    fs::write(&path, "{").unwrap();
    let output = run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("读取配置"));
}

use nalgebra::Vector3;
use rand::SeedableRng;
use rand::rngs::StdRng;
use spectral_caustics::caustics::ground_to_pixel;
use spectral_caustics::color::linear_srgb_to_xyz;
use spectral_caustics::config::Config;
use spectral_caustics::config::RenderJob;
use spectral_caustics::config::material::MaterialConfig;
use spectral_caustics::config::scene::ObjectConfig;
use spectral_caustics::config::scene::SceneConfig;
use spectral_caustics::film::Framebuffer;
use spectral_caustics::geometry::ground::Ground;
use spectral_caustics::geometry::hit::GeometryHit;
use spectral_caustics::geometry::ray::Ray;
use spectral_caustics::material::Material;
use spectral_caustics::material::ScatterType;
use spectral_caustics::optics::dispersion::IorMode;
use spectral_caustics::optics::interface::fresnel_dielectric;
use spectral_caustics::renderer::Renderer;
use spectral_caustics::spectrum::rgb_reflectance;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

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
        material: MaterialConfig::Dielectric {
            ior: 1.5,
            dispersion: None,
        },
    }];
    let camera_image = |config: &Config| {
        let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
        spectral_caustics::integrator::camera::integrate_rgb(
            &loaded.build().unwrap(),
            &spectral_caustics::config::render::RenderSetting {
                width: loaded.render.width,
                height: loaded.render.height,
                camera_spp: loaded.render.camera_spp,
                seed: loaded.render.seed,
                ..Default::default()
            },
            None,
        )
        .map(|images| images.combined)
        .unwrap()
    };
    config.render.camera_spp = 1;
    let single = camera_image(&config);
    let photons = render(&config);
    config.render.camera_spp = 64;
    let multiple = camera_image(&config);
    assert_ne!(single.pixels(), multiple.pixels());
    assert_eq!(photons.pixels(), render(&config).pixels());
    assert_eq!(multiple.pixels(), camera_image(&config).pixels());
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
    config.scene.light.as_parallel_mut().unwrap().direction = [0.0, 0.0, -1.0];
    config.render.width = 24;
    config.render.height = 12;
    config.render.samples_per_pixel_axis = 2;
    config.render.thread_count = Some(2);
    config
}

fn render(config: &Config) -> Framebuffer {
    let scene = config.build().unwrap();
    spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (Renderer::new(config.render.clone()).unwrap()).settings(),
    )
    .and_then(|images| images.combined())
    .unwrap()
}

fn total_y(image: &Framebuffer) -> f64 {
    image.pixels().iter().map(|v| v.y).sum()
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
    config.scene.light.as_parallel_mut().unwrap().direction = [-3.0, 0.0, -4.0];
    let renderer = Renderer::new(config.render.clone()).unwrap();
    let unlit = config.build().unwrap();
    let mut lighting = spectral_caustics::integrator::photon::integrate_xyz(
        &unlit,
        unlit.photon.caustics_object_id,
        unlit.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .unwrap();
    let photons = spectral_caustics::integrator::photon::integrate_xyz(
        &unlit,
        unlit.photon.caustics_object_id,
        unlit.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .and_then(|images| images.combined())
    .unwrap();
    // 解析太阳不能再叠加这张已包含太阳的采样直射图。
    lighting
        .direct
        .as_mut()
        .unwrap()
        .pixels_mut()
        .fill(Vector3::repeat(100.0));
    config.scene.environment = Some(spectral_caustics::config::scene::EnvironmentConfig {
        horizon_linear_rgb: [0.03, 0.04, 0.06],
        zenith_linear_rgb: [0.03, 0.04, 0.06],
        ..Default::default()
    });
    let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
    let scene = loaded.build().unwrap();
    assert_eq!(
        photons.pixels(),
        spectral_caustics::integrator::photon::integrate_xyz(
            &scene,
            scene.photon.caustics_object_id,
            scene.photon.ground_id.expect("receiver"),
            (renderer).settings()
        )
        .and_then(|images| images.combined())
        .unwrap()
        .pixels()
    );
    let sky_lighting = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .unwrap();
    assert_eq!(sky_lighting.sun_irradiance_xyz, lighting.sun_irradiance_xyz);
    assert_eq!(sky_lighting.caustics.pixels(), lighting.caustics.pixels());
    let render_entries = |scene: &RenderJob| {
        let images = spectral_caustics::integrator::camera::integrate_rgb(
            scene,
            &spectral_caustics::config::render::RenderSetting {
                width: 3,
                height: 2,
                camera_spp: 8,
                seed: 42,
                ..Default::default()
            },
            Some(&lighting),
        )
        .unwrap();
        [
            spectral_caustics::integrator::camera::integrate_rgb(
                scene,
                &spectral_caustics::config::render::RenderSetting {
                    width: 3,
                    height: 2,
                    camera_spp: 8,
                    seed: 42,
                    ..Default::default()
                },
                None,
            )
            .map(|images| images.combined)
            .unwrap(),
            images.direct,
            images.combined,
        ]
    };
    let expected_sky =
        linear_srgb_to_xyz(scene.scene.environment.as_ref().unwrap().horizon_color() * 0.8)
            .unwrap();
    let baseline = render_entries(&unlit);
    let with_sky = render_entries(&scene);
    for (entry, (before, after)) in baseline.iter().zip(&with_sky).enumerate() {
        for (before, after) in before.pixels().iter().zip(after.pixels()) {
            assert!(
                (after - before - expected_sky).norm() < 1e-12,
                "entry {entry}"
            );
        }
    }
    let expected_combined =
        lighting.sun_irradiance_xyz * lighting.light_wi.z * (0.8 / std::f64::consts::PI)
            + expected_sky;
    for xyz in with_sky[2].pixels() {
        assert!((xyz - expected_combined).norm() < 1e-12);
    }
}

#[test]
fn split_lighting_flows_through_fresnel_camera_paths_and_three_debug_pngs() {
    let mut config = small_config();
    config.scene.environment = Some(Default::default());
    config.scene.ground_checkerboard = Some(Default::default());
    let scene = config.build().unwrap();
    let renderer = Renderer::new(config.render.clone()).unwrap();
    let lighting = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .unwrap();
    let photon_total = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .and_then(|images| images.combined())
    .unwrap();
    for (actual, expected) in lighting
        .combined()
        .unwrap()
        .pixels()
        .iter()
        .zip(photon_total.pixels())
    {
        assert!((actual - expected).norm() < 1e-12);
    }
    let images = spectral_caustics::integrator::camera::integrate_rgb(
        &scene,
        &spectral_caustics::config::render::RenderSetting {
            width: 24,
            height: 16,
            camera_spp: 16,
            seed: config.render.seed,
            ..Default::default()
        },
        Some(&lighting),
    )
    .unwrap();
    let repeated = spectral_caustics::integrator::camera::integrate_rgb(
        &scene,
        &spectral_caustics::config::render::RenderSetting {
            width: 24,
            height: 16,
            camera_spp: 16,
            seed: config.render.seed,
            ..Default::default()
        },
        Some(&lighting),
    )
    .unwrap();
    for (actual, repeated) in [
        (&images.direct, &repeated.direct),
        (&images.caustics, &repeated.caustics),
        (&images.combined, &repeated.combined),
    ] {
        assert_eq!(actual.pixels(), repeated.pixels());
    }
    let (direct, caustics, combined) = (&images.direct, &images.caustics, &images.combined);
    for ((direct, caustics), combined) in images
        .direct
        .pixels()
        .iter()
        .zip(images.caustics.pixels())
        .zip(images.combined.pixels())
    {
        assert!((direct + caustics - combined).norm() < 1e-12);
    }
    // 相机查询不能修改光源端的照明数据或随机序列。
    let after = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .unwrap();
    assert_eq!(
        lighting.direct.as_ref().unwrap().pixels(),
        after.direct.as_ref().unwrap().pixels()
    );
    assert_eq!(lighting.caustics.pixels(), after.caustics.pixels());
    assert_eq!(lighting.sun_irradiance_xyz, after.sun_irradiance_xyz);
    assert_eq!(
        spectral_caustics::integrator::photon::integrate_xyz(
            &scene,
            scene.photon.caustics_object_id,
            scene.photon.ground_id.expect("receiver"),
            (renderer).settings()
        )
        .and_then(|images| images.combined())
        .unwrap()
        .pixels(),
        photon_total.pixels()
    );
    let mut solar_scene = config.build().unwrap();
    solar_scene.scene.environment = None;
    solar_scene.camera_options.environment_samples = 7;
    assert_eq!(
        caustics.pixels(),
        spectral_caustics::integrator::camera::integrate_rgb(
            &solar_scene,
            &spectral_caustics::config::render::RenderSetting {
                width: 24,
                height: 16,
                camera_spp: 16,
                seed: config.render.seed,
                ..Default::default()
            },
            Some(&lighting)
        )
        .map(|images| images.caustics)
        .unwrap()
        .pixels()
    );
    assert_eq!(combined.pixels(), repeated.combined.pixels());
    assert!(caustics.pixels().iter().any(|p| p.y > 1e-6));
    for ((d, c), sum) in direct
        .pixels()
        .iter()
        .zip(caustics.pixels())
        .zip(combined.pixels())
    {
        assert!((sum - d - c).norm() < 1e-12);
    }
    let directory = TemporaryDirectory::new();
    for (name, image) in [
        ("direct", images.direct),
        ("caustics", images.caustics),
        ("combined", images.combined),
    ] {
        let path = directory.0.join(format!("{name}.png"));
        spectral_caustics::output::save_png_with_exposure(
            &image,
            &path,
            (renderer).settings().exposure,
        )
        .unwrap();
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
    let renderer = Renderer::new(config.render.clone()).unwrap();
    let reference = config.build().unwrap();
    let photons = spectral_caustics::integrator::photon::integrate_xyz(
        &reference,
        reference.photon.caustics_object_id,
        reference.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .and_then(|images| images.combined())
    .unwrap();
    let mut lighting = spectral_caustics::integrator::photon::integrate_xyz(
        &reference,
        reference.photon.caustics_object_id,
        reference.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .unwrap();
    let mut ground_map = Framebuffer::new(1, 1).unwrap();
    ground_map.pixels_mut()[0] = Vector3::new(0.1, 0.2, 0.4);
    lighting.caustics.pixels_mut().fill(ground_map.pixels()[0]);
    let entries = |scene: &RenderJob| {
        let images = spectral_caustics::integrator::camera::integrate_rgb(
            scene,
            &spectral_caustics::config::render::RenderSetting {
                width: 2,
                height: 2,
                camera_spp: 8,
                seed: 42,
                ..Default::default()
            },
            Some(&lighting),
        )
        .unwrap();
        [images.direct, images.caustics, images.combined]
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
        let before = entries(&config.build().unwrap());
        config.scene.ground_checkerboard = Some(Default::default());
        // 经 JSON 往返再建场景，纹理必须进入实际相机着色流程。
        let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
        let scene = loaded.build().unwrap();
        let after = entries(&scene);
        for (entry, (before, after)) in before.iter().zip(&after).enumerate() {
            for (before, after) in before.pixels().iter().zip(after.pixels()) {
                assert!(
                    (after - before * scale).norm() < 1e-12,
                    "entry {entry} at ({x},{y})"
                );
            }
        }
        for ((direct, caustics), combined) in after[0]
            .pixels()
            .iter()
            .zip(after[1].pixels())
            .zip(after[2].pixels())
        {
            assert!((combined - direct - caustics).norm() < 1e-12);
        }
        assert_eq!(
            spectral_caustics::integrator::photon::integrate_xyz(
                &scene,
                scene.photon.caustics_object_id,
                scene.photon.ground_id.expect("receiver"),
                (renderer).settings()
            )
            .and_then(|images| images.combined())
            .unwrap()
            .pixels(),
            photons.pixels()
        );
    }
}

#[test]
fn finite_ground_keeps_lighting_beyond_receiver_and_shows_sky_beyond_its_edge() {
    let mut config = small_config();
    config
        .scene
        .objects
        .retain(|object| matches!(object, ObjectConfig::Ground { .. }));
    config.scene.environment = Some(spectral_caustics::config::scene::EnvironmentConfig {
        horizon_linear_rgb: [0.03, 0.04, 0.06],
        zenith_linear_rgb: [0.03, 0.04, 0.06],
        ..Default::default()
    });
    config.scene.ground_checkerboard = Some(Default::default());
    config.scene.camera.up = [0.0, 1.0, 0.0];
    config.scene.camera.fov_y_degrees = 0.001;
    let renderer = Renderer::new(config.render.clone()).unwrap();
    let mut lighting = spectral_caustics::integrator::photon::integrate_xyz(
        &config.build().unwrap(),
        config.build().unwrap().photon.caustics_object_id,
        config.build().unwrap().photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .unwrap();
    let e_caustics = Vector3::new(0.1, 0.2, 0.4);
    lighting.caustics.pixels_mut().fill(e_caustics);
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
        let scene = loaded.build().unwrap();
        let ray = scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap();
        assert_eq!(scene.scene.intersect(&ray).is_some(), x < 2.5);
        let images = spectral_caustics::integrator::camera::integrate_rgb(
            &scene,
            &spectral_caustics::config::render::RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 8,
                seed: 42,
                ..Default::default()
            },
            Some(&lighting),
        )
        .unwrap();
        for (image, expected) in [
            (&images.direct, expected_direct),
            (&images.caustics, expected_caustics),
            (&images.combined, expected_direct + expected_caustics),
        ] {
            assert!((image.pixels()[0] - expected).norm() < 1e-12, "x={x}");
        }
    }
}

#[test]
fn ground_larger_than_receiver_preserves_photon_energy_maps() {
    let mut config = small_config();
    config.scene.light.as_parallel_mut().unwrap().direction = [0.45, 0.0, -1.0];
    let scene = config.build().unwrap();
    let mut infinite_reference = config.build().unwrap();
    infinite_reference.scene.objects[scene.photon.ground_id.unwrap()].geometry =
        Box::new(Ground::new(0.0).unwrap());
    let renderer = Renderer::new(config.render.clone()).unwrap();
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        assert_eq!(
            spectral_caustics::integrator::photon::integrate_xyz(
                &scene,
                scene.photon.caustics_object_id,
                scene.photon.ground_id.expect("receiver"),
                &spectral_caustics::config::render::RenderSetting {
                    mode,
                    ..(renderer).settings().clone()
                }
            )
            .and_then(|images| images.combined())
            .unwrap()
            .pixels(),
            spectral_caustics::integrator::photon::integrate_xyz(
                &infinite_reference,
                infinite_reference.photon.caustics_object_id,
                infinite_reference.photon.ground_id.expect("receiver"),
                &spectral_caustics::config::render::RenderSetting {
                    mode,
                    ..(renderer).settings().clone()
                }
            )
            .and_then(|images| images.combined())
            .unwrap()
            .pixels()
        );
    }
    let finite = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .unwrap();
    let infinite = spectral_caustics::integrator::photon::integrate_xyz(
        &infinite_reference,
        infinite_reference.photon.caustics_object_id,
        infinite_reference.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .unwrap();
    assert_eq!(
        finite.direct.as_ref().unwrap().pixels(),
        infinite.direct.as_ref().unwrap().pixels()
    );
    assert_eq!(finite.caustics.pixels(), infinite.caustics.pixels());
    assert_eq!(finite.sun_irradiance_xyz, infinite.sun_irradiance_xyz);
}

#[test]
fn traced_prism_caustics_light_the_geometric_glass_shadow_independently() {
    let mut config = small_config();
    config.render.width = 48;
    config.render.height = 36;
    let mut scene = config.build().unwrap();
    let renderer = Renderer::new(config.render.clone()).unwrap();
    let lighting = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .unwrap();
    let span = lighting.receiver.max_m - lighting.receiver.min_m;
    let mut shadow_point = None;
    for y in 0..lighting.caustics.height() {
        for x in 0..lighting.caustics.width() {
            if lighting.caustics.get(x, y).unwrap().y <= 1e-6 {
                continue;
            }
            let p = Vector3::new(
                lighting.receiver.min_m.x
                    + (x as f64 + 0.5) / lighting.caustics.width() as f64 * span.x,
                lighting.receiver.max_m.y
                    - (y as f64 + 0.5) / lighting.caustics.height() as f64 * span.y,
                1e-5,
            );
            let shadow = Ray::new(p, lighting.light_wi).unwrap();
            if scene
                .scene
                .intersect(&shadow)
                .is_some_and(|hit| Some(hit.object_id) == scene.photon.caustics_object_id)
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
        &spectral_caustics::config::camera::CameraConfig {
            position: [p.x, p.y, 0.015],
            target: [p.x, p.y, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 0.001,
        },
    )
    .unwrap();
    let images = spectral_caustics::integrator::camera::integrate_rgb(
        &scene,
        &spectral_caustics::config::render::RenderSetting {
            width: 1,
            height: 1,
            camera_spp: 8,
            seed: 42,
            ..Default::default()
        },
        Some(&lighting),
    )
    .unwrap();
    let (direct, caustics, combined) = (images.direct, images.caustics, images.combined);
    assert_eq!(direct.pixels()[0], Vector3::zeros());
    assert!(caustics.pixels()[0].y > 1e-6);
    assert_eq!(combined.pixels(), caustics.pixels());
}

#[test]
fn json_camera_generates_rays_that_hit_the_configured_scene() {
    let mut config = small_config();
    // 这条已知主光线指向棱镜内部，独立于展示场景的新构图。
    config.scene.camera.position = [3.0, -5.0, 3.0];
    config.scene.camera.target = [0.0, 0.0, 0.4];
    config.render.width = 5;
    config.render.height = 3;
    // JSON 往返后，由真实的 RenderJob 构建相机，再用主光线查询现有几何。
    let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
    let scene = loaded.build().unwrap();
    let ray = scene
        .camera
        .ray_for_pixel(2, 1, loaded.render.width, loaded.render.height)
        .unwrap();
    assert_eq!(ray.origin, Vector3::from(loaded.scene.camera.position));
    assert_eq!(
        Some(scene.scene.intersect(&ray).unwrap().object_id),
        scene.photon.caustics_object_id
    );

    config.scene.camera.target = [1.5, 0.0, 0.0];
    let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
    let ground_view = loaded.build().unwrap();
    let ground_ray = ground_view
        .camera
        .ray_for_pixel(2, 1, loaded.render.width, loaded.render.height)
        .unwrap();
    let hit = ground_view.scene.intersect(&ground_ray).unwrap();
    assert_eq!(Some(hit.object_id), ground_view.photon.ground_id);
    assert!((hit.coords - Vector3::new(1.5, 0.0, 0.0)).norm() < 1e-12);
    assert!((ray.direction - ground_ray.direction).norm() > 0.1);
    // 保持相机姿态不变，只增大 FOV，边角光线与观察方向的夹角应增大。
    let narrow = ground_view.camera.ray_for_pixel(0, 0, 5, 3).unwrap();
    config.scene.camera.fov_y_degrees = 70.0;
    let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
    let wide_view = loaded.build().unwrap();
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
            *material = MaterialConfig::Dielectric {
                ior: 1.33,
                dispersion: None,
            };
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
        let scene = loaded.build().unwrap();
        for (x, expected_id, expected_bsdf, expected_ior) in [
            (
                0.25,
                scene.photon.caustics_object_id,
                Vector3::zeros(),
                Some(1.33),
            ),
            (
                1.5,
                scene.photon.ground_id,
                Vector3::new(0.2, 0.3, 0.4) / std::f64::consts::PI,
                None,
            ),
        ] {
            let ray = Ray::new(Vector3::new(x, 0.2, 3.0), -Vector3::z()).unwrap();
            let hit = scene.scene.intersect(&ray).unwrap();
            assert_eq!(Some(hit.object_id), expected_id);
            let material = scene.scene.material(hit.material_id);
            assert_eq!(material.ior(), expected_ior);
            assert_eq!(!material.has_non_delta(), expected_ior.is_some());
            // 材质通过 dyn Material 分派；在命中法线的正半球求 BSDF。
            assert_eq!(
                material.evaluate_rgb(hit.normal, -ray.direction, hit.normal),
                (expected_bsdf)
            );
            for wavelength_nm in [450.0, 550.0, 650.0] {
                let expected_spectral = if !material.has_non_delta() {
                    0.0
                } else {
                    rgb_reflectance(Vector3::new(0.2, 0.3, 0.4), wavelength_nm).unwrap()
                        / std::f64::consts::PI
                };
                assert_eq!(
                    material.evaluate_spectral(
                        hit.normal,
                        -ray.direction,
                        hit.normal,
                        wavelength_nm
                    ),
                    (expected_spectral)
                );
            }
            let mut rng = StdRng::seed_from_u64(42);
            if !material.has_non_delta() {
                let wo = -ray.direction;
                let sample = material.sample(wo, hit.normal, &mut rng).unwrap();
                assert!((sample.wi.norm() - 1.0).abs() < 1e-12);
                let ior = expected_ior.unwrap();
                assert!(fresnel_dielectric(wo.dot(&hit.normal), 1.0, ior).unwrap() >= 0.0);
                match sample.event {
                    ScatterType::SpecularReflection => {
                        assert!(sample.wi.dot(&hit.normal) > 0.0);

                        assert_eq!(sample.weight, Vector3::repeat(1.0));
                    }
                    ScatterType::SpecularTransmission => {
                        assert!(sample.wi.dot(&hit.normal) < 0.0);

                        assert_eq!(sample.weight, Vector3::repeat((1.0 / ior).powi(2)));
                    }
                    _ => unreachable!(),
                }
                assert_eq!(material.pdf(hit.normal, hit.normal, hit.normal), 0.0);
            } else {
                let sample = material.sample(hit.normal, hit.normal, &mut rng).unwrap();
                let wi = sample.wi;
                assert!((wi.norm() - 1.0).abs() < 1e-12);
                let cosine = hit.normal.dot(&wi);
                let pdf = material.pdf(wi, hit.normal, hit.normal);
                assert!(cosine > 0.0 && pdf > 0.0);
                let f = material.evaluate_rgb(wi, hit.normal, hit.normal);
                let weight = f * (cosine / pdf);
                assert!((weight - Vector3::new(0.2, 0.3, 0.4)).norm() < 1e-12);
            }
            assert_eq!(
                scene.scene.objects[hit.object_id].material_id(),
                hit.material_id
            );
        }
    }
}

#[test]
fn scene_accepts_custom_materials_without_changing_geometry_or_a_material_enum() {
    struct CustomMaterial;
    impl Material for CustomMaterial {
        fn evaluate_rgb(
            &self,
            _wi: Vector3<f64>,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
        ) -> Vector3<f64> {
            Vector3::new(0.1, 0.2, 0.3)
        }
        fn evaluate_spectral(
            &self,
            _wi: Vector3<f64>,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
            wavelength_nm: f64,
        ) -> f64 {
            {
                {
                    // 自定义材质也可直接提供光谱 BSDF；这里设为常数 0.2 sr⁻¹。
                    if wavelength_nm.is_finite() && wavelength_nm > 0.0 {
                        0.2
                    } else {
                        0.0
                    }
                }
            }
        }
    }

    let mut scene = RenderJob::new();
    scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            spectral_caustics::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(
                0.8,
            ))
            .unwrap(),
        ),
    );
    let id = scene.scene.add(
        Box::new(Ground::new(1.0).unwrap()),
        Box::new(CustomMaterial),
    );
    let ray = Ray::new(Vector3::new(0.0, 0.0, 3.0), -Vector3::z()).unwrap();
    let hit = scene.scene.intersect(&ray).unwrap();
    assert_eq!(hit.object_id, id);
    assert_eq!(hit.material_id, 1); // 两个物体均显式绑定材质。
    let material = scene.scene.material(hit.material_id);
    assert_eq!(
        material.evaluate_rgb(-ray.direction, -ray.direction, hit.normal),
        (Vector3::new(0.1, 0.2, 0.3))
    );
    // 同一个 trait object 按 mode 分派，光谱变体保留标量 BSDF。
    assert_eq!(
        material.evaluate_spectral(-ray.direction, -ray.direction, hit.normal, 550.0),
        (0.2)
    );
    // 只提供 BSDF 求值的自定义材质可保留默认的“无采样器”行为。
    let mut rng = StdRng::seed_from_u64(42);
    assert!(
        material
            .sample(-ray.direction, hit.normal, &mut rng)
            .is_none()
    );
    assert_eq!(material.pdf(hit.normal, -ray.direction, hit.normal), 0.0);
    assert!(!!material.has_non_delta());
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
    config.light.as_parallel_mut().unwrap().height_m = 4.0;
    config.light.as_parallel_mut().unwrap().direction = [0.4, 0.0, -2.0];
    config.light.as_parallel_mut().unwrap().aperture_min_m = [-1.0, -0.5];
    config.light.as_parallel_mut().unwrap().aperture_max_m = [1.0, 0.5];
    config.receiver.min_m = [-3.0, -2.0];
    config.receiver.max_m = [3.0, 2.0];
    let scene = RenderJob::from_config(&config).unwrap();
    assert_eq!(scene.photon.ground_id, Some(0));
    assert_eq!(scene.photon.caustics_object_id, Some(1));
    assert_eq!(scene.camera.fov_y_degrees(), 65.0);
    assert_eq!(scene.scene.background_color, Vector3::new(0.1, 0.2, 0.3));
    assert_eq!(scene.scene.light.as_parallel().unwrap().height_m, 4.0);
    assert!((scene.scene.light.as_parallel().unwrap().direction.norm() - 1.0).abs() < 1e-12);
    assert_eq!(
        scene.scene.light.as_parallel().unwrap().aperture_min_m.x,
        -1.0
    );
    let ray = Ray::new(Vector3::new(1.0, 0.4, 4.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
    let hit = scene.scene.intersect(&ray).unwrap();
    assert_eq!(Some(hit.object_id), scene.photon.caustics_object_id);
    assert!((hit.coords - Vector3::new(1.0, 0.4, 0.43)).norm() < 1e-12);
    let outside = Ray::new(Vector3::new(-1.5, 0.0, 4.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
    assert_eq!(scene.scene.intersect(&outside).unwrap().coords.z, -0.5);
    assert_eq!(scene.photon.receiver.area(), 24.0);
    assert_eq!(
        ground_to_pixel(
            Vector3::new(-3.0, -2.0, -0.5),
            12,
            8,
            &scene.photon.receiver
        ),
        Some((-0.5, 7.5))
    );
    assert_eq!(
        ground_to_pixel(Vector3::new(3.0, 0.0, -0.5), 12, 8, &scene.photon.receiver),
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
    config.scene.light.as_parallel_mut().unwrap().height_m = 4.0;
    config.scene.light.as_parallel_mut().unwrap().direction = [0.2, 0.0, -1.0];
    config.scene.light.as_parallel_mut().unwrap().aperture_min_m = [-1.0, -0.5];
    config.scene.light.as_parallel_mut().unwrap().aperture_max_m = [1.0, 0.5];
    config.scene.light.as_parallel_mut().unwrap().total_flux = 6.0;
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
    changed.scene.light.as_parallel_mut().unwrap().total_flux *= 2.0;
    for (pixel, doubled) in image.pixels().iter().zip(render(&changed).pixels()) {
        assert!((pixel * 2.0 - doubled).norm() < 1e-12);
    }
    changed = config.clone();
    changed.scene.light.as_parallel_mut().unwrap().total_flux = 0.0;
    assert!(
        render(&changed)
            .pixels()
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
            0 => changed.scene.light.as_parallel_mut().unwrap().direction = [0.0, 0.0, -1.0],
            1 => changed.scene.light.as_parallel_mut().unwrap().height_m = 3.0,
            _ => {
                changed
                    .scene
                    .light
                    .as_parallel_mut()
                    .unwrap()
                    .aperture_max_m = [1.5, 0.75]
            }
        }
        assert_ne!(render(&changed).pixels(), image.pixels());
    }
}

#[test]
fn mode_seed_wavelength_range_and_moved_prism_change_the_computed_image() {
    let config = small_config();
    let reference = render(&config);
    assert_eq!(render(&config).pixels(), reference.pixels());
    let mut changed = config.clone();
    changed.render.mode = IorMode::Constant;
    assert_ne!(render(&changed).pixels(), reference.pixels());
    changed = config.clone();
    changed.render.seed = 123;
    assert_ne!(render(&changed).pixels(), reference.pixels());
    changed = config.clone();
    changed.render.wavelength_min_nm = 450.0;
    changed.render.wavelength_max_nm = 650.0;
    changed.render.wavelength_step_nm = 17.0;
    assert_ne!(render(&changed).pixels(), reference.pixels());
    changed = config.clone();
    if let ObjectConfig::Prism { vertices_m, .. } = &mut changed.scene.objects[0] {
        for vertex in vertices_m {
            vertex[0] += 6.0;
        }
    }
    assert_ne!(render(&changed).pixels(), reference.pixels());
}

#[test]
fn configured_thread_pool_and_block_counts_preserve_results_and_flux() {
    let mut config = small_config();
    config.render.thread_count = Some(1);
    let renderer = Renderer::new(config.render.clone()).unwrap();
    assert_eq!(renderer.thread_count(), 1);
    let reference = spectral_caustics::integrator::photon::integrate_xyz(
        &config.build().unwrap(),
        config.build().unwrap().photon.caustics_object_id,
        config.build().unwrap().photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .and_then(|images| images.combined())
    .unwrap();
    config.render.thread_count = Some(4);
    let renderer = Renderer::new(config.render.clone()).unwrap();
    assert_eq!(renderer.thread_count(), 4);
    assert_eq!(
        spectral_caustics::integrator::photon::integrate_xyz(
            &config.build().unwrap(),
            config.build().unwrap().photon.caustics_object_id,
            config.build().unwrap().photon.ground_id.expect("receiver"),
            (renderer).settings()
        )
        .and_then(|images| images.combined())
        .unwrap()
        .pixels(),
        reference.pixels()
    );
    for blocks in [1, 5, 32] {
        config.render.max_spectral_blocks = blocks;
        let image = render(&config);
        for (actual, expected) in image.pixels().iter().zip(reference.pixels()) {
            assert!((actual - expected).norm() < 1e-12);
        }
        assert!((total_y(&image) - total_y(&reference)).abs() < 1e-10);
    }
}

#[test]
fn camera_pixels_are_reproducible_across_configured_thread_counts() {
    let mut config = small_config();
    config.render.height = 16;
    config.scene.environment = Some(Default::default());
    config.scene.environment_samples = 3;
    config.scene.ground_checkerboard = Some(Default::default());
    let scene = config.build().unwrap();
    let renderer = Renderer::new(config.render.clone()).unwrap();
    let lighting = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (renderer).settings(),
    )
    .unwrap();
    let reference = spectral_caustics::integrator::camera::integrate_rgb(
        &scene,
        &spectral_caustics::config::render::RenderSetting {
            width: (renderer).settings().width,
            height: (renderer).settings().height,
            camera_spp: 8,
            seed: config.render.seed,
            ..Default::default()
        },
        Some(&lighting),
    )
    .unwrap();
    assert!(reference.direct.pixels().iter().any(|p| p.y > 0.0));
    assert!(reference.caustics.pixels().iter().any(|p| p.y > 0.0));
    for threads in [Some(1), Some(2), Some(4), None] {
        config.render.thread_count = threads;
        let renderer = Renderer::new(config.render.clone()).unwrap();
        if let Some(threads) = threads {
            assert_eq!(renderer.thread_count(), threads);
        }
        for _ in 0..2 {
            let images = spectral_caustics::integrator::camera::integrate_rgb(
                &scene,
                &spectral_caustics::config::render::RenderSetting {
                    width: (renderer).settings().width,
                    height: (renderer).settings().height,
                    camera_spp: 8,
                    seed: config.render.seed,
                    ..Default::default()
                },
                Some(&lighting),
            )
            .unwrap();
            for (actual, expected) in [
                (&images.direct, &reference.direct),
                (&images.caustics, &reference.caustics),
                (&images.combined, &reference.combined),
            ] {
                assert_eq!((actual.width(), actual.height()), (24, 16));
                assert_eq!(actual.pixels(), expected.pixels());
            }
            for ((direct, caustics), combined) in images
                .direct
                .pixels()
                .iter()
                .zip(images.caustics.pixels())
                .zip(images.combined.pixels())
            {
                assert!((direct + caustics - combined).norm() < 1e-12);
            }
        }
        // 相机线程池和逐像素 RNG 不影响已有光源通道。
        let photons = spectral_caustics::integrator::photon::integrate_xyz(
            &scene,
            scene.photon.caustics_object_id,
            scene.photon.ground_id.expect("receiver"),
            (renderer).settings(),
        )
        .unwrap();
        assert_eq!(
            photons.direct.as_ref().unwrap().pixels(),
            lighting.direct.as_ref().unwrap().pixels()
        );
        assert_eq!(photons.caustics.pixels(), lighting.caustics.pixels());
    }
}

#[test]
fn camera_paths_run_inside_the_configured_render_pool() {
    use spectral_caustics::geometry::object::Hittable;
    // 场景需要拥有静态几何，使用 Arc 共享只记录线程池大小的测试探针。
    struct OwnedPoolProbe(std::sync::Arc<AtomicUsize>);
    impl Hittable for OwnedPoolProbe {
        fn intersect(&self, _ray: &Ray) -> Option<GeometryHit> {
            self.0
                .fetch_or(1 << rayon::current_num_threads(), Ordering::Relaxed);
            None
        }
    }
    let mut config = small_config();
    let mut scene = config.build().unwrap();
    let lighting = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (Renderer::new(config.render.clone()).unwrap()).settings(),
    )
    .unwrap();
    let observed = std::sync::Arc::new(AtomicUsize::new(0));
    scene.scene.add(
        Box::new(OwnedPoolProbe(observed.clone())),
        Box::new(
            spectral_caustics::material::lambertian::Lambertian::new(nalgebra::Vector3::zeros())
                .unwrap(),
        ),
    );
    for threads in [1, 2, 4] {
        config.render.thread_count = Some(threads);
        let renderer = Renderer::new(config.render.clone()).unwrap();
        observed.store(0, Ordering::Relaxed);
        renderer.render(&scene).unwrap();
        assert_eq!(observed.load(Ordering::Relaxed), 1 << threads);
        assert!(
            spectral_caustics::integrator::camera::integrate_rgb(
                &scene,
                &spectral_caustics::config::render::RenderSetting {
                    width: (renderer).settings().width,
                    height: (renderer).settings().height,
                    camera_spp: 0,
                    seed: 42,
                    ..Default::default()
                },
                Some(&lighting)
            )
            .is_err()
        );
    }
    // 让错误发生在像素路径内，验证并行迭代和 install 两层都保留错误消息。
    scene.scene.environment = None;
    scene.scene.background_color = Vector3::repeat(f64::NAN);
    scene.camera = spectral_caustics::camera::Camera::from_config(
        &spectral_caustics::config::camera::CameraConfig {
            position: [0.0, 0.0, 3.0],
            target: [0.0, 0.0, 4.0],
            up: [0.0, 1.0, 0.0],
            ..Default::default()
        },
    )
    .unwrap();
    let renderer = Renderer::new(config.render).unwrap();
    let split_error = spectral_caustics::integrator::camera::integrate_rgb(
        &scene,
        &spectral_caustics::config::render::RenderSetting {
            width: (renderer).settings().width,
            height: (renderer).settings().height,
            camera_spp: 8,
            seed: 42,
            ..Default::default()
        },
        Some(&lighting),
    )
    .err()
    .unwrap()
    .to_string();
    let single_error = spectral_caustics::integrator::camera::integrate_rgb(
        &scene,
        &spectral_caustics::config::render::RenderSetting {
            width: 24,
            height: 16,
            camera_spp: 8,
            seed: 42,
            ..Default::default()
        },
        None,
    )
    .map(|images| images.combined)
    .err()
    .unwrap()
    .to_string();
    assert!(!split_error.is_empty());
    assert_eq!(split_error, single_error);
}

#[test]
fn runtime_rejects_unsupported_counts_and_invalid_prism_geometry() {
    let original = SceneConfig::default();
    let mut config = original.clone();
    config.objects.clear();
    assert!(RenderJob::from_config(&config).is_err());
    config = original.clone();
    config.objects.push(config.objects[0].clone());
    assert!(RenderJob::from_config(&config).is_err());
    config = original.clone();
    config.objects.push(ObjectConfig::Ground {
        height_m: -1.0,
        material: Default::default(),
    });
    assert!(RenderJob::from_config(&config).is_err());
    config = original.clone();
    config.objects[0] = ObjectConfig::Prism {
        caustics: true,
        medium_interface: None,
        vertices_m: [[0.0; 3]; 6],
        material: MaterialConfig::Dielectric {
            ior: 1.5,
            dispersion: None,
        },
    };
    assert!(config.validate().is_ok()); // 数据校验与几何构建职责分开。
    assert!(RenderJob::from_config(&config).is_err());
    config = original.clone();
    if let ObjectConfig::Prism { vertices_m, .. } = &mut config.objects[0] {
        vertices_m.swap(0, 1);
    }
    assert!(RenderJob::from_config(&config).is_err());
    config = original;
    config.objects[1] = ObjectConfig::Ground {
        height_m: 3.0,
        material: Default::default(),
    };
    assert!(RenderJob::from_config(&config).is_err());
}

#[test]
fn configured_exposure_scales_linear_color_before_aces_and_srgb_encoding() {
    let directory = TemporaryDirectory::new();
    let mut framebuffer = Framebuffer::new(1, 1).unwrap();
    let white = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
    assert!(framebuffer.set(0, 0, white * 0.5));
    for (exposure, byte) in [
        (0.0, 0),
        (0.5, 165),
        (1.0, 206),
        (2.0, 232),
        (4.0, 245),
        (8.0, 252),
    ] {
        let mut config = small_config();
        config.render.exposure = exposure;
        let path = directory.0.join("exposure.png");
        spectral_caustics::output::save_png_with_exposure(
            &framebuffer,
            &path,
            (Renderer::new(config.render).unwrap()).settings().exposure,
        )
        .unwrap();
        assert_eq!(
            image::open(path).unwrap().into_rgb8().get_pixel(0, 0).0,
            [byte; 3]
        );
    }
}

#[cfg(feature = "diagnostics")]
#[test]
fn default_executable_renders_all_three_spheres_and_repeats_across_threads() {
    let directory = TemporaryDirectory::new();
    fs::create_dir(directory.0.join("configs")).unwrap();
    let mut config = Config::from_json(include_str!("../configs/default.json")).unwrap();
    config.render.width = 96;
    config.render.height = 54;
    config.render.camera_spp = 8;
    config.render.samples_per_pixel_axis = 1;
    #[cfg(feature = "diagnostics")]
    {
        config.render.diagnostics.path = true;
    }
    // 入口验证使用渐变，HDRI 的加载、采样和积分由现有数值测试覆盖。
    config.scene.environment.as_mut().unwrap().map_path = None;
    let scene = config.build().unwrap();
    assert_eq!(scene.photon.caustics_object_id, None);
    let output_directory = directory.0.join("output/spheres");
    let mut reference = None;
    for (target, threads) in [(0, 1), (1, 2), (2, 4)] {
        #[cfg(feature = "diagnostics")]
        {
            config.render.diagnostics.target = Some(target);
        }
        config.render.thread_count = Some(threads);
        config
            .save(directory.0.join("configs/default.json"))
            .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_spectral-caustics"))
            .current_dir(&directory.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let images: Vec<_> = [
            "camera_direct.png",
            "camera_caustics.png",
            "camera_combined.png",
        ]
        .iter()
        .map(|name| {
            let path = output_directory.join(name);
            let image = image::open(&path).unwrap();
            assert_eq!((image.width(), image.height()), (96, 54));
            assert_eq!(image.color(), image::ColorType::Rgb8);
            fs::read(path).unwrap()
        })
        .collect();
        assert_eq!(images[0], images[2]);
        assert!(
            image::open(output_directory.join("camera_caustics.png"))
                .unwrap()
                .into_rgb8()
                .pixels()
                .all(|p| p.0 == [0; 3])
        );
        if let Some(expected) = &reference {
            assert_eq!(&images, expected);
        } else {
            reference = Some(images);
        }
        assert_eq!(fs::read_dir(&output_directory).unwrap().count(), 8);
        let stats: serde_json::Value =
            serde_json::from_slice(&fs::read(output_directory.join("path_debug.json")).unwrap())
                .unwrap();
        assert_eq!(stats["target_object_id"], target);
        let pixels = stats["pixels"].as_array().unwrap();
        for object in 0..4 {
            let key = object.to_string();
            assert!(
                pixels
                    .iter()
                    .any(|p| p["first_hit_objects"][&key].as_u64().unwrap_or(0) > 0)
            );
        }
        let event = [
            "specular_transmission",
            "specular_reflection",
            "glossy_reflection",
        ][target];
        assert!(
            pixels
                .iter()
                .any(|p| p["events"][event].as_u64().unwrap() > 0)
        );
    }
}

#[test]
fn executable_reads_json_and_generates_current_outputs_reproducibly() {
    let directory = TemporaryDirectory::new();
    let mut config = small_config();
    config.render.width = 12;
    config.render.height = 8;
    config.render.camera_spp = 8;
    config.render.mode = IorMode::Constant;
    config.render.wavelength_min_nm = 390.0;
    config.render.wavelength_max_nm = 760.0;
    config.render.wavelength_step_nm = 17.0;
    config.render.output_path = PathBuf::from("nested/custom.png");
    let path = directory.0.join("config.json");
    config.save(&path).unwrap();
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_spectral-caustics"))
            .arg(&path)
            .current_dir(&directory.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run();
    let names = ["custom.png", "camera_direct.png", "camera_caustics.png"];
    let original: Vec<_> = names
        .iter()
        .map(|name| {
            let path = directory.0.join("nested").join(name);
            let image = image::open(&path).unwrap();
            assert_eq!((image.width(), image.height()), (12, 8));
            assert_eq!(image.color(), image::ColorType::Rgb8);
            fs::read(path).unwrap()
        })
        .collect();
    run();
    for (name, bytes) in names.iter().zip(original) {
        assert_eq!(
            fs::read(directory.0.join("nested").join(name)).unwrap(),
            bytes
        );
    }
    config.render.exposure = 0.0;
    config.render.output_path = PathBuf::from("nested/camera_direct.png");
    config.save(&path).unwrap();
    run();
    for name in [
        "camera_direct.png",
        "aux_camera_direct.png",
        "camera_caustics.png",
    ] {
        assert!(
            image::open(directory.0.join("nested").join(name))
                .unwrap()
                .into_rgb8()
                .pixels()
                .all(|p| p.0 == [0; 3])
        );
    }
    fs::write(&path, "{").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_spectral-caustics"))
        .arg(&path)
        .current_dir(&directory.0)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("读取配置"));
}

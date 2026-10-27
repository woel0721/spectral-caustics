use nalgebra::Vector3;
use spectral_caustics::{
    config::{
        Config,
        sceneconfig::{ObjectConfig, SceneConfig},
    },
    geometry::ray::Ray,
    optics::iormode::IorMode,
    render::{Render, caustics::ground_to_pixel, framebuffer::Framebuffer},
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
fn json_camera_generates_rays_that_hit_the_configured_scene() {
    let mut config = small_config();
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
fn runtime_scene_builds_configured_vertices_ground_light_receiver_and_ids() {
    let mut config = SceneConfig::default();
    if let ObjectConfig::Prism { vertices_m } = &mut config.objects[0] {
        for vertex in vertices_m {
            vertex[0] += 0.75;
            vertex[1] += 0.2;
        }
    }
    config.objects[1] = ObjectConfig::Ground { height_m: -0.5 };
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
    assert_eq!(scene.camera.fov_y_degrees, 65.0);
    assert_eq!(scene.background_color, Vector3::new(0.1, 0.2, 0.3));
    assert_eq!(scene.light.height_m, 4.0);
    assert!((scene.light.direction.norm() - 1.0).abs() < 1e-12);
    assert_eq!(scene.light.aperture_min_m.x, -1.0);
    let ray = Ray::new(
        Vector3::new(1.0, 0.4, 4.0),
        Vector3::new(0.0, 0.0, -1.0),
        0.0,
    )
    .unwrap();
    let hit = scene.intersect(&ray).unwrap();
    assert_eq!(hit.object_id, scene.prism_id);
    assert!((hit.coords - Vector3::new(1.0, 0.4, 1.4)).norm() < 1e-12);
    let outside = Ray::new(
        Vector3::new(-1.5, 0.0, 4.0),
        Vector3::new(0.0, 0.0, -1.0),
        0.0,
    )
    .unwrap();
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
    config.scene.objects = vec![ObjectConfig::Ground { height_m: -0.5 }];
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
    if let ObjectConfig::Prism { vertices_m } = &mut changed.scene.objects[0] {
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
    config.objects.push(ObjectConfig::Ground { height_m: -1.0 });
    assert!(Scene::from_config(&config).is_err());
    config = original.clone();
    config.objects[0] = ObjectConfig::Prism {
        vertices_m: [[0.0; 3]; 6],
    };
    assert!(config.validate().is_ok()); // 数据校验与几何构建职责分开。
    assert!(Scene::from_config(&config).is_err());
    config = original.clone();
    if let ObjectConfig::Prism { vertices_m } = &mut config.objects[0] {
        vertices_m.swap(0, 1);
    }
    assert!(Scene::from_config(&config).is_err());
    config = original;
    config.objects[1] = ObjectConfig::Ground { height_m: 3.0 };
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
    for name in ["no_prism.png", "debug_no_prism.png", "dispersion.png"] {
        assert!(
            image::open(directory.0.join("nested").join(name))
                .unwrap()
                .into_rgb8()
                .pixels()
                .all(|p| p.0 == [0; 3])
        );
    }
    fs::write(&path, "{").unwrap();
    let output = run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("读取配置"));
}

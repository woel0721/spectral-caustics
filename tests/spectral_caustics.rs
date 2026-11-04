use nalgebra::Vector3;
use rand::{RngCore, SeedableRng, rngs::StdRng};
use spectral_caustics::{
    config::{
        Config,
        sceneconfig::{EnvironmentMisHeuristic, ObjectConfig},
    },
    geometry::{hit::Hit, object::Hittable, ray::Ray},
    material::lambertian::Lambertian,
    optics::{
        iormode::IorMode,
        spectrum::{cie_xyz, d65_normalization, d65_spd, rgb_reflectance},
    },
    render::{
        Render, SpectralCausticsMap, SpectralPathState, caustics::render_wavelength_caustics,
        framebuffer::Framebuffer, render_spectral_camera_components,
        render_spectral_camera_with_packet, trace_spectral_camera_path,
        trace_spectral_camera_path_with_caustics,
    },
    scene::Scene,
};
use std::{f64::consts::PI, process::Command};

fn config() -> Config {
    let mut c = Config::from_json(include_str!(
        "../configs/vertical_prism_spectral_caustics.json"
    ))
    .unwrap();
    c.render.width = 24;
    c.render.height = 24;
    c.render.samples_per_pixel_axis = 1;
    c.render.camera_spp = 16;
    c.render.thread_count = Some(1);
    c.scene.environment.as_mut().unwrap().map_path = None;
    c
}

fn same(a: &Framebuffer, b: &Framebuffer) {
    for (a, b) in a.pixels.iter().zip(&b.pixels) {
        for (a, b) in a.iter().zip(b.iter()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }
}

fn center(map: &SpectralCausticsMap, index: usize) -> Vector3<f64> {
    let (w, h) = map.dimensions();
    let r = map.receiver();
    let span = r.max_m - r.min_m;
    Vector3::new(
        r.min_m.x + (index % w) as f64 * span.x / w as f64 + span.x / w as f64 * 0.5,
        r.max_m.y - ((index / w) as f64 + 0.5) * span.y / h as f64,
        0.0,
    )
}

fn centroid(map: &SpectralCausticsMap, layer: usize) -> Vector3<f64> {
    let values = map.layer_irradiance(layer).unwrap();
    values
        .iter()
        .enumerate()
        .map(|(i, &e)| center(map, i) * e)
        .sum::<Vector3<f64>>()
        / values.iter().sum::<f64>()
}

#[test]
fn scalar_nodes_reintegrate_to_original_xyz_flux_and_resolve_dispersion() {
    let mut c = config();
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        let scene = Scene::from_config(&c.scene).unwrap();
        let atlas = render_wavelength_caustics(&scene, &c.render)
            .unwrap()
            .unwrap();
        let rgb = Render::new(c.render.clone())
            .unwrap()
            .render_lighting(&scene)
            .unwrap();
        let mut integral = vec![Vector3::zeros(); rgb.caustics.pixels.len()];
        for (j, &(nm, width)) in atlas.wavelength_samples().iter().enumerate() {
            for (xyz, e) in integral.iter_mut().zip(atlas.layer_irradiance(j).unwrap()) {
                *xyz += cie_xyz(nm).unwrap() * (*e * width);
            }
        }
        for (a, b) in integral.iter().zip(&rgb.caustics.pixels) {
            assert!((a - b).norm() < 2e-12 * (1.0 + b.norm()));
        }
        let area = scene.receiver.area() / integral.len() as f64;
        let flux = integral.iter().map(|xyz| xyz.y * area).sum::<f64>();
        assert!(flux > 0.0 && flux < scene.light.as_parallel().unwrap().total_flux);
        let nodes: Vec<_> = [450.0, 550.0, 650.0]
            .map(|nm| {
                atlas
                    .wavelength_samples()
                    .iter()
                    .position(|s| s.0 == nm)
                    .unwrap()
            })
            .into();
        let positions: Vec<_> = nodes.iter().map(|&i| centroid(&atlas, i)).collect();
        if mode == IorMode::Constant {
            assert!((positions[0] - positions[2]).norm() < 1e-12);
        } else {
            assert!(positions[0].x > positions[1].x && positions[1].x > positions[2].x);
            assert!((positions[0] - positions[2]).norm() > 0.09);
        }
    }
}

#[test]
fn source_flux_does_not_gain_a_wavelength_width_or_spatial_sample_count_factor() {
    let mut c = config();
    c.render.mode = IorMode::Constant;
    let scene = Scene::from_config(&c.scene).unwrap();
    let reference = render_wavelength_caustics(&scene, &c.render)
        .unwrap()
        .unwrap();
    let geometric_flux =
        |map: &SpectralCausticsMap,
         settings: &spectral_caustics::config::rendersetting::RenderSetting| {
            let node = map
                .wavelength_samples()
                .iter()
                .position(|s| s.0 == 550.0)
                .unwrap();
            map.layer_irradiance(node).unwrap().iter().sum::<f64>() * scene.receiver.area()
                / (settings.width * settings.height) as f64
                / (d65_normalization(map.wavelength_samples()).unwrap() * d65_spd(550.0).unwrap())
        };
    let expected = geometric_flux(&reference, &c.render);
    for step in [5.0, 10.0, 17.0] {
        c.render.wavelength_min_nm = 380.0;
        c.render.wavelength_step_nm = step;
        for axis in [1, 2] {
            c.render.samples_per_pixel_axis = axis;
            let map = render_wavelength_caustics(&scene, &c.render)
                .unwrap()
                .unwrap();
            assert!((geometric_flux(&map, &c.render) / expected - 1.0).abs() < 2e-4);
        }
    }
    c.scene.light.as_parallel_mut().unwrap().total_flux *= 2.0;
    let doubled = Scene::from_config(&c.scene).unwrap();
    c.render.samples_per_pixel_axis = 1;
    c.render.wavelength_step_nm = 10.0;
    let map = render_wavelength_caustics(&doubled, &c.render)
        .unwrap()
        .unwrap();
    for i in 0..map.wavelength_samples().len() {
        for (&a, &b) in map
            .layer_irradiance(i)
            .unwrap()
            .iter()
            .zip(reference.layer_irradiance(i).unwrap())
        {
            assert_eq!(a, 2.0 * b);
        }
    }
}

struct Transmit;
impl RngCore for Transmit {
    fn next_u64(&mut self) -> u64 {
        u64::MAX
    }
    fn next_u32(&mut self) -> u32 {
        u32::MAX
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        dest.fill(255);
    }
}

// 仅拦截向上的 shadow ray，保证已预计算的折射可见性不被二次遮挡。
struct ShadowBlocker;
impl Hittable for ShadowBlocker {
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        (ray.direction.z > 0.0).then(|| Hit {
            coords: ray.at(1e-3),
            normal: -Vector3::z(),
            distance: 1e-3,
            ..Hit::default()
        })
    }
}

#[test]
fn glass_prefix_receives_caustics_once_with_scalar_throughput_and_no_shadow_or_rng_draws() {
    let mut c = config();
    c.scene.environment = None;
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        let mut scene = Scene::from_config(&c.scene).unwrap();
        let map = render_wavelength_caustics(&scene, &c.render)
            .unwrap()
            .unwrap();
        let l = scene.light.as_parallel().unwrap();
        let ray = Ray::new(
            Vector3::new(
                (l.aperture_min_m.x + l.aperture_max_m.x) * 0.5,
                (l.aperture_min_m.y + l.aperture_max_m.y) * 0.5,
                l.height_m,
            ),
            l.direction,
        )
        .unwrap();
        let landing = scene.trace_caustics_photon(ray, Some(550.0), mode).unwrap();
        let e = map.sample(landing.position, 550.0).unwrap();
        assert!(e > 0.0);
        scene.add_with_material(
            Box::new(ShadowBlocker),
            Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
        );
        let mut without = SpectralPathState::new(ray, 550.0, 0).unwrap();
        without.throughput = 0.37;
        let mut with = without.clone();
        let mut env = StdRng::seed_from_u64(43);
        let mut cont = StdRng::seed_from_u64(44);
        let (mut env2, mut cont2) = (env.clone(), cont.clone());
        trace_spectral_camera_path(
            &scene,
            &mut without,
            mode,
            &mut Transmit,
            &mut env,
            &mut cont,
        )
        .unwrap();
        trace_spectral_camera_path_with_caustics(
            &scene,
            &mut with,
            mode,
            Some(&map),
            &mut Transmit,
            &mut env2,
            &mut cont2,
        )
        .unwrap();
        assert_eq!(with.delta_events(), 2);
        assert_eq!(with.radiance, without.radiance);
        assert_eq!(with.radiance, 0.0);
        assert_eq!(with.throughput, without.throughput);
        assert_eq!(env.next_u64(), env2.next_u64());
        assert_eq!(cont.next_u64(), cont2.next_u64());
        let expected =
            with.throughput * rgb_reflectance(Vector3::repeat(0.18), 550.0).unwrap() / PI * e;
        assert!((with.caustics_radiance / expected - 1.0).abs() < 1e-10);
        assert_eq!(
            with.combined_radiance(),
            without.radiance + with.caustics_radiance
        );
        // 已经发生过连续着色时，即使仍经过相同玻璃前缀，也不可再读取贴图。
        let mut later = SpectralPathState::new(ray, 550.0, 0).unwrap();
        later.has_non_delta_vertex = true;
        trace_spectral_camera_path_with_caustics(
            &scene,
            &mut later,
            mode,
            Some(&map),
            &mut Transmit,
            &mut env2,
            &mut cont2,
        )
        .unwrap();
        assert_eq!(later.caustics_radiance, 0.0);
    }
}

struct Ceiling {
    z: f64,
}
impl Hittable for Ceiling {
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        let t = (self.z - ray.origin.z) / ray.direction.z;
        (t.is_finite() && t > 1e-5).then(|| Hit {
            coords: ray.at(t),
            normal: -Vector3::z(),
            distance: t,
            ..Hit::default()
        })
    }
}

#[test]
fn a_non_ground_first_diffuse_vertex_prevents_later_ground_lookup() {
    let c = config();
    let mut scene = Scene::from_config(&c.scene).unwrap();
    scene.environment = None;
    let map = render_wavelength_caustics(&scene, &c.render)
        .unwrap()
        .unwrap();
    let node = map
        .wavelength_samples()
        .iter()
        .position(|s| s.0 == 550.0)
        .unwrap();
    let (i, _) = map
        .layer_irradiance(node)
        .unwrap()
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap();
    let p = center(&map, i);
    assert!(map.sample(p, 550.0).unwrap() > 0.0);
    scene.add_with_material(
        Box::new(Ceiling { z: 2.0 }),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    let mut state =
        SpectralPathState::new(Ray::new(p + Vector3::z(), Vector3::z()).unwrap(), 550.0, 1)
            .unwrap();
    struct Zero;
    impl RngCore for Zero {
        fn next_u64(&mut self) -> u64 {
            0
        }
        fn next_u32(&mut self) -> u32 {
            0
        }
        fn fill_bytes(&mut self, d: &mut [u8]) {
            d.fill(0);
        }
    }
    trace_spectral_camera_path_with_caustics(
        &scene,
        &mut state,
        c.render.mode,
        Some(&map),
        &mut Zero,
        &mut Zero,
        &mut Zero,
    )
    .unwrap();
    assert_eq!(state.non_delta_bounces, 1);
    assert!(state.ray.direction.z < 0.0);
    assert_eq!(state.caustics_radiance, 0.0);
}

#[test]
fn spectral_components_preserve_direct_crn_additivity_and_thread_determinism() {
    let mut c = config();
    c.render.width = 12;
    c.render.height = 12;
    let dir = std::env::temp_dir().join(format!("hero-caustics-mis-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hdr = dir.join("test.hdr");
    let pixels: Vec<_> = (0..32)
        .map(|i| {
            image::Rgb(if i % 3 == 0 {
                [4.0_f32, 0.2, 1.0]
            } else {
                [0.1; 3]
            })
        })
        .collect();
    image::codecs::hdr::HdrEncoder::new(std::fs::File::create(&hdr).unwrap())
        .encode(&pixels, 8, 4)
        .unwrap();
    c.scene.environment.as_mut().unwrap().map_path = Some(hdr);
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        for packet in [1, 4] {
            c.render.spectral_packet_size = packet;
            for heuristic in [
                EnvironmentMisHeuristic::Balance,
                EnvironmentMisHeuristic::Power,
            ] {
                c.scene.environment_mis_heuristic = heuristic;
                for depth in [0, 4] {
                    c.scene.max_diffuse_bounces = depth;
                    let scene = Scene::from_config(&c.scene).unwrap();
                    let mut reference: Option<spectral_caustics::render::CameraLightingImages> =
                        None;
                    for threads in [1, 2, 4, 1] {
                        c.render.thread_count = Some(threads);
                        let images = Render::new(c.render.clone())
                            .unwrap()
                            .render_spectral_camera_components(&scene)
                            .unwrap();
                        assert!(images.caustics.pixels.iter().any(|p| p.y > 0.0));
                        for ((d, k), sum) in images
                            .direct
                            .pixels
                            .iter()
                            .zip(&images.caustics.pixels)
                            .zip(&images.combined.pixels)
                        {
                            assert_eq!(*d + *k, *sum);
                        }
                        if let Some(old) = &reference {
                            same(&images.direct, &old.direct);
                            same(&images.caustics, &old.caustics);
                            same(&images.combined, &old.combined);
                        }
                        reference = Some(images);
                    }
                    let old = render_spectral_camera_with_packet(
                        &scene,
                        c.render.width,
                        c.render.height,
                        c.render.camera_spp,
                        packet,
                        c.render.seed,
                        mode,
                    )
                    .unwrap();
                    same(&reference.unwrap().direct, &old);
                }
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn hero_packet_integral_matches_independent_fine_wavelength_quadrature() {
    let mut c = config();
    let mut scene = Scene::from_config(&c.scene).unwrap();
    let map = render_wavelength_caustics(&scene, &c.render)
        .unwrap()
        .unwrap();
    let node = map
        .wavelength_samples()
        .iter()
        .position(|s| s.0 == 550.0)
        .unwrap();
    let (i, _) = map
        .layer_irradiance(node)
        .unwrap()
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap();
    let p = center(&map, i);
    c.scene.camera.position = (p + Vector3::z()).into();
    c.scene.camera.target = p.into();
    c.scene.camera.up = [0.0, 1.0, 0.0];
    c.scene.camera.fov_y_degrees = 1e-6;
    scene.camera = spectral_caustics::camera::Camera::from_config(&c.scene.camera).unwrap();
    c.render.width = 1;
    c.render.height = 1;
    c.render.camera_spp = 4096;
    let mut reference = Vector3::zeros();
    for i in 0..4000 {
        let nm = 380.0 + (f64::from(i) + 0.5) * 0.1;
        reference += cie_xyz(nm).unwrap()
            * (rgb_reflectance(Vector3::repeat(0.18), nm).unwrap() / PI
                * map.sample(p, nm).unwrap()
                * 0.1);
    }
    assert!(reference.y > 0.0);
    for packet in [1, 4] {
        c.render.spectral_packet_size = packet;
        let image = render_spectral_camera_components(&scene, &c.render, Some(&map)).unwrap();
        assert!((image.caustics.pixels[0] - reference).norm() / reference.norm() < 1e-4);
    }
}

#[test]
fn disabled_or_absent_source_preserves_old_spectral_camera_and_skips_atlas() {
    let mut c = config();
    if let ObjectConfig::VerticalPrism { caustics, .. } = &mut c.scene.objects[0] {
        *caustics = false;
    }
    for remove in [false, true] {
        let mut scene_config = c.scene.clone();
        if remove {
            scene_config.objects.remove(0);
            scene_config.debug_object_id = None;
        }
        let scene = Scene::from_config(&scene_config).unwrap();
        assert!(
            render_wavelength_caustics(&scene, &c.render)
                .unwrap()
                .is_none()
        );
        let old = render_spectral_camera_with_packet(
            &scene,
            c.render.width,
            c.render.height,
            c.render.camera_spp,
            4,
            c.render.seed,
            c.render.mode,
        )
        .unwrap();
        let renderer = Render::new(c.render.clone()).unwrap();
        same(&old, &renderer.render_camera(&scene).unwrap());
        let images = renderer.render_spectral_camera_components(&scene).unwrap();
        same(&old, &images.direct);
        same(&old, &images.combined);
        assert!(
            images
                .caustics
                .pixels
                .iter()
                .all(|v| *v == Vector3::zeros())
        );
    }
}

#[test]
fn ground_sun_debug_preserves_source_maps_caustics_rgb_and_packet_thread_results() {
    let mut c = config();
    c.render.width = 12;
    c.render.height = 12;
    c.scene.max_diffuse_bounces = 4;
    let scene = Scene::from_config(&c.scene).unwrap();
    let map = render_wavelength_caustics(&scene, &c.render)
        .unwrap()
        .unwrap();
    let rgb_source = Render::new(c.render.clone())
        .unwrap()
        .render_lighting(&scene)
        .unwrap();
    c.render.spectral_debug_disable_ground_sun = true;
    let debug_map = render_wavelength_caustics(&scene, &c.render)
        .unwrap()
        .unwrap();
    for i in 0..map.wavelength_samples().len() {
        assert_eq!(map.layer_irradiance(i), debug_map.layer_irradiance(i));
    }
    let debug_source = Render::new(c.render.clone())
        .unwrap()
        .render_lighting(&scene)
        .unwrap();
    same(&rgb_source.direct, &debug_source.direct);
    same(&rgb_source.caustics, &debug_source.caustics);
    for packet in [1, 4] {
        c.render.spectral_packet_size = packet;
        c.render.spectral_debug_disable_ground_sun = false;
        let baseline = render_spectral_camera_components(&scene, &c.render, Some(&map)).unwrap();
        c.render.spectral_debug_disable_ground_sun = true;
        let mut reference: Option<spectral_caustics::render::CameraLightingImages> = None;
        for threads in [1, 2, 4, 1] {
            c.render.thread_count = Some(threads);
            let images = Render::new(c.render.clone())
                .unwrap()
                .render_camera(&scene)
                .unwrap();
            let split = render_spectral_camera_components(&scene, &c.render, Some(&map)).unwrap();
            same(&images, &split.combined);
            same(&baseline.caustics, &split.caustics);
            assert!(split.caustics.pixels.iter().any(|p| p.y > 0.0));
            assert!(
                split.direct.pixels.iter().any(|p| p.y > 0.0),
                "保留环境照明"
            );
            assert!(
                split
                    .direct
                    .pixels
                    .iter()
                    .zip(&baseline.direct.pixels)
                    .all(|(d, b)| d.y < b.y)
            );
            for ((d, k), sum) in split
                .direct
                .pixels
                .iter()
                .zip(&split.caustics.pixels)
                .zip(&split.combined.pixels)
            {
                assert_eq!(*d + *k, *sum);
            }
            if let Some(old) = &reference {
                same(&split.direct, &old.direct);
                same(&split.combined, &old.combined);
            }
            reference = Some(split);
        }
    }
    c.render.camera_mode = spectral_caustics::config::rendersetting::CameraMode::Rgb;
    c.render.spectral_debug_disable_ground_sun = false;
    let baseline = Render::new(c.render.clone())
        .unwrap()
        .render_camera_glass_components(&scene, &rgb_source, c.render.camera_spp, c.render.seed)
        .unwrap();
    c.render.spectral_debug_disable_ground_sun = true;
    let debug = Render::new(c.render.clone())
        .unwrap()
        .render_camera_glass_components(&scene, &rgb_source, c.render.camera_spp, c.render.seed)
        .unwrap();
    same(&baseline.direct, &debug.direct);
    same(&baseline.caustics, &debug.caustics);
    same(&baseline.combined, &debug.combined);
    // 没有焦散目标时的单图入口也应传递调试开关，不依赖光源图集。
    c.render.camera_mode = spectral_caustics::config::rendersetting::CameraMode::Spectral;
    c.scene.environment = None;
    if let ObjectConfig::VerticalPrism { caustics, .. } = &mut c.scene.objects[0] {
        *caustics = false;
    }
    let scene = Scene::from_config(&c.scene).unwrap();
    let dark = Render::new(c.render.clone())
        .unwrap()
        .render_camera(&scene)
        .unwrap();
    assert!(dark.pixels.iter().all(|p| *p == Vector3::zeros()));
    c.render.spectral_debug_disable_ground_sun = false;
    let lit = Render::new(c.render)
        .unwrap()
        .render_camera(&scene)
        .unwrap();
    assert!(lit.pixels.iter().any(|p| p.y > 0.0));
}

#[test]
fn enabled_cli_writes_three_components_and_map_mismatches_fail_explicitly() {
    let mut c = config();
    let scene = Scene::from_config(&c.scene).unwrap();
    let map = render_wavelength_caustics(&scene, &c.render)
        .unwrap()
        .unwrap();
    let mut invalid = c.render.clone();
    invalid.mode = IorMode::Constant;
    assert!(render_spectral_camera_components(&scene, &invalid, Some(&map)).is_err());
    let mut invalid_scene = Scene::from_config(&c.scene).unwrap();
    invalid_scene.receiver.max_m.x = invalid_scene.receiver.min_m.x;
    assert!(render_wavelength_caustics(&invalid_scene, &c.render).is_err());
    let dir = std::env::temp_dir().join(format!("hero-caustics-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    c.render.output_path = dir.join("hero.png");
    c.save(dir.join("config.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_spectral-caustics"))
        .arg(dir.join("config.json"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    for name in [
        "hero.png",
        "camera_direct.png",
        "camera_caustics.png",
        "camera_combined.png",
    ] {
        assert_eq!(
            image::open(dir.join(name))
                .unwrap()
                .into_rgb8()
                .dimensions(),
            (24, 24)
        );
    }
    assert_eq!(
        std::fs::read(dir.join("hero.png")).unwrap(),
        std::fs::read(dir.join("camera_combined.png")).unwrap()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

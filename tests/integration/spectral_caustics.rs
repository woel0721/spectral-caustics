use crate::support::{
    TempDir, TransmissionRng as Transmit, assert_camera_images_bits_eq, assert_framebuffer_bits_eq,
};
use nalgebra::Vector3;
use rand::RngCore;
use rand::SeedableRng;
use rand::rngs::StdRng;
use spectral_caustics::caustics::SpectralCausticsMap;
use spectral_caustics::config::Config;
#[cfg(feature = "diagnostics")]
use spectral_caustics::config::RenderJob;
use spectral_caustics::config::scene::EnvironmentMisHeuristic;
#[cfg(feature = "diagnostics")]
use spectral_caustics::config::scene::ObjectConfig;
use spectral_caustics::geometry::hit::GeometryHit;
use spectral_caustics::geometry::object::Hittable;
use spectral_caustics::geometry::ray::Ray;
use spectral_caustics::integrator::camera::{
    path::trace_camera_path,
    transport::Caustics,
    types::{CameraPathRngs, CameraPathState},
};
use spectral_caustics::material::lambertian::Lambertian;
use spectral_caustics::optics::dispersion::IorMode;
use spectral_caustics::renderer::Renderer;
use spectral_caustics::spectrum::cie_xyz;
use spectral_caustics::spectrum::d65_normalization;
use spectral_caustics::spectrum::d65_spd;
use spectral_caustics::spectrum::rgb_reflectance;
use std::f64::consts::PI;
use std::process::Command;

fn config() -> Config {
    let mut c = Config::from_json(include_str!(
        "../../configs/vertical_prism_spectral_caustics.json"
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
        .map(|(i, e)| center(map, i) * e)
        .sum::<Vector3<f64>>()
        / values.iter().sum::<f64>()
}

#[test]
fn scalar_nodes_reintegrate_to_original_xyz_flux_and_resolve_dispersion() {
    let mut c = config();
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        let scene = c.build().unwrap();
        let atlas = spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
            .unwrap()
            .unwrap();
        let rgb = spectral_caustics::integrator::photon::integrate_xyz(
            &scene,
            scene.photon.caustics_object_id,
            scene.photon.ground_id.expect("receiver"),
            &c.render,
        )
        .unwrap();
        let mut integral = vec![Vector3::zeros(); rgb.caustics.pixels().len()];
        for (j, &(nm, width)) in atlas.wavelength_samples().iter().enumerate() {
            for (xyz, e) in integral.iter_mut().zip(atlas.layer_irradiance(j).unwrap()) {
                *xyz += cie_xyz(nm).unwrap() * (e * width);
            }
        }
        for (a, b) in integral.iter().zip(rgb.caustics.pixels()) {
            assert!((a - b).norm() < 2e-12 * (1.0 + b.norm()));
        }
        let area = scene.photon.receiver.area() / integral.len() as f64;
        let flux = integral.iter().map(|xyz| xyz.y * area).sum::<f64>();
        assert!(flux > 0.0 && flux < scene.scene.light.as_parallel().unwrap().total_flux);
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
    let scene = c.build().unwrap();
    let reference = spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
        .unwrap()
        .unwrap();
    let geometric_flux =
        |map: &SpectralCausticsMap, settings: &spectral_caustics::config::render::RenderSetting| {
            let node = map
                .wavelength_samples()
                .iter()
                .position(|s| s.0 == 550.0)
                .unwrap();
            map.layer_irradiance(node).unwrap().iter().sum::<f64>() * scene.photon.receiver.area()
                / (settings.width * settings.height) as f64
                / (d65_normalization(map.wavelength_samples()).unwrap() * d65_spd(550.0).unwrap())
        };
    // For each spatial budget, changing quadrature nodes must leave the geometric flux unchanged.
    // Different budgets use different stochastic Fresnel events; exact cross-budget equality is invalid.
    for axis in [1, 2] {
        c.render.samples_per_pixel_axis = axis;
        c.render.wavelength_step_nm = 10.0;
        let base = spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
            .unwrap()
            .unwrap();
        let expected_axis = geometric_flux(&base, &c.render);
        assert!(expected_axis > 0.0 && expected_axis <= scene.scene.light.total_flux());
        for step in [5.0, 10.0, 17.0] {
            c.render.wavelength_step_nm = step;
            let map = spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
                .unwrap()
                .unwrap();
            assert!((geometric_flux(&map, &c.render) / expected_axis - 1.0).abs() < 1e-12);
        }
    }
    c.scene.light.as_parallel_mut().unwrap().total_flux *= 2.0;
    let doubled = c.build().unwrap();
    c.render.samples_per_pixel_axis = 1;
    c.render.wavelength_step_nm = 10.0;
    let map = spectral_caustics::integrator::photon::integrate_spectral(&doubled, &c.render)
        .unwrap()
        .unwrap();
    for i in 0..map.wavelength_samples().len() {
        for (a, b) in map
            .layer_irradiance(i)
            .unwrap()
            .iter()
            .zip(reference.layer_irradiance(i).unwrap())
        {
            assert_eq!(a, 2.0 * b);
        }
    }
}

// 仅拦截向上的 shadow ray，保证已预计算的折射可见性不被二次遮挡。
struct ShadowBlocker;
impl Hittable for ShadowBlocker {
    fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
        (ray.direction.z > 0.0).then(|| GeometryHit {
            coords: ray.at(1e-3),
            normal: -Vector3::z(),
            distance: 1e-3,
        })
    }
}

struct Ceiling {
    z: f64,
}
impl Hittable for Ceiling {
    fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
        let t = (self.z - ray.origin.z) / ray.direction.z;
        (t.is_finite() && t > 1e-5).then(|| GeometryHit {
            coords: ray.at(t),
            normal: -Vector3::z(),
            distance: t,
        })
    }
}

#[test]
fn a_non_ground_first_diffuse_vertex_prevents_later_ground_lookup() {
    let c = config();
    let mut scene = c.build().unwrap();
    scene.scene.environment = None;
    let map = spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
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
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    let p = center(&map, i);
    assert!(map.sample(p, 550.0).unwrap() > 0.0);
    scene.scene.add(
        Box::new(Ceiling { z: 2.0 }),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    let mut state =
        CameraPathState::spectral(Ray::new(p + Vector3::z(), Vector3::z()).unwrap(), 550.0, 1)
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
    trace_camera_path(
        &scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut Zero,
            environment: &mut Zero,
            diffuse: &mut Zero,
            medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
        },
        scene
            .scene
            .light
            .analytic_sun()
            .unwrap()
            .unwrap_or((Vector3::z(), 0.0))
            .0,
        scene
            .scene
            .light
            .analytic_sun()
            .unwrap()
            .unwrap_or((Vector3::z(), 0.0))
            .1,
        (Some(&map)).map_or(Caustics::None, Caustics::Spectral),
        c.render.mode,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();
    assert_eq!(state.non_delta_bounces, 1);
    assert!(state.ray.direction.z < 0.0);
    assert_eq!(state.radiance.caustics, 0.0);
}

#[test]
fn spectral_components_preserve_direct_crn_additivity_and_thread_determinism() {
    let mut c = config();
    c.render.width = 8;
    c.render.height = 8;
    c.render.camera_spp = 8;
    c.render.wavelength_step_nm = 50.0;
    let dir = TempDir::new("hero-caustics-mis");
    let pixels: Vec<_> = (0..32)
        .map(|i| {
            if i % 3 == 0 {
                [4.0_f32, 0.2, 1.0]
            } else {
                [0.1; 3]
            }
        })
        .collect();
    let hdr = dir.write_hdr("test.hdr", 8, 4, &pixels);
    c.scene.environment.as_mut().unwrap().map_path = Some(hdr);
    for (mode, packet, heuristic, depth) in [
        (IorMode::Constant, 1, EnvironmentMisHeuristic::Balance, 0),
        (IorMode::Constant, 4, EnvironmentMisHeuristic::Power, 4),
        (IorMode::Dispersion, 1, EnvironmentMisHeuristic::Power, 4),
        (IorMode::Dispersion, 4, EnvironmentMisHeuristic::Balance, 0),
    ] {
        c.render.mode = mode;
        c.render.spectral_packet_size = packet;
        c.scene.environment_mis_heuristic = heuristic;
        c.scene.max_diffuse_bounces = depth;
        let scene = c.build().unwrap();
        let mut reference: Option<spectral_caustics::integrator::camera::CameraLightingImages> =
            None;
        for threads in [1, 2, 4] {
            c.render.thread_count = Some(threads);
            let images = Renderer::new(c.render.clone())
                .unwrap()
                .render(&scene)
                .unwrap();
            assert!(images.caustics.pixels().iter().any(|p| p.y > 0.0));
            for ((d, k), sum) in images
                .direct
                .pixels()
                .iter()
                .zip(images.caustics.pixels())
                .zip(images.combined.pixels())
            {
                assert_eq!(*d + *k, *sum);
            }
            if let Some(old) = &reference {
                assert_camera_images_bits_eq(&images, old);
            }
            reference = Some(images);
        }
        let old = spectral_caustics::integrator::camera::integrate_spectral(
            &scene,
            &spectral_caustics::config::render::RenderSetting {
                width: c.render.width,
                height: c.render.height,
                camera_spp: c.render.camera_spp,
                spectral_packet_size: packet,
                seed: c.render.seed,
                mode,
                ..Default::default()
            },
            None,
        )
        .map(|images| images.combined)
        .unwrap();
        assert_framebuffer_bits_eq(&reference.unwrap().direct, &old);
    }
}

#[test]
fn hero_packet_integral_matches_independent_fine_wavelength_quadrature() {
    let mut c = config();
    let mut scene = c.build().unwrap();
    let map = spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
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
        .max_by(|a, b| a.1.total_cmp(&b.1))
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
        let image = spectral_caustics::integrator::camera::integrate_spectral(
            &scene,
            &c.render,
            Some(&map),
        )
        .unwrap();
        assert!((image.caustics.pixels()[0] - reference).norm() / reference.norm() < 1e-4);
    }
}

#[cfg(feature = "diagnostics")]
#[test]
fn disabled_or_absent_source_preserves_old_spectral_camera_and_skips_atlas() {
    let mut c = config();
    c.render.width = 12;
    c.render.height = 12;
    c.render.camera_spp = 8;
    if let ObjectConfig::VerticalPrism { caustics, .. } = &mut c.scene.objects[0] {
        *caustics = false;
    }
    for remove in [false, true] {
        let mut scene_config = c.scene.clone();
        if remove {
            scene_config.objects.remove(0);
        }
        let scene = RenderJob::from_config(&scene_config).unwrap();
        assert!(
            spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
                .unwrap()
                .is_none()
        );
        let old = spectral_caustics::integrator::camera::integrate_spectral(
            &scene,
            &spectral_caustics::config::render::RenderSetting {
                width: c.render.width,
                height: c.render.height,
                camera_spp: c.render.camera_spp,
                spectral_packet_size: 4,
                seed: c.render.seed,
                mode: c.render.mode,
                ..Default::default()
            },
            None,
        )
        .map(|images| images.combined)
        .unwrap();
        let renderer = Renderer::new(c.render.clone()).unwrap();
        assert_framebuffer_bits_eq(
            &old,
            &renderer
                .render(&scene)
                .map(|images| images.combined)
                .unwrap(),
        );
        let images = renderer.render(&scene).unwrap();
        assert_framebuffer_bits_eq(&old, &images.direct);
        assert_framebuffer_bits_eq(&old, &images.combined);
        assert!(
            images
                .caustics
                .pixels()
                .iter()
                .all(|v| *v == Vector3::zeros())
        );
    }
}

#[cfg(feature = "diagnostics")]
#[test]
fn ground_sun_debug_preserves_source_maps_caustics_rgb_and_packet_thread_results() {
    let mut c = config();
    c.render.width = 8;
    c.render.height = 8;
    c.render.camera_spp = 8;
    c.render.wavelength_step_nm = 50.0;
    c.scene.max_diffuse_bounces = 4;
    let scene = c.build().unwrap();
    let map = spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
        .unwrap()
        .unwrap();
    let rgb_source = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        &c.render,
    )
    .unwrap();
    #[cfg(feature = "diagnostics")]
    {
        c.render.diagnostics.disable_ground_sun = true;
    }
    let debug_map = spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
        .unwrap()
        .unwrap();
    for i in 0..map.wavelength_samples().len() {
        assert_eq!(map.layer_irradiance(i), debug_map.layer_irradiance(i));
    }
    let debug_source = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        &c.render,
    )
    .unwrap();
    assert_framebuffer_bits_eq(
        rgb_source.direct.as_ref().unwrap(),
        debug_source.direct.as_ref().unwrap(),
    );
    assert_framebuffer_bits_eq(&rgb_source.caustics, &debug_source.caustics);
    for packet in [1, 4] {
        c.render.spectral_packet_size = packet;
        #[cfg(feature = "diagnostics")]
        {
            c.render.diagnostics.disable_ground_sun = false;
        }
        let baseline = spectral_caustics::integrator::camera::integrate_spectral(
            &scene,
            &c.render,
            Some(&map),
        )
        .unwrap();
        #[cfg(feature = "diagnostics")]
        {
            c.render.diagnostics.disable_ground_sun = true;
        }
        let mut reference: Option<spectral_caustics::integrator::camera::CameraLightingImages> =
            None;
        for threads in [1, 2, 4] {
            c.render.thread_count = Some(threads);
            let split = crate::support::run_in_threads(threads, || {
                spectral_caustics::integrator::camera::integrate_spectral(
                    &scene,
                    &c.render,
                    Some(&map),
                )
                .unwrap()
            });
            if threads == 1 {
                let images = Renderer::new(c.render.clone())
                    .unwrap()
                    .render(&scene)
                    .map(|images| images.combined)
                    .unwrap();
                assert_framebuffer_bits_eq(&images, &split.combined);
            }
            assert_framebuffer_bits_eq(&baseline.caustics, &split.caustics);
            assert!(split.caustics.pixels().iter().any(|p| p.y > 0.0));
            assert!(
                split.direct.pixels().iter().any(|p| p.y > 0.0),
                "保留环境照明"
            );
            assert!(
                split
                    .direct
                    .pixels()
                    .iter()
                    .zip(baseline.direct.pixels())
                    .all(|(d, b)| d.y < b.y)
            );
            for ((d, k), sum) in split
                .direct
                .pixels()
                .iter()
                .zip(split.caustics.pixels())
                .zip(split.combined.pixels())
            {
                assert_eq!(*d + *k, *sum);
            }
            if let Some(old) = &reference {
                assert_framebuffer_bits_eq(&split.direct, &old.direct);
                assert_framebuffer_bits_eq(&split.combined, &old.combined);
            }
            reference = Some(split);
        }
    }
    c.render.camera_mode = spectral_caustics::config::render::CameraMode::Rgb;
    #[cfg(feature = "diagnostics")]
    {
        c.render.diagnostics.disable_ground_sun = false;
    }
    let baseline =
        spectral_caustics::integrator::camera::integrate_rgb(&scene, &c.render, Some(&rgb_source))
            .unwrap();
    #[cfg(feature = "diagnostics")]
    {
        c.render.diagnostics.disable_ground_sun = true;
    }
    let debug =
        spectral_caustics::integrator::camera::integrate_rgb(&scene, &c.render, Some(&rgb_source))
            .unwrap();
    assert_camera_images_bits_eq(&baseline, &debug);
    // 没有焦散目标时的单图入口也应传递调试开关，不依赖光源图集。
    c.render.camera_mode = spectral_caustics::config::render::CameraMode::Spectral;
    c.scene.environment = None;
    if let ObjectConfig::VerticalPrism { caustics, .. } = &mut c.scene.objects[0] {
        *caustics = false;
    }
    let scene = c.build().unwrap();
    let dark = Renderer::new(c.render.clone())
        .unwrap()
        .render(&scene)
        .map(|images| images.combined)
        .unwrap();
    assert!(dark.pixels().iter().all(|p| *p == Vector3::zeros()));
    #[cfg(feature = "diagnostics")]
    {
        c.render.diagnostics.disable_ground_sun = false;
    }
    let lit = Renderer::new(c.render)
        .unwrap()
        .render(&scene)
        .map(|images| images.combined)
        .unwrap();
    assert!(lit.pixels().iter().any(|p| p.y > 0.0));
}

#[test]
fn enabled_cli_writes_three_components_and_map_mismatches_fail_explicitly() {
    let mut c = config();
    c.render.width = 12;
    c.render.height = 12;
    c.render.camera_spp = 4;
    let scene = c.build().unwrap();
    let map = spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
        .unwrap()
        .unwrap();
    let mut invalid = c.render.clone();
    invalid.mode = IorMode::Constant;
    assert!(
        spectral_caustics::integrator::camera::integrate_spectral(&scene, &invalid, Some(&map))
            .is_err()
    );
    let mut invalid_scene = c.build().unwrap();
    invalid_scene.photon.receiver.max_m.x = invalid_scene.photon.receiver.min_m.x;
    assert!(
        spectral_caustics::integrator::photon::integrate_spectral(&invalid_scene, &c.render)
            .is_err()
    );
    let dir = TempDir::new("hero-caustics-cli");
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
            (12, 12)
        );
    }
    assert_eq!(
        std::fs::read(dir.join("hero.png")).unwrap(),
        std::fs::read(dir.join("camera_combined.png")).unwrap()
    );
}

#[test]
fn glass_prefix_receives_caustics_once_with_scalar_throughput_and_no_shadow_or_rng_draws() {
    let mut c = config();
    c.scene.environment = None;
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        let mut scene = c.build().unwrap();
        let map = spectral_caustics::integrator::photon::integrate_spectral(&scene, &c.render)
            .unwrap()
            .unwrap();
        let l = scene.scene.light.as_parallel().unwrap();
        let ray = Ray::new(
            Vector3::new(
                (l.aperture_min_m.x + l.aperture_max_m.x) * 0.5,
                (l.aperture_min_m.y + l.aperture_max_m.y) * 0.5,
                l.height_m,
            ),
            l.direction,
        )
        .unwrap();
        let landing = spectral_caustics::integrator::photon::trace_photon(
            &scene,
            ray,
            Some(550.0),
            mode,
            32,
            &mut crate::support::TransmissionRng,
        )
        .unwrap()
        .landing()
        .unwrap();
        let e = map.sample(landing.position, 550.0).unwrap();
        assert!(e > 0.0);
        scene.scene.add(
            Box::new(ShadowBlocker),
            Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
        );
        let mut without = CameraPathState::spectral(ray, 550.0, 0).unwrap();
        without.throughput = 0.37;
        let mut with = without.clone();
        let mut env = StdRng::seed_from_u64(43);
        let mut cont = StdRng::seed_from_u64(44);
        let (mut env2, mut cont2) = (env.clone(), cont.clone());
        trace_camera_path(
            &scene,
            &mut without,
            &mut CameraPathRngs {
                camera: &mut Transmit,
                environment: &mut env,
                diffuse: &mut cont,
                medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
            },
            scene
                .scene
                .light
                .analytic_sun()
                .unwrap()
                .unwrap_or((Vector3::z(), 0.0))
                .0,
            scene
                .scene
                .light
                .analytic_sun()
                .unwrap()
                .unwrap_or((Vector3::z(), 0.0))
                .1,
            (None).map_or(Caustics::None, Caustics::Spectral),
            mode,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        trace_camera_path(
            &scene,
            &mut with,
            &mut CameraPathRngs {
                camera: &mut Transmit,
                environment: &mut env2,
                diffuse: &mut cont2,
                medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
            },
            scene
                .scene
                .light
                .analytic_sun()
                .unwrap()
                .unwrap_or((Vector3::z(), 0.0))
                .0,
            scene
                .scene
                .light
                .analytic_sun()
                .unwrap()
                .unwrap_or((Vector3::z(), 0.0))
                .1,
            (Some(&map)).map_or(Caustics::None, Caustics::Spectral),
            mode,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        assert_eq!(with.delta_events(), 2);
        assert_eq!(with.radiance.direct, without.radiance.direct);
        assert_eq!(with.radiance.direct, 0.0);
        assert_eq!(with.throughput, without.throughput);
        assert_eq!(env.next_u64(), env2.next_u64());
        assert_eq!(cont.next_u64(), cont2.next_u64());
        let expected = with.throughput * rgb_reflectance(Vector3::repeat(0.18), 550.0).unwrap()
            / PI
            * map
                .sample(scene.scene.intersect(&with.ray).unwrap().coords, 550.0)
                .unwrap();
        assert!((with.radiance.caustics / expected - 1.0).abs() < 1e-10);
        assert_eq!(
            with.combined_radiance(),
            without.radiance.direct + with.radiance.caustics
        );
        // 已经发生过连续着色时，即使仍经过相同玻璃前缀，也不可再读取贴图。
        let mut later = CameraPathState::spectral(ray, 550.0, 0).unwrap();
        later.has_non_delta_vertex = true;
        trace_camera_path(
            &scene,
            &mut later,
            &mut CameraPathRngs {
                camera: &mut Transmit,
                environment: &mut env2,
                diffuse: &mut cont2,
                medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
            },
            scene
                .scene
                .light
                .analytic_sun()
                .unwrap()
                .unwrap_or((Vector3::z(), 0.0))
                .0,
            scene
                .scene
                .light
                .analytic_sun()
                .unwrap()
                .unwrap_or((Vector3::z(), 0.0))
                .1,
            (Some(&map)).map_or(Caustics::None, Caustics::Spectral),
            mode,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        assert_eq!(later.radiance.caustics, 0.0);
    }
}

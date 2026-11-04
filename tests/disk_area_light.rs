use nalgebra::Vector3;
use rand::{Rng, RngCore, SeedableRng, rngs::StdRng};
use spectral_caustics::{
    config::{
        Config,
        rendersetting::{CameraMode, RenderSetting},
        sceneconfig::{DiskAreaLightConfig, GroundReceiverConfig, ObjectConfig, SceneConfig},
    },
    geometry::{ground::Ground, ray::Ray, sphere::Sphere},
    light::{DiskAreaLight, Light},
    material::lambertian::Lambertian,
    optics::{
        emission::d65_spectrum,
        iormode::IorMode,
        spectrum::{cie_xyz, rgb_reflectance},
    },
    render::{
        Render, SpectralPathState, caustics::render_wavelength_caustics, framebuffer::Framebuffer,
        trace_spectral_camera_path,
    },
    scene::Scene,
};
use std::f64::consts::PI;

fn config() -> Config {
    let mut c =
        Config::from_json(include_str!("../configs/vertical_prism_area_light.json")).unwrap();
    c.render.width = 12;
    c.render.height = 12;
    c.render.camera_spp = 16;
    c.render.samples_per_pixel_axis = 4;
    c.render.thread_count = Some(1);
    c
}
fn disk(radius: f64) -> DiskAreaLight {
    DiskAreaLight::new(Vector3::z() * 2.0, -Vector3::z(), radius, 20.0).unwrap()
}
fn same(a: &Framebuffer, b: &Framebuffer) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    for (a, b) in a.pixels.iter().zip(&b.pixels) {
        for (a, b) in a.iter().zip(b.iter()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }
}
fn ground_scene(radius: f64) -> Scene {
    let mut scene = Scene::new();
    scene.environment = None;
    scene.ground_checkerboard = None;
    scene.light = disk(radius).into();
    scene.ground_id = Some(scene.add_with_material(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.5)).unwrap()),
    ));
    scene
}
struct Transmit {
    draws: usize,
}
impl RngCore for Transmit {
    fn next_u64(&mut self) -> u64 {
        self.draws += 1;
        u64::MAX
    }
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
    fn fill_bytes(&mut self, d: &mut [u8]) {
        for byte in d {
            *byte = self.next_u64() as u8;
        }
    }
}

#[test]
fn disk_area_pdf_full_hemisphere_emission_and_power_match_for_every_radius() {
    for radius in [0.01, 0.1, 0.5] {
        let light = DiskAreaLight::new(
            Vector3::new(0.3, -0.2, 2.0),
            Vector3::new(1.0, 2.0, -3.0),
            radius,
            20.0,
        )
        .unwrap();
        let mut rng = StdRng::seed_from_u64(42);
        let mut radial_moment = 0.0;
        let mut cos_moment = 0.0;
        let mut cos2_moment = 0.0;
        let mut min_cos = 1.0_f64;
        let mut flux = 0.0;
        let n = 8192;
        for _ in 0..n {
            let s = light
                .sample_emission([rng.random(), rng.random()], [rng.random(), rng.random()])
                .unwrap();
            let p = s.ray.origin - light.center;
            let cos = s.normal.dot(&s.ray.direction);
            assert!(p.dot(&light.normal).abs() < 1e-13);
            assert!(p.norm() <= radius * (1.0 + 1e-12));
            assert!((s.pdf_area - 1.0 / (PI * radius * radius)).abs() / s.pdf_area < 1e-14);
            assert!((s.pdf_direction - cos / PI).abs() < 1e-15);
            let power = light.radiance().unwrap() * cos / (s.pdf_area * s.pdf_direction);
            assert!((power / light.total_flux - 1.0).abs() < 1e-13);
            flux += power / n as f64;
            radial_moment += p.norm_squared() / (radius * radius * n as f64);
            cos_moment += cos / n as f64;
            cos2_moment += cos * cos / n as f64;
            min_cos = min_cos.min(cos);
        }
        assert!((flux / 20.0 - 1.0).abs() < 1e-12);
        assert!((radial_moment - 0.5).abs() < 0.015);
        assert!((cos_moment - 2.0 / 3.0).abs() < 0.015);
        assert!((cos2_moment - 0.5).abs() < 0.015);
        assert!(min_cos < 0.08, "圆盘必须覆盖整个发光半球，不能退化为准直束");
    }
}

#[test]
fn disk_direct_matches_area_integral_inverse_square_and_both_cosines() {
    for radius in [0.01, 0.4] {
        let light = disk(radius);
        let mut estimates = Vec::new();
        for h in [1.0, 2.0] {
            let position = light.center - Vector3::z() * h;
            let mut e = 0.0;
            for i in 0..8192 {
                let s = light
                    .sample_direct(position, [(f64::from(i) + 0.5) / 8192.0, 0.37])
                    .unwrap()
                    .unwrap();
                let explicit = light.radiance().unwrap() * s.cosine_light
                    / (s.distance * s.distance * s.pdf_area);
                assert!((s.incident_weight / explicit - 1.0).abs() < 1e-14);
                e += s.incident_weight * s.wi.z / 8192.0;
            }
            // 同轴平行圆盘对轴心微面元的精确辐照度：Le*πR²/(h²+R²)。
            let expected = light.total_flux / (PI * (h * h + radius * radius));
            assert!((e / expected - 1.0).abs() < 1e-7);
            estimates.push(e);
        }
        if radius == 0.01 {
            assert!((estimates[0] / estimates[1] - 4.0).abs() < 0.001);
        }
    }
    let tilted =
        DiskAreaLight::new(Vector3::z() * 2.0, Vector3::new(0.6, 0.0, -0.8), 0.01, 20.0).unwrap();
    let s = tilted
        .sample_direct(Vector3::zeros(), [0.0, 0.0])
        .unwrap()
        .unwrap();
    assert!((s.cosine_light - 0.8).abs() < 1e-14);
    assert!((s.incident_weight - 20.0 * 0.8 / (4.0 * PI)).abs() < 1e-13);
    assert!(
        tilted
            .sample_direct(Vector3::z() * 3.0, [0.0, 0.0])
            .unwrap()
            .is_none()
    );
    assert!(tilted.sample_direct(tilted.center, [0.0, 0.0]).is_err());
}

#[test]
fn disk_config_and_legacy_parallel_json_roundtrip_and_reject_invalid_inputs() {
    let c = config();
    assert!(matches!(
        Scene::from_config(&c.scene).unwrap().light,
        Light::DiskArea(_)
    ));
    assert_eq!(Config::from_json(&c.to_json().unwrap()).unwrap(), c);
    let legacy =
        Config::from_json(include_str!("../configs/vertical_prism_caustics.json")).unwrap();
    let mut tagged = serde_json::to_value(&legacy).unwrap();
    assert_eq!(tagged["scene"]["light"]["type"], "parallel");
    assert_eq!(Config::from_json(&tagged.to_string()).unwrap(), legacy);
    tagged["scene"]["light"]
        .as_object_mut()
        .unwrap()
        .remove("type");
    assert_eq!(Config::from_json(&tagged.to_string()).unwrap(), legacy);
    let base = serde_json::to_value(&c).unwrap();
    for (field, value) in [
        ("radius", serde_json::json!(0)),
        ("radius", serde_json::json!(-0.1)),
        ("normal", serde_json::json!([0, 0, 0])),
        ("total_flux", serde_json::json!(-1)),
        ("type", serde_json::json!("point")),
        ("unexpected", serde_json::json!(true)),
    ] {
        let mut bad = base.clone();
        bad["scene"]["light"][field] = value;
        assert!(Config::from_json(&bad.to_string()).is_err(), "{field}");
    }
    for radius in [f64::NAN, f64::INFINITY, 1e-200, 1e200] {
        assert!(DiskAreaLight::new(Vector3::zeros(), Vector3::z(), radius, 1.0).is_err());
    }
    let mut bad = c;
    bad.scene.light_samples = 0;
    assert!(bad.validate().is_err());
    bad.scene.light_samples = usize::MAX;
    assert!(bad.validate().is_err());
}

#[test]
fn emitted_ground_flux_matches_fixed_spd_and_quadrature_for_all_radii_and_counts() {
    let mut scene_config = SceneConfig {
        environment: None,
        ground_checkerboard: None,
        objects: vec![ObjectConfig::Ground {
            height_m: 0.0,
            material: Default::default(),
        }],
        receiver: GroundReceiverConfig {
            min_m: [-1e6; 2],
            max_m: [1e6; 2],
        },
        ..SceneConfig::default()
    };
    let mut settings = RenderSetting {
        width: 12,
        height: 10,
        camera_spp: 4,
        samples_per_pixel_axis: 1,
        ..Default::default()
    };
    for radius in [0.01, 0.5] {
        scene_config.light = DiskAreaLightConfig {
            center: [0.0, 0.0, 2.0],
            normal: [0.0, 0.0, -1.0],
            radius,
            total_flux: 20.0,
        }
        .into();
        let scene = Scene::from_config(&scene_config).unwrap();
        for axis in [1, 3] {
            settings.samples_per_pixel_axis = axis;
            for step in [5.0, 17.0] {
                settings.wavelength_step_nm = step;
                let light = Render::new(settings.clone())
                    .unwrap()
                    .render_lighting(&scene)
                    .unwrap();
                let flux = light.direct.pixels.iter().map(|v| v.y).sum::<f64>()
                    * scene.receiver.area()
                    / (settings.width * settings.height) as f64;
                let quadrature_y: f64 = settings
                    .wavelength_samples()
                    .unwrap()
                    .iter()
                    .map(|&(nm, width)| cie_xyz(nm).unwrap().y * d65_spectrum(nm).unwrap() * width)
                    .sum();
                assert!((quadrature_y - 1.0).abs() < 0.003);
                assert!(
                    (flux / (20.0 * quadrature_y) - 1.0).abs() < 2e-12,
                    "r={radius},axis={axis},step={step}: {flux}"
                );
                assert!(light.caustics.pixels.iter().all(|p| *p == Vector3::zeros()));
                assert_eq!(light.sun_irradiance_xyz, Vector3::zeros());
            }
        }
    }
}

#[test]
fn disk_shadow_uses_finite_segment_and_soft_shadow_converges_to_quadrature() {
    let mut scene = ground_scene(0.6);
    scene.light_samples = 8192;
    let ray = Ray::new(Vector3::z() * 0.1, -Vector3::z()).unwrap();
    let run = |scene: &Scene| {
        let mut state = SpectralPathState::new(ray, 550.0, 0).unwrap();
        trace_spectral_camera_path(
            scene,
            &mut state,
            IorMode::Constant,
            &mut Transmit { draws: 0 },
            &mut StdRng::seed_from_u64(43),
            &mut StdRng::seed_from_u64(44),
        )
        .unwrap();
        state.radiance
    };
    let clear = run(&scene);
    // 这颗球完全位于光源后方，不得被无限长 shadow ray 错误计作遮挡。
    scene.add_with_material(
        Box::new(Sphere::new(Vector3::z() * 3.0, 0.2).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.5)).unwrap()),
    );
    assert_eq!(clear, run(&scene));
    let center = Vector3::new(0.2, 0.0, 1.0);
    scene.add_with_material(
        Box::new(Sphere::new(center, 0.15).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.5)).unwrap()),
    );
    let estimate = run(&scene);
    let light = scene.light.as_disk().unwrap();
    let mut expected = 0.0;
    for i in 0..128 {
        for j in 0..128 {
            let s = light
                .sample_direct(
                    Vector3::zeros(),
                    [(f64::from(i) + 0.5) / 128.0, (f64::from(j) + 0.5) / 128.0],
                )
                .unwrap()
                .unwrap();
            // 独立解析球/线段距离判定，不调用渲染器的可见性逻辑。
            let t = center.dot(&s.wi);
            let blocked =
                t > 0.0 && t < s.distance && (center - s.wi * t).norm_squared() < 0.15_f64.powi(2);
            if !blocked {
                expected += s.incident_weight * s.wi.z / (128.0 * 128.0);
            }
        }
    }
    expected *=
        rgb_reflectance(Vector3::repeat(0.5), 550.0).unwrap() / PI * d65_spectrum(550.0).unwrap();
    assert!(estimate > 0.0 && estimate < clear);
    assert!(
        (estimate / expected - 1.0).abs() < 0.025,
        "{estimate} vs {expected}"
    );
}

#[test]
fn disk_prism_center_ray_dispersion_and_source_extent_blur_are_physical() {
    let c = config();
    let scene = Scene::from_config(&c.scene).unwrap();
    let base = *scene.light.as_disk().unwrap();
    let center_ray = Ray::new(base.center, base.normal).unwrap();
    let a = scene
        .trace_caustics_photon(center_ray, Some(450.0), IorMode::Dispersion)
        .unwrap();
    let b = scene
        .trace_caustics_photon(center_ray, Some(650.0), IorMode::Dispersion)
        .unwrap();
    assert!((a.position - b.position).norm() > 0.09);
    assert_eq!(
        scene.trace_caustics_photon(center_ray, Some(450.0), IorMode::Constant),
        scene.trace_caustics_photon(center_ray, Some(650.0), IorMode::Constant)
    );
    // 固定方向只用于隔离光源位置大小的光学投影；实际发射始终使用完整余弦半球。
    let mut widths = Vec::new();
    for radius in [0.01, 0.06] {
        let light = DiskAreaLight { radius, ..base };
        let mut points = Vec::new();
        for i in 0..16 {
            for j in 0..32 {
                let p = light
                    .sample_position([(f64::from(i) + 0.5) / 16.0, (f64::from(j) + 0.5) / 32.0])
                    .unwrap();
                let landing = scene
                    .trace_caustics_photon(
                        Ray::new(p.position, light.normal).unwrap(),
                        Some(550.0),
                        IorMode::Dispersion,
                    )
                    .unwrap();
                points.push(landing.position);
            }
        }
        let mean = points.iter().copied().sum::<Vector3<f64>>() / points.len() as f64;
        widths.push(
            (points
                .iter()
                .map(|p| (p - mean).norm_squared())
                .sum::<f64>()
                / points.len() as f64)
                .sqrt(),
        );
    }
    assert!((widths[1] / widths[0] - 6.0).abs() < 1e-8);
}

#[test]
fn disk_scalar_nodes_reintegrate_without_extra_fresnel_d65_or_pdf_factors() {
    let mut c = config();
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        let scene = Scene::from_config(&c.scene).unwrap();
        let map = render_wavelength_caustics(&scene, &c.render)
            .unwrap()
            .unwrap();
        let xyz = Render::new(c.render.clone())
            .unwrap()
            .render_lighting(&scene)
            .unwrap();
        let mut integrated = vec![Vector3::zeros(); c.render.width * c.render.height];
        for (j, &(nm, delta)) in map.wavelength_samples().iter().enumerate() {
            for (p, e) in integrated.iter_mut().zip(map.layer_irradiance(j).unwrap()) {
                *p += cie_xyz(nm).unwrap() * (*e * delta);
            }
        }
        for (a, b) in integrated.iter().zip(&xyz.caustics.pixels) {
            assert!((a - b).norm() < 2e-12 * (1.0 + b.norm()));
        }
        let flux = integrated.iter().map(|p| p.y).sum::<f64>() * scene.receiver.area()
            / integrated.len() as f64;
        assert!(flux > 0.0 && flux < scene.light.total_flux());
        let white: Vector3<f64> = c
            .render
            .wavelength_samples()
            .unwrap()
            .iter()
            .map(|&(nm, width)| cie_xyz(nm).unwrap() * (d65_spectrum(nm).unwrap() * width))
            .sum();
        let chroma = integrated
            .iter()
            .filter(|p| p.y > 1e-9)
            .map(|p| (p / p.y - white / white.y).norm())
            .fold(0.0_f64, f64::max);
        if mode == IorMode::Constant {
            assert!(chroma < 1e-12);
        } else {
            assert!(chroma > 0.05);
        }
        println!(
            "{mode:?}: captured caustics Y flux={flux:.8}, emitted={}",
            scene.light.total_flux()
        );
    }
    // 固定 λ 的发射密度不能随图集步长或发射波段改变；与 Camera 的 D65 定义一致。
    c.render.mode = IorMode::Constant;
    let scene = Scene::from_config(&c.scene).unwrap();
    let mut reference: Option<f64> = None;
    for (min, max, step) in [
        (380.0, 780.0, 5.0),
        (380.0, 780.0, 10.0),
        (380.0, 780.0, 17.0),
        (500.0, 600.0, 10.0),
    ] {
        c.render.wavelength_min_nm = min;
        c.render.wavelength_max_nm = max;
        c.render.wavelength_step_nm = step;
        let map = render_wavelength_caustics(&scene, &c.render)
            .unwrap()
            .unwrap();
        let j = map
            .wavelength_samples()
            .iter()
            .position(|s| s.0 == 550.0)
            .unwrap();
        let flux = map.layer_irradiance(j).unwrap().iter().sum::<f64>() * scene.receiver.area()
            / (c.render.width * c.render.height) as f64
            / d65_spectrum(550.0).unwrap();
        if let Some(old) = reference {
            assert!((flux / old - 1.0).abs() < 2e-13);
        }
        reference = Some(flux);
    }
}

#[test]
fn disk_packet_components_and_rgb_repeat_across_threads_with_crn_preserved() {
    let mut c = config();
    c.render.width = 8;
    c.render.height = 8;
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        let scene = Scene::from_config(&c.scene).unwrap();
        for packet in [1, 4] {
            c.render.spectral_packet_size = packet;
            let mut reference: Option<spectral_caustics::render::CameraLightingImages> = None;
            for threads in [1, 2, 4, 1] {
                c.render.thread_count = Some(threads);
                let image = Render::new(c.render.clone())
                    .unwrap()
                    .render_spectral_camera_components(&scene)
                    .unwrap();
                assert!(image.direct.pixels.iter().any(|p| p.y > 0.0));
                assert!(image.caustics.pixels.iter().any(|p| p.y > 0.0));
                for ((d, k), sum) in image
                    .direct
                    .pixels
                    .iter()
                    .zip(&image.caustics.pixels)
                    .zip(&image.combined.pixels)
                {
                    assert_eq!(*d + *k, *sum);
                }
                if let Some(old) = &reference {
                    same(&old.direct, &image.direct);
                    same(&old.caustics, &image.caustics);
                    same(&old.combined, &image.combined);
                }
                reference = Some(image);
            }
            c.render.spectral_debug_disable_ground_sun = true;
            let debug = Render::new(c.render.clone())
                .unwrap()
                .render_camera(&scene)
                .unwrap();
            same(&debug, &reference.unwrap().combined);
            c.render.spectral_debug_disable_ground_sun = false;
        }
        c.render.camera_mode = CameraMode::Rgb;
        let mut reference: Option<spectral_caustics::render::CameraLightingImages> = None;
        for threads in [1, 2, 4] {
            c.render.thread_count = Some(threads);
            let r = Render::new(c.render.clone()).unwrap();
            let light = r.render_lighting(&scene).unwrap();
            let image = r
                .render_camera_glass_components(&scene, &light, c.render.camera_spp, c.render.seed)
                .unwrap();
            assert!(image.direct.pixels.iter().any(|p| p.y > 0.0));
            assert!(image.caustics.pixels.iter().any(|p| p.y > 0.0));
            for ((d, k), sum) in image
                .direct
                .pixels
                .iter()
                .zip(&image.caustics.pixels)
                .zip(&image.combined.pixels)
            {
                assert!((*d + *k - *sum).norm() < 1e-12);
            }
            if let Some(old) = &reference {
                same(&old.direct, &image.direct);
                same(&old.caustics, &image.caustics);
                same(&old.combined, &image.combined);
            }
            reference = Some(image);
        }
        c.render.camera_mode = CameraMode::Spectral;
    }
}

#[test]
fn disk_nee_quota_and_wavelength_crn_do_not_consume_environment_or_continuation_streams() {
    let mut c = config();
    c.scene.max_diffuse_bounces = 0;
    let env = spectral_caustics::config::sceneconfig::EnvironmentConfig {
        horizon_linear_rgb: [0.1; 3],
        zenith_linear_rgb: [0.1; 3],
        ..Default::default()
    };
    c.scene.environment = Some(env);
    let mut scene = Scene::from_config(&c.scene).unwrap();
    let d = scene.light.as_disk().unwrap();
    let ray = Ray::new(d.center, d.normal).unwrap();
    let mut streams = None;
    for samples in [1, 8] {
        scene.light_samples = samples;
        let mut normalized: Option<f64> = None;
        for nm in [450.0, 550.0, 650.0] {
            let mut state = SpectralPathState::new(ray, nm, 0).unwrap();
            let mut prefix = Transmit { draws: 0 };
            let mut environment = StdRng::seed_from_u64(43);
            let mut continuation = StdRng::seed_from_u64(44);
            trace_spectral_camera_path(
                &scene,
                &mut state,
                IorMode::Constant,
                &mut prefix,
                &mut environment,
                &mut continuation,
            )
            .unwrap();
            assert_eq!(state.delta_events(), 2);
            let value = (
                prefix.draws,
                environment.next_u64(),
                continuation.next_u64(),
            );
            if let Some(old) = streams {
                assert_eq!(value, old);
            }
            streams = Some(value);
            let l = state.radiance
                / (rgb_reflectance(Vector3::repeat(0.18), nm).unwrap() * d65_spectrum(nm).unwrap());
            if let Some(old) = normalized {
                assert!((l - old).abs() < 1e-12);
            }
            normalized = Some(l);
        }
    }
    // 光源是显式 NEE 发射器，不在可命中的几何列表中，不会与 BSDF/环境 NEE 双计。
    let mut emitter_only = Scene::new();
    emitter_only.light = disk(0.01).into();
    assert!(
        emitter_only
            .intersect(&Ray::new(Vector3::zeros(), Vector3::z()).unwrap())
            .is_none()
    );
}

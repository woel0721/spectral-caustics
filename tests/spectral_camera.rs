use nalgebra::Vector3;
use rand::{RngCore, SeedableRng, rngs::StdRng};
use spectral_caustics::{
    config::{
        Config,
        materialconfig::MaterialConfig,
        rendersetting::{CameraMode, RenderSetting},
        sceneconfig::{EnvironmentConfig, EnvironmentMisHeuristic, ObjectConfig},
    },
    geometry::{ground::Ground, hit::Hit, object::Hittable, ray::Ray, sphere::Sphere},
    material::{
        BsdfValue, EvaluateMode, Material, ScatterType,
        dielectric::{Dielectric, fresnel_dielectric},
        lambertian::Lambertian,
        mirror::Mirror,
    },
    optics::{
        dispersion::Cauchy,
        emission::{d65_spectrum, rgb_emission},
        iormode::IorMode,
        spectrum::{cie_xyz, rgb_reflectance},
    },
    render::{
        Render, SpectralPathState, framebuffer::Framebuffer, render_camera_glass,
        render_spectral_camera, trace_spectral_camera_path,
    },
    scene::{Environment, Scene},
};
use std::{
    f64::consts::PI,
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct FixedRng {
    value: u64,
    draws: usize,
}
impl FixedRng {
    fn new(value: u64) -> Self {
        Self { value, draws: 0 }
    }
}
impl RngCore for FixedRng {
    fn next_u64(&mut self) -> u64 {
        self.draws += 1;
        self.value
    }
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for b in dest {
            *b = self.next_u64() as u8;
        }
    }
}

fn settings(spp: usize) -> RenderSetting {
    RenderSetting {
        camera_mode: CameraMode::Spectral,
        width: 8,
        height: 6,
        camera_spp: spp,
        thread_count: Some(1),
        ..RenderSetting::default()
    }
}
fn mean(image: &Framebuffer) -> Vector3<f64> {
    image.pixels.iter().copied().sum::<Vector3<f64>>() / image.pixels.len() as f64
}
fn same(a: &Framebuffer, b: &Framebuffer) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    for (a, b) in a.pixels.iter().zip(&b.pixels) {
        for (a, b) in a.iter().zip(b.iter()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }
}
fn integrate(f: impl Fn(f64) -> f64) -> Vector3<f64> {
    (0..4000)
        .map(|i| {
            let nm = 380.0 + (f64::from(i) + 0.5) * 0.1;
            cie_xyz(nm).unwrap() * f(nm) * 0.1
        })
        .sum()
}
fn empty() -> Scene {
    let mut scene = Scene::new();
    scene.environment = None;
    scene.background_color = Vector3::repeat(1.0);
    scene.light.total_flux = 0.0;
    scene.ground_checkerboard = None;
    scene
}
fn ground_scene() -> Scene {
    let mut scene = empty();
    let id = scene.add_with_material(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    scene.ground_id = Some(id);
    scene.light.direction = -Vector3::z();
    scene
}
fn probe(scene: &Scene, nm: f64, depth: usize) -> SpectralPathState {
    let ray = Ray::new(Vector3::new(0.2, 0.1, 3.0), -Vector3::z()).unwrap();
    let mut state = SpectralPathState::new(ray, nm, depth).unwrap();
    let mut scattering = StdRng::seed_from_u64(42);
    let mut environment = StdRng::seed_from_u64(43);
    trace_spectral_camera_path(
        scene,
        &mut state,
        IorMode::Dispersion,
        &mut scattering,
        &mut environment,
    )
    .unwrap();
    state
}

#[test]
fn hero_wavelength_integrates_d65_white_and_converges_without_a_second_delta_lambda() {
    let scene = empty();
    let expected = integrate(|nm| d65_spectrum(nm).unwrap());
    let low = Render::new(settings(16))
        .unwrap()
        .render_camera(&scene)
        .unwrap();
    let high = Render::new(settings(16384))
        .unwrap()
        .render_camera(&scene)
        .unwrap();
    let error = (mean(&high) - expected).norm();
    assert!(error < 0.012, "MC white {error}: {:?}", mean(&high));
    assert!(error < (mean(&low) - expected).norm() * 0.4);
    assert!((mean(&high).y - 1.0).abs() < 0.01);
}

#[test]
fn dielectric_spectral_sample_uses_current_wavelength_and_preserves_constant_ior_rgb_behavior() {
    let glass = Dielectric::with_dispersion(1.52, Some(Cauchy::default())).unwrap();
    let wo = Vector3::new(0.6, 0.0, 0.8);
    let mut fixed = FixedRng::new(u64::MAX);
    let mut directions = Vec::new();
    for nm in [450.0, 550.0, 650.0] {
        let n = Cauchy::default().ior(nm).unwrap();
        let s = glass
            .sample_spectral(wo, Vector3::z(), nm, IorMode::Dispersion, &mut fixed)
            .unwrap();
        assert_eq!(s.event, ScatterType::SpecularTransmission);
        assert_eq!(s.pdf, None);
        assert!((s.wi.x + 0.6 / n).abs() < 1e-12);
        assert!((s.weight - 1.0 / (n * n)).abs() < 1e-12);
        directions.push(s.wi);
        let constant = glass
            .sample_spectral(wo, Vector3::z(), nm, IorMode::Constant, &mut fixed)
            .unwrap();
        let rgb = glass.sample(wo, Vector3::z(), &mut fixed).unwrap();
        assert_eq!(constant.wi, rgb.wi);
        assert_eq!(Vector3::repeat(constant.weight), rgb.weight);
    }
    assert!((directions[0] - directions[2]).norm() > 1e-3);
    for nm in [f64::NAN, 0.0, -1.0] {
        assert!(
            glass
                .sample_spectral(wo, Vector3::z(), nm, IorMode::Constant, &mut fixed)
                .is_none()
        );
    }
}

#[test]
fn spectral_glass_fresnel_probabilities_and_radiance_eta_weights_are_compensated() {
    let glass = Dielectric::with_dispersion(1.52, Some(Cauchy::default())).unwrap();
    let wo = Vector3::new(0.6, 0.0, 0.8);
    let n = Cauchy::default().ior(450.0).unwrap();
    let f = fresnel_dielectric(0.8, 1.0, n).unwrap();
    let mut rng = StdRng::seed_from_u64(42);
    let mut reflections = 0;
    let mut weight = 0.0;
    let count = 65536;
    for _ in 0..count {
        let s = glass
            .sample_spectral(wo, Vector3::z(), 450.0, IorMode::Dispersion, &mut rng)
            .unwrap();
        reflections += usize::from(s.event == ScatterType::SpecularReflection);
        weight += s.weight / count as f64;
    }
    assert!((reflections as f64 / count as f64 - f).abs() < 0.003);
    assert!((weight - (f + (1.0 - f) / (n * n))).abs() < 0.003);
    let angle = 60.0_f64.to_radians();
    let inside = Vector3::new(angle.sin(), 0.0, -angle.cos());
    let mut fixed = FixedRng::new(u64::MAX);
    let tir = glass
        .sample_spectral(inside, Vector3::z(), 450.0, IorMode::Dispersion, &mut fixed)
        .unwrap();
    assert_eq!(tir.event, ScatterType::SpecularReflection);
    assert_eq!(tir.weight, 1.0);
    assert_eq!(fixed.draws, 0);
}

#[test]
fn sphere_two_spectral_interfaces_cancel_eta_weights_and_keep_one_wavelength() {
    let mut scene = empty();
    scene.add_with_material(
        Box::new(Sphere::new(Vector3::new(0.0, 0.0, 1.5), 0.8).unwrap()),
        Box::new(Dielectric::with_dispersion(1.52, Some(Cauchy::default())).unwrap()),
    );
    let mut directions = Vec::new();
    for nm in [450.0, 650.0] {
        let mut state = SpectralPathState::new(
            Ray::new(Vector3::new(0.4, 0.0, 3.0), -Vector3::z()).unwrap(),
            nm,
            0,
        )
        .unwrap();
        trace_spectral_camera_path(
            &scene,
            &mut state,
            IorMode::Dispersion,
            &mut FixedRng::new(u64::MAX),
            &mut FixedRng::new(0),
        )
        .unwrap();
        assert_eq!(state.wavelength_nm, nm);
        assert_eq!(state.path_depth, 2);
        assert_eq!(state.non_delta_bounces, 0);
        assert!((state.throughput - 1.0).abs() < 1e-12);
        assert!((state.radiance - d65_spectrum(nm).unwrap()).abs() < 1e-12);
        directions.push(state.ray.direction);
    }
    assert!((directions[0] - directions[1]).norm() > 1e-3);
}

#[test]
fn lambertian_and_mirror_spectral_weights_come_from_reconstruction_and_match_their_pdf() {
    let color = Vector3::new(0.1, 0.5, 0.9);
    for material in [
        Box::new(Lambertian::new(color).unwrap()) as Box<dyn Material>,
        Box::new(Mirror::new(color).unwrap()),
    ] {
        for nm in [450.0, 550.0, 650.0] {
            let s = material
                .sample_spectral(
                    Vector3::z(),
                    Vector3::z(),
                    nm,
                    IorMode::Dispersion,
                    &mut StdRng::seed_from_u64(42),
                )
                .unwrap();
            assert_eq!(s.weight, rgb_reflectance(color, nm).unwrap());
            assert!(s.weight >= 0.0 && s.weight <= 1.0);
            if let Some(pdf) = s.pdf {
                let BsdfValue::Spectral(f) = material.evaluate(
                    s.wi,
                    Vector3::z(),
                    Vector3::z(),
                    EvaluateMode::Spectral { wavelength_nm: nm },
                ) else {
                    panic!()
                };
                assert_eq!(
                    pdf,
                    material.pdf_spectral(s.wi, Vector3::z(), Vector3::z(), nm)
                );
                assert!((f * s.wi.z / pdf - s.weight).abs() < 1e-12);
            } else {
                assert_eq!(s.event, ScatterType::SpecularReflection);
            }
        }
    }
}

#[test]
fn scalar_direct_sun_matches_spectral_lambertian_and_spatial_flux_normalization() {
    let mut scene = ground_scene();
    scene.light.total_flux = 8.0;
    let strength = scene.light.sunlight().unwrap().1;
    for nm in [450.0, 550.0, 650.0] {
        let state = probe(&scene, nm, 0);
        let expected = rgb_reflectance(Vector3::repeat(0.8), nm).unwrap() / PI
            * strength
            * d65_spectrum(nm).unwrap();
        assert!((state.radiance - expected).abs() < 1e-13);
        assert_eq!(state.path_depth, 0);
        assert_eq!(state.throughput, 1.0);
    }
    let expected = integrate(|nm| {
        rgb_reflectance(Vector3::repeat(0.8), nm).unwrap() / PI
            * strength
            * d65_spectrum(nm).unwrap()
    });
    let high = Render::new(settings(4096))
        .unwrap()
        .render_camera(&scene)
        .unwrap();
    assert!((mean(&high) - expected).norm() < 0.002);
}

#[test]
fn environment_nee_and_diffuse_escape_are_not_double_counted_and_hdr_gain_is_preserved() {
    let mut scene = ground_scene();
    let environment = EnvironmentConfig {
        horizon_linear_rgb: [7.0; 3],
        zenith_linear_rgb: [7.0; 3],
        ..EnvironmentConfig::default()
    };
    scene.environment = Some(Environment::try_from(&environment).unwrap());
    for depth in [0, 1, 4] {
        let state = probe(&scene, 550.0, depth);
        let expected = rgb_reflectance(Vector3::repeat(0.8), 550.0).unwrap()
            * 7.0
            * d65_spectrum(550.0).unwrap();
        assert!((state.radiance - expected).abs() < 1e-12);
        assert_eq!(state.non_delta_bounces, usize::from(depth > 0));
    }
    let before = probe(&scene, 450.0, 0).radiance;
    scene.environment.as_mut().unwrap().intensity = 100.0;
    assert!((probe(&scene, 450.0, 0).radiance - before * 100.0).abs() < 1e-12);
}

#[test]
fn environment_escape_uses_full_scalar_throughput_only_after_delta() {
    let mut scene = empty();
    scene.environment = Some(
        Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: [3.0; 3],
            zenith_linear_rgb: [3.0; 3],
            ..EnvironmentConfig::default()
        })
        .unwrap(),
    );
    for (event, include) in [
        (ScatterType::DiffuseReflection, false),
        (ScatterType::SpecularTransmission, true),
        (ScatterType::SpecularReflection, true),
    ] {
        let mut state =
            SpectralPathState::new(Ray::new(Vector3::zeros(), Vector3::z()).unwrap(), 550.0, 4)
                .unwrap();
        state.has_non_delta_vertex = true;
        state.last_scatter = Some(event);
        state.throughput = 0.2;
        state.radiance = 0.123;
        trace_spectral_camera_path(
            &scene,
            &mut state,
            IorMode::Dispersion,
            &mut FixedRng::new(0),
            &mut FixedRng::new(0),
        )
        .unwrap();
        let extra = if include {
            0.2 * 3.0 * d65_spectrum(550.0).unwrap()
        } else {
            0.0
        };
        assert!((state.radiance - 0.123 - extra).abs() < 1e-12);
    }
}

struct Plane {
    z: f64,
    normal: Vector3<f64>,
}
impl Hittable for Plane {
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        let t = (self.z - ray.origin.z) / ray.direction.z;
        (t.is_finite() && t > 1e-6).then(|| Hit {
            coords: ray.at(t),
            normal: self.normal,
            distance: t,
            ..Hit::default()
        })
    }
}
fn chamber(diffuse: bool) -> Scene {
    let mut scene = empty();
    for (z, n) in [(-1.0, Vector3::z()), (1.0, -Vector3::z())] {
        let m: Box<dyn Material> = if diffuse {
            Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap())
        } else {
            Box::new(Mirror::new(Vector3::repeat(1.0)).unwrap())
        };
        scene.add_with_material(Box::new(Plane { z, normal: n }), m);
    }
    scene
}

#[test]
fn spectral_paths_keep_the_32_delta_budget_and_roulette_after_three_continuous_connections() {
    let ray = Ray::new(Vector3::zeros(), Vector3::z()).unwrap();
    let mut state = SpectralPathState::new(ray, 550.0, 0).unwrap();
    trace_spectral_camera_path(
        &chamber(false),
        &mut state,
        IorMode::Dispersion,
        &mut FixedRng::new(0),
        &mut FixedRng::new(0),
    )
    .unwrap();
    assert_eq!(state.delta_events(), 32);
    assert_eq!(state.radiance, 0.0);
    for depth in [0, 1, 2, 3, 4] {
        let mut state = SpectralPathState::new(ray, 550.0, depth).unwrap();
        let mut rng = FixedRng::new(u64::MAX);
        trace_spectral_camera_path(
            &chamber(true),
            &mut state,
            IorMode::Dispersion,
            &mut rng,
            &mut FixedRng::new(0),
        )
        .unwrap();
        assert_eq!(state.non_delta_bounces, depth.min(3));
        assert_eq!(rng.draws, depth.min(3) * 2 + usize::from(depth > 3));
    }
}

#[test]
fn spectral_modes_repeat_bitwise_across_threads_and_rgb_entry_stays_compatible() {
    let mut c = Config::from_json(include_str!("../configs/sphere_spectral.json")).unwrap();
    c.render.width = 12;
    c.render.height = 10;
    c.render.camera_spp = 12;
    c.scene.environment.as_mut().unwrap().map_path = None;
    let mut modes = Vec::new();
    for ior_mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = ior_mode;
        for heuristic in [
            EnvironmentMisHeuristic::Balance,
            EnvironmentMisHeuristic::Power,
        ] {
            c.scene.environment_mis_heuristic = heuristic;
            for depth in [0, 4] {
                c.scene.max_diffuse_bounces = depth;
                let scene = Scene::from_config(&c.scene).unwrap();
                let mut reference = None;
                for threads in [1, 2, 4, 1] {
                    c.render.thread_count = Some(threads);
                    let image = Render::new(c.render.clone())
                        .unwrap()
                        .render_camera(&scene)
                        .unwrap();
                    assert!(image.pixels.iter().any(|p| p.y > 0.0));
                    if let Some(previous) = &reference {
                        same(&image, previous);
                    }
                    reference = Some(image);
                }
                modes.push(reference.unwrap());
            }
        }
    }
    assert_ne!(modes[0].pixels, modes[4].pixels);
    c.render.camera_mode = CameraMode::Rgb;
    let scene = Scene::from_config(&c.scene).unwrap();
    let image = Render::new(c.render.clone())
        .unwrap()
        .render_camera(&scene)
        .unwrap();
    same(
        &image,
        &render_camera_glass(&scene, 12, 10, 12, c.render.seed).unwrap(),
    );
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("spectral-hero-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn spectral_hdri_balance_power_and_odd_quotas_converge_to_a_uniform_emitter() {
    let dir = Temp::new();
    let path = dir.0.join("white.hdr");
    image::codecs::hdr::HdrEncoder::new(fs::File::create(&path).unwrap())
        .encode(&[image::Rgb([4.0_f32; 3]); 8], 4, 2)
        .unwrap();
    let mut scene = ground_scene();
    scene.environment = Some(
        Environment::try_from(&EnvironmentConfig {
            map_path: Some(path),
            ..EnvironmentConfig::default()
        })
        .unwrap(),
    );
    let expected =
        rgb_reflectance(Vector3::repeat(0.8), 550.0).unwrap() * 4.0 * d65_spectrum(550.0).unwrap();
    for heuristic in [
        EnvironmentMisHeuristic::Balance,
        EnvironmentMisHeuristic::Power,
    ] {
        scene.environment_mis_heuristic = heuristic;
        for count in [1, 2, 3, 8] {
            scene.environment_samples = count;
            let mut environment = StdRng::seed_from_u64(42);
            let mut scattering = StdRng::seed_from_u64(43);
            let mut mean = 0.0;
            for _ in 0..8192 {
                let mut state = SpectralPathState::new(
                    Ray::new(Vector3::new(0.0, 0.0, 2.0), -Vector3::z()).unwrap(),
                    550.0,
                    0,
                )
                .unwrap();
                trace_spectral_camera_path(
                    &scene,
                    &mut state,
                    IorMode::Dispersion,
                    &mut scattering,
                    &mut environment,
                )
                .unwrap();
                mean += state.radiance / 8192.0;
            }
            assert!(
                (mean / expected - 1.0).abs() < 0.025,
                "{heuristic:?} N={count}: {mean} vs {expected}"
            );
        }
    }
}

#[test]
fn spectral_config_defaults_roundtrips_rejects_ggx_and_cli_renders_xyz_output() {
    assert_eq!(Config::default().render.camera_mode, CameraMode::Rgb);
    let mut c = Config::from_json(include_str!("../configs/sphere_spectral.json")).unwrap();
    assert_eq!(Config::from_json(&c.to_json().unwrap()).unwrap(), c);
    c.render.width = 16;
    c.render.height = 12;
    c.render.camera_spp = 16;
    c.scene.environment.as_mut().unwrap().map_path = None;
    let dir = Temp::new();
    c.render.output_path = dir.0.join("camera.png");
    c.save(dir.0.join("scene.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_spectral-caustics"))
        .arg(dir.0.join("scene.json"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Hero wavelength"));
    let image = image::open(&c.render.output_path).unwrap().into_rgb8();
    assert_eq!(image.dimensions(), (16, 12));
    assert!(image.pixels().any(|p| p.0.iter().any(|v| *v > 0)));
    assert!(!dir.0.join("camera_caustics.png").exists());
    if let ObjectConfig::Sphere { material, .. } = &mut c.scene.objects[0] {
        *material = MaterialConfig::GgxMetal {
            eta: [0.2; 3],
            k: [3.0; 3],
            roughness: 0.4,
        };
    }
    // 关闭光源焦散目标，确保错误确实来自光谱相机材质能力检查。
    if let ObjectConfig::Sphere { caustics, .. } = &mut c.scene.objects[0] {
        *caustics = false;
    }
    assert!(c.validate().unwrap_err().contains("GGX"));
    c.render.camera_mode = CameraMode::Rgb;
    assert!(c.validate().is_ok());
    let scene = Scene::from_config(&c.scene).unwrap();
    assert!(
        render_spectral_camera(&scene, 8, 6, 1, 42, IorMode::Dispersion)
            .err()
            .unwrap()
            .to_string()
            .contains("GGX")
    );
}

#[test]
fn spectral_invalid_workloads_and_path_states_fail_explicitly() {
    let scene = empty();
    assert!(render_spectral_camera(&scene, 8, 6, 0, 42, IorMode::Dispersion).is_err());
    assert!(render_spectral_camera(&scene, 0, 6, 1, 42, IorMode::Dispersion).is_err());
    let ray = Ray::new(Vector3::zeros(), Vector3::z()).unwrap();
    for nm in [379.0, 781.0, f64::NAN] {
        assert!(SpectralPathState::new(ray, nm, 0).is_err());
    }
    assert!(SpectralPathState::new(ray, 550.0, 1025).is_err());
    let mut bad_ray = ray;
    bad_ray.direction = Vector3::repeat(f64::NAN);
    assert!(SpectralPathState::new(bad_ray, 550.0, 0).is_err());
    let mut state = SpectralPathState::new(ray, 550.0, 0).unwrap();
    state.throughput = f64::NAN;
    assert!(
        trace_spectral_camera_path(
            &scene,
            &mut state,
            IorMode::Dispersion,
            &mut FixedRng::new(0),
            &mut FixedRng::new(0)
        )
        .is_err()
    );
    assert!(rgb_emission(Vector3::new(-1.0, 0.0, 0.0), 550.0).is_none());
}

#[test]
fn colored_hdr_odd_mis_with_glass_occlusion_matches_independent_hemisphere_quadrature() {
    let dir = Temp::new();
    let path = dir.0.join("colored.hdr");
    let pixels: Vec<_> = (0..32)
        .map(|i| {
            image::Rgb(match i % 4 {
                0 => [0.1, 0.2, 3.0],
                1 => [8.0, 1.0, 0.1],
                2 => [0.1, 2.0, 0.1],
                _ => [0.05; 3],
            })
        })
        .collect();
    image::codecs::hdr::HdrEncoder::new(fs::File::create(&path).unwrap())
        .encode(&pixels, 8, 4)
        .unwrap();
    let mut scene = ground_scene();
    scene.environment = Some(
        Environment::try_from(&EnvironmentConfig {
            map_path: Some(path),
            rotation_degrees: 35.0,
            intensity: 2.0,
            ..EnvironmentConfig::default()
        })
        .unwrap(),
    );
    scene.environment_samples = 3;
    // 摄像机位于遮挡球下方；球只遮挡地面的环境照明，局部 NEE 对玻璃仍采用二值可见性。
    scene.add_with_material(
        Box::new(Sphere::new(Vector3::z(), 0.4).unwrap()),
        Box::new(Dielectric::new(1.52).unwrap()),
    );
    let ray = Ray::new(Vector3::z() * 0.1, -Vector3::z()).unwrap();
    for nm in [450.0, 550.0, 650.0] {
        let rho = rgb_reflectance(Vector3::repeat(0.8), nm).unwrap();
        let mut reference = 0.0;
        for j in 0..128 {
            let cos = (f64::from(j) + 0.5) / 128.0;
            // 半径 0.4、中心距离 1 的球从原点遮挡一个解析圆锥。
            if cos > (1.0_f64 - 0.4_f64.powi(2)).sqrt() {
                continue;
            }
            for i in 0..256 {
                let phi = 2.0 * PI * (f64::from(i) + 0.5) / 256.0;
                let sin = (1.0 - cos * cos).sqrt();
                let wi = Vector3::new(sin * phi.cos(), sin * phi.sin(), cos);
                let li = rgb_emission(
                    scene.environment.as_ref().unwrap().radiance(wi).unwrap(),
                    nm,
                )
                .unwrap();
                reference += rho / PI * li * cos * (2.0 * PI / (128.0 * 256.0));
            }
        }
        for heuristic in [
            EnvironmentMisHeuristic::Balance,
            EnvironmentMisHeuristic::Power,
        ] {
            scene.environment_mis_heuristic = heuristic;
            let mut environment = StdRng::seed_from_u64(42);
            let mut scattering = StdRng::seed_from_u64(43);
            let mut estimate = 0.0;
            for _ in 0..32768 {
                let mut state = SpectralPathState::new(ray, nm, 0).unwrap();
                trace_spectral_camera_path(
                    &scene,
                    &mut state,
                    IorMode::Dispersion,
                    &mut scattering,
                    &mut environment,
                )
                .unwrap();
                estimate += state.radiance / 32768.0;
            }
            assert!(
                (estimate / reference - 1.0).abs() < 0.035,
                "{nm} nm {heuristic:?}: {estimate} vs {reference}"
            );
        }
    }
}

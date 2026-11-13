use nalgebra::Vector3;
use rand::{SeedableRng, rngs::StdRng};
use spectral_caustics::{
    config::{
        Config, RenderJob,
        render::CameraMode,
        scene::{EnvironmentConfig, EnvironmentMisHeuristic},
    },
    environment::Environment,
    geometry::{ground::Ground, ray::Ray, sphere::Sphere},
    integrator::camera::{
        path::trace_camera_path,
        transport::{CameraTransport, Caustics},
        types::{CameraPathRngs, CameraPathState},
    },
    light::{DiskAreaLight, Light},
    material::lambertian::Lambertian,
    medium::{HomogeneousMedium, HomogeneousMediumConfig},
    optics::dispersion::IorMode,
    renderer::Renderer,
};

fn slab(a: f64, s: f64, g: f64) -> HomogeneousMedium {
    HomogeneousMedium::try_from(&HomogeneousMediumConfig {
        min_m: [-100.0, -100.0, 1.0],
        max_m: [100.0, 100.0, 2.0],
        sigma_a: a,
        sigma_s: s,
        g,
    })
    .unwrap()
}
fn empty() -> RenderJob {
    let mut j = RenderJob::new();
    j.scene.environment = None;
    j.scene.background_color = Vector3::zeros();
    j.ground_checkerboard = None;
    j
}
fn ground(j: &mut RenderJob) {
    j.photon.ground_id = Some(j.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.5)).unwrap()),
    ));
}
fn ray(z: f64) -> Ray {
    Ray::new(Vector3::z() * z, -Vector3::z()).unwrap()
}
fn trace<T: CameraTransport>(
    j: &RenderJob,
    state: &mut CameraPathState<T>,
    sun: (Vector3<f64>, f64),
    rng: &mut StdRng,
) {
    trace_camera_path(
        j,
        state,
        &mut CameraPathRngs {
            camera: &mut StdRng::seed_from_u64(1),
            environment: &mut StdRng::seed_from_u64(2),
            diffuse: &mut StdRng::seed_from_u64(3),
            medium: rng,
        },
        sun.0,
        sun.1,
        Caustics::None,
        IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();
}
fn rgb(j: &RenderJob, r: Ray, sun: (Vector3<f64>, f64), seed: u64) -> Vector3<f64> {
    let mut state = CameraPathState::new(r, 0);
    trace(j, &mut state, sun, &mut StdRng::seed_from_u64(seed));
    state.combined_radiance()
}

#[test]
fn pure_absorption_attenuates_camera_and_surface_sun_once_in_rgb_and_spectral() {
    let mut j = empty();
    j.scene.background_color = Vector3::new(0.4, 0.6, 0.8);
    let sun = (Vector3::z(), 1.0);
    let original = rgb(&j, ray(3.0), sun, 1);
    j.scene.medium = Some(slab(0.4, 0.0, 0.7));
    assert!((rgb(&j, ray(3.0), sun, 1) - original * (-0.4_f64).exp()).norm() < 1e-14);
    let mut spectral = CameraPathState::spectral(ray(3.0), 550.0, 0).unwrap();
    j.scene.medium = None;
    trace(&j, &mut spectral, sun, &mut StdRng::seed_from_u64(1));
    let reference = spectral.combined_radiance();
    j.scene.medium = Some(slab(0.4, 0.0, 0.0));
    let mut spectral = CameraPathState::spectral(ray(3.0), 550.0, 0).unwrap();
    trace(&j, &mut spectral, sun, &mut StdRng::seed_from_u64(1));
    assert!((spectral.combined_radiance() / reference - (-0.4_f64).exp()).abs() < 1e-14);
    ground(&mut j);
    j.scene.medium = None;
    let surface = rgb(&j, ray(3.0), sun, 1);
    j.scene.medium = Some(slab(0.4, 0.0, 0.0));
    assert!((rgb(&j, ray(3.0), sun, 1) - surface * (-0.8_f64).exp()).norm() < 1e-14);
    // A surface before the box must not receive camera-segment attenuation.
    assert!((rgb(&j, ray(0.5), sun, 1) - surface * (-0.4_f64).exp()).norm() < 1e-14);
}

#[test]
fn volume_sun_matches_the_independent_single_scattering_slab_integral() {
    let mut j = empty();
    for g in [0.0, 0.7] {
        j.scene.medium = Some(slab(0.2, 0.5, g));
        for sign in [-1.0, 1.0] {
            let sun = (Vector3::z() * sign, 1.0);
            let phase = j.scene.medium.unwrap().phase().eval(Vector3::z(), sun.0);
            let expected = if sign > 0.0 {
                phase * 0.5 / 1.4 * (1.0 - (-1.4_f64).exp())
            } else {
                phase * 0.5 * (-0.7_f64).exp()
            };
            let mut rng = StdRng::seed_from_u64(120);
            let mut mean = 0.0;
            for _ in 0..40_000 {
                let mut state = CameraPathState::new(ray(3.0), 0);
                trace(&j, &mut state, sun, &mut rng);
                mean += state.radiance.direct.x / 40_000.0;
                assert_eq!(state.path_depth, 0); // Volume has its own budget.
                assert_eq!(state.radiance.caustics, Vector3::zeros());
            }
            assert!(
                (mean / expected - 1.0).abs() < 0.025,
                "g={g}, sign={sign}, {mean}/{expected}"
            );
            // Scalar transport uses exactly the same sampled geometry/weights.
            let mut rgb = CameraPathState::new(ray(3.0), 0);
            let mut spectral = CameraPathState::spectral(ray(3.0), 550.0, 0).unwrap();
            trace(&j, &mut rgb, sun, &mut StdRng::seed_from_u64(4));
            trace(&j, &mut spectral, sun, &mut StdRng::seed_from_u64(4));
            let d65 = spectral_caustics::spectrum::emission::d65_spectrum(550.0).unwrap();
            assert!((spectral.radiance.direct - rgb.radiance.direct.x * d65).abs() < 1e-14);
        }
    }
}

#[test]
fn phase_continuation_reaches_surface_without_a_second_scatter_or_caustic_lookup() {
    let mut j = empty();
    ground(&mut j);
    j.scene.medium = Some(slab(0.1, 0.8, 0.0));
    let mut rng = StdRng::seed_from_u64(27);
    let mut reaches_ground = false;
    for _ in 0..100 {
        let mut state = CameraPathState::new(ray(3.0), 0);
        trace(&j, &mut state, (Vector3::z(), 1.0), &mut rng);
        if state.medium_scattered && state.ray.direction.z < 0.0 {
            // It terminated at the surface at the ordinary surface budget, after shading.
            assert_eq!(state.path_depth, 0);
            assert!(state.radiance.direct.x > 0.0);
            assert_eq!(state.radiance.caustics, Vector3::zeros());
            reaches_ground = true;
        }
    }
    assert!(reaches_ground);
}

#[test]
fn uniform_environment_single_scatter_does_not_double_count_phase_escape_or_gain_energy() {
    let mut j = empty();
    j.scene.environment = Some(
        Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: [1.0; 3],
            zenith_linear_rgb: [1.0; 3],
            ..Default::default()
        })
        .unwrap(),
    );
    // A cubic volume, camera outside. Both view/lighting legs lose second-order paths.
    j.scene.medium = Some(
        HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: [-1.0; 3],
            max_m: [1.0; 3],
            sigma_a: 0.0,
            sigma_s: 0.4,
            g: 0.0,
        })
        .unwrap(),
    );
    for mis in [
        EnvironmentMisHeuristic::Balance,
        EnvironmentMisHeuristic::Power,
    ] {
        j.camera_options.environment_mis_heuristic = mis;
        j.camera_options.environment_samples = 2;
        let mut rng = StdRng::seed_from_u64(87);
        let mut mean = 0.0;
        for _ in 0..25_000 {
            let mut state = CameraPathState::new(ray(3.0), 0);
            trace(&j, &mut state, (Vector3::z(), 0.0), &mut rng);
            mean += state.radiance.direct.x / 25_000.0;
        }
        assert!(mean > 0.65 && mean < 1.0, "{mis:?}: {mean}");
    }
}

#[test]
fn volume_light_visibility_includes_geometry_and_finite_disk_distance() {
    let mut j = empty();
    j.scene.medium = Some(slab(0.0, 1.0, 0.0));
    let unblocked = rgb(&j, ray(3.0), (Vector3::x(), 1.0), 5);
    assert!(unblocked.x > 0.0);
    j.scene.add(
        Box::new(Sphere::new(Vector3::new(20.0, 0.0, 1.5), 19.0).unwrap()),
        Box::new(Lambertian::new(Vector3::zeros()).unwrap()),
    );
    let blocked = rgb(&j, ray(3.0), (Vector3::x(), 1.0), 5);
    assert_eq!(blocked, Vector3::zeros());
    let mut j = empty();
    ground(&mut j);
    j.scene.light =
        Light::DiskArea(DiskAreaLight::new(Vector3::z() * 3.0, -Vector3::z(), 1e-4, 1.0).unwrap());
    let no_medium = rgb(&j, ray(0.5), (Vector3::z(), 0.0), 0);
    j.scene.medium = Some(slab(0.6, 0.0, 0.0));
    let attenuated = rgb(&j, ray(0.5), (Vector3::z(), 0.0), 0);
    assert!((attenuated.x / no_medium.x - (-0.6_f64).exp()).abs() < 1e-8);
    // A box beyond the sampled emitter does not attenuate this finite shadow segment.
    j.scene.medium = Some(
        HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: [-1.0, -1.0, 4.0],
            max_m: [1.0, 1.0, 5.0],
            sigma_a: 10.0,
            sigma_s: 0.0,
            g: 0.0,
        })
        .unwrap(),
    );
    assert_eq!(rgb(&j, ray(0.5), (Vector3::z(), 0.0), 0), no_medium);
}

#[test]
fn surface_environment_shadow_attenuation_matches_hemisphere_quadrature() {
    let mut j = empty();
    ground(&mut j);
    j.scene.environment = Some(
        Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: [1.0; 3],
            zenith_linear_rgb: [1.0; 3],
            ..Default::default()
        })
        .unwrap(),
    );
    j.scene.medium = Some(slab(0.5, 0.0, 0.0));
    j.camera_options.environment_samples = 80_000;
    let value = rgb(&j, ray(0.5), (Vector3::z(), 0.0), 42).x;
    let expected: f64 = (0..100_000)
        .map(|i| {
            let mu = (i as f64 + 0.5) / 100_000.0;
            mu * (-0.5 / mu).exp() / 100_000.0 // albedo=0.5 cancels factor 2
        })
        .sum();
    assert!((value - expected).abs() < 0.003, "{value}/{expected}");
    assert!(value < 0.5);
}

#[test]
fn configured_fog_repeats_across_rgb_hero_packet_mis_threads_and_zero_density() {
    let mut c = Config::load("configs/homogeneous_fog.json").unwrap();
    c.render.width = 12;
    c.render.height = 8;
    c.render.camera_spp = 16;
    c.render.samples_per_pixel_axis = 1;
    for (mode, packet) in [
        (CameraMode::Rgb, 1),
        (CameraMode::Spectral, 1),
        (CameraMode::Spectral, 4),
    ] {
        c.render.camera_mode = mode;
        c.render.spectral_packet_size = packet;
        for mis in [
            EnvironmentMisHeuristic::Balance,
            EnvironmentMisHeuristic::Power,
        ] {
            c.scene.environment_mis_heuristic = mis;
            let job = c.build().unwrap();
            let mut reference = None;
            for threads in [1, 2, 4] {
                c.render.thread_count = Some(threads);
                let images = Renderer::new(c.render.clone())
                    .unwrap()
                    .render(&job)
                    .unwrap();
                assert_eq!(
                    images
                        .direct
                        .pixels()
                        .iter()
                        .zip(images.caustics.pixels())
                        .map(|(d, c)| d + c)
                        .collect::<Vec<_>>(),
                    images.combined.pixels()
                );
                if let Some(r) = &reference {
                    assert_eq!(images.combined.pixels(), r);
                } else {
                    reference = Some(images.combined.pixels().to_vec());
                }
            }
            let mut vacuum = c.clone();
            vacuum.scene.medium = None;
            let mut zero = c.clone();
            let m = zero.scene.medium.as_mut().unwrap();
            m.sigma_a = 0.0;
            m.sigma_s = 0.0;
            let render = |config: &Config| {
                Renderer::new(config.render.clone())
                    .unwrap()
                    .render(&config.build().unwrap())
                    .unwrap()
            };
            assert_eq!(
                render(&zero).combined.pixels(),
                render(&vacuum).combined.pixels()
            );
        }
    }
}

#[test]
fn medium_json_defaults_roundtrip_and_invalid_values_fail_building() {
    let mut c = Config::load("configs/homogeneous_fog.json").unwrap();
    assert_eq!(Config::from_json(&c.to_json().unwrap()).unwrap(), c);
    c.scene.medium.as_mut().unwrap().g = 1.0;
    assert!(c.validate().is_err());
    assert!(c.build().is_err());
    let old = Config::load("configs/default.json").unwrap();
    assert!(old.scene.medium.is_none());
}

#[test]
fn absorption_keeps_glass_events_and_attenuates_the_entire_delta_chain() {
    let mut j = empty();
    j.scene.background_color = Vector3::repeat(1.0);
    j.scene.add(
        Box::new(Sphere::new(Vector3::z() * 0.5, 0.4).unwrap()),
        Box::new(spectral_caustics::material::dielectric::Dielectric::new(1.52).unwrap()),
    );
    let mut reference = CameraPathState::new(ray(3.0), 0);
    trace(
        &j,
        &mut reference,
        (Vector3::z(), 0.0),
        &mut StdRng::seed_from_u64(1),
    );
    assert!(reference.path_depth >= 2);
    j.scene.medium = Some(slab(0.4, 0.0, 0.0));
    let mut attenuated = CameraPathState::new(ray(3.0), 0);
    trace(
        &j,
        &mut attenuated,
        (Vector3::z(), 0.0),
        &mut StdRng::seed_from_u64(1),
    );
    assert_eq!(reference.path_depth, attenuated.path_depth);
    assert_eq!(reference.last_scatter, attenuated.last_scatter);
    assert_eq!(reference.ray.direction, attenuated.ray.direction);
    let length = if reference.ray.direction.z < 0.0 {
        1.0
    } else {
        2.0
    };
    assert!(
        (attenuated.radiance.direct - reference.radiance.direct * (-0.4_f64 * length).exp()).norm()
            < 1e-14
    );
}

#[test]
fn volume_disk_matches_finite_distance_single_scattering_quadrature() {
    use spectral_caustics::integrator::camera::transport::Rgb;
    use std::f64::consts::PI;
    let mut j = empty();
    j.scene.medium = Some(slab(0.2, 0.5, 0.0));
    j.scene.light =
        Light::DiskArea(DiskAreaLight::new(Vector3::z() * 4.0, -Vector3::z(), 1e-5, 1.0).unwrap());
    j.camera_options.light_samples = 1;
    let mut rng = StdRng::seed_from_u64(871);
    let mut mean = Vector3::zeros();
    for _ in 0..40_000 {
        let mut state = CameraPathState::new(ray(3.0), 0);
        trace(&j, &mut state, (Vector3::z(), 0.0), &mut rng);
        mean += state.radiance.direct / 40_000.0;
    }
    let integral: f64 = (0..50_000)
        .map(|i| {
            let s = (i as f64 + 0.5) / 50_000.0;
            0.5 * (-1.4 * s).exp() / (4.0 * PI * PI * (2.0 + s).powi(2)) / 50_000.0
        })
        .sum();
    let expected = Rgb::default().disk_emission() * integral;
    assert!((mean - expected).norm() / expected.norm() < 0.025);
}

#[test]
fn adding_fog_preserves_existing_xyz_and_spectral_photon_maps() {
    use spectral_caustics::integrator::photon;
    let mut c = Config::load("configs/vertical_prism_spectral_caustics.json").unwrap();
    c.render.width = 12;
    c.render.height = 8;
    c.render.samples_per_pixel_axis = 2;
    let mut job = c.build().unwrap();
    let target = job.photon.caustics_object_id;
    let ground = job.photon.ground_id.unwrap();
    let xyz = photon::integrate_xyz(&job, target, ground, &c.render).unwrap();
    let spectral = photon::integrate_spectral(&job, &c.render)
        .unwrap()
        .unwrap();
    job.scene.medium = Some(slab(2.0, 4.0, 0.7));
    let fog_xyz = photon::integrate_xyz(&job, target, ground, &c.render).unwrap();
    let fog_spectral = photon::integrate_spectral(&job, &c.render)
        .unwrap()
        .unwrap();
    assert_eq!(xyz.caustics.pixels(), fog_xyz.caustics.pixels());
    assert_eq!(
        xyz.direct.unwrap().pixels(),
        fog_xyz.direct.unwrap().pixels()
    );
    assert_eq!(xyz.sun_irradiance_xyz, fog_xyz.sun_irradiance_xyz);
    for i in 0..spectral.wavelength_samples().len() {
        assert_eq!(
            spectral.layer_irradiance(i),
            fog_spectral.layer_irradiance(i)
        );
    }
}

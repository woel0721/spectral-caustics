//! Analytic and end-to-end checks of heterogeneous scattering through Medium.
use nalgebra::Vector3;
use rand::{RngCore, SeedableRng, rngs::StdRng};
use spectral_caustics::{
    config::{
        RenderJob,
        scene::{EnvironmentConfig, EnvironmentMisHeuristic},
    },
    environment::Environment,
    geometry::ray::Ray,
    integrator::camera::{
        path::trace_camera_path,
        transport::{CameraTransport, Caustics},
        types::{CameraPathRngs, CameraPathState},
    },
    medium::{GridMedium, GridMediumConfig},
    optics::dispersion::IorMode,
};

fn empty(c: &GridMediumConfig) -> RenderJob {
    let mut job = RenderJob::new();
    job.scene.environment = None;
    job.scene.background_color = Vector3::zeros();
    job.scene.medium = Some(Box::new(GridMedium::try_from(c).unwrap()));
    job
}

fn trace<T: CameraTransport>(
    job: &RenderJob,
    state: &mut CameraPathState<T>,
    sun: (Vector3<f64>, f64),
    rng: &mut StdRng,
) {
    let mut camera = StdRng::seed_from_u64(1);
    let mut environment = StdRng::seed_from_u64(2);
    let mut diffuse = StdRng::seed_from_u64(3);
    let mut untouched = [camera.clone(), environment.clone(), diffuse.clone()];
    trace_camera_path(
        job,
        state,
        &mut CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut diffuse,
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
    // Delta candidates, phase sampling and volume NEE only consume the medium stream.
    for (mut actual, expected) in [camera, environment, diffuse]
        .into_iter()
        .zip(&mut untouched)
    {
        assert_eq!(actual.next_u64(), expected.next_u64());
    }
}

#[test]
fn layered_grid_single_scattering_matches_independent_slab_integrals() {
    let mut job = empty(&GridMediumConfig {
        min_m: [-100.0, -100.0, 1.0],
        max_m: [100.0, 100.0, 3.0],
        resolution: [1, 1, 2],
        densities: vec![1.0, 3.0],
        sigma_a_base: 0.2,
        sigma_s_base: 0.3,
        g: 0.0,
    });
    job.camera_options.max_volume_scatters = 1;
    let ray = Ray::new(Vector3::z() * 4.0, -Vector3::z()).unwrap();
    for g in [0.0, 0.7] {
        // Same layered density and albedo; g changes only the directional factor.
        job.scene.medium = Some(Box::new(
            GridMedium::try_from(&GridMediumConfig {
                min_m: [-100.0, -100.0, 1.0],
                max_m: [100.0, 100.0, 3.0],
                resolution: [1, 1, 2],
                densities: vec![1.0, 3.0],
                sigma_a_base: 0.2,
                sigma_s_base: 0.3,
                g,
            })
            .unwrap(),
        ));
        for sign in [-1.0, 1.0] {
            let wi = Vector3::z() * sign;
            let phase = job
                .scene
                .medium
                .as_ref()
                .unwrap()
                .phase()
                .eval(Vector3::z(), wi);
            // Total tau = 0.5 * (1 + 3) = 2. On the camera side integrate
            // albedo * exp(-2 tau) d tau; opposite-side Tr is exp(-2) everywhere.
            let expected = phase
                * if sign > 0.0 {
                    0.3 * (1.0 - (-4.0_f64).exp())
                } else {
                    1.2 * (-2.0_f64).exp()
                };
            let mut mean = 0.0;
            let mut rng = StdRng::seed_from_u64(192);
            for _ in 0..50_000 {
                let mut state = CameraPathState::new(ray, 0);
                trace(&job, &mut state, (wi, 1.0), &mut rng);
                mean += state.radiance.direct.x / 50_000.0;
                assert!(state.volume_scatters <= 1);
                assert_eq!(state.path_depth, 0);
                assert_eq!(state.radiance.caustics, Vector3::zeros());
            }
            assert!(
                (mean / expected - 1.0).abs() < 0.025,
                "g={g}, sign={sign}: {mean} vs {expected}"
            );
        }
    }
}

#[test]
fn heterogeneous_multiple_scattering_preserves_albedo_geometry_budgets_and_scalar_weights() {
    let mut job = empty(&GridMediumConfig {
        min_m: [-1.0; 3],
        max_m: [1.0; 3],
        resolution: [2; 3],
        densities: vec![0.2, 2.0, 1.0, 3.0, 2.0, 0.5, 4.0, 1.5],
        sigma_a_base: 0.8,
        sigma_s_base: 3.2,
        g: 0.4,
    });
    let ray = Ray::new(Vector3::z() * 3.0, -Vector3::z()).unwrap();
    let d65 = spectral_caustics::spectrum::emission::d65_spectrum(550.0).unwrap();
    for budget in [0, 1, 2, 4, 8] {
        job.camera_options.max_volume_scatters = budget;
        let mut reached = false;
        for seed in 0..128 {
            let mut rgb = CameraPathState::new(ray, 0);
            let mut scalar = CameraPathState::spectral(ray, 550.0, 0).unwrap();
            let mut a = StdRng::seed_from_u64(seed);
            let mut b = a.clone();
            trace(&job, &mut rgb, (Vector3::z(), 1.0), &mut a);
            trace(&job, &mut scalar, (Vector3::z(), 1.0), &mut b);
            assert_eq!(rgb.volume_scatters, scalar.volume_scatters);
            assert!(rgb.volume_scatters <= budget);
            reached |= rgb.volume_scatters == budget;
            assert_eq!(rgb.ray.origin, scalar.ray.origin);
            assert_eq!(rgb.ray.direction, scalar.ray.direction);
            assert_eq!(a.next_u64(), b.next_u64());
            let mut expected = 0.8_f64.powi(rgb.volume_scatters as i32);
            if rgb.volume_scatters == budget {
                expected *= job
                    .scene
                    .medium
                    .as_ref()
                    .unwrap()
                    .transmittance_ray(&rgb.ray, f64::INFINITY);
            }
            assert!((rgb.throughput - Vector3::repeat(expected)).norm() < 1e-13);
            assert!((scalar.throughput - expected).abs() < 1e-13);
            assert!((scalar.radiance.direct - rgb.radiance.direct.x * d65).abs() < 1e-13);
            assert_eq!(rgb.path_depth, 0);
            assert_eq!(rgb.non_delta_bounces, 0);
            assert_eq!(rgb.radiance.caustics, Vector3::zeros());
        }
        assert!(reached, "never reached real scattering budget {budget}");
    }
}

#[test]
fn conservative_heterogeneous_fog_recovers_unit_environment_with_multiple_scattering() {
    let mut job = empty(&GridMediumConfig {
        min_m: [-1.0; 3],
        max_m: [1.0; 3],
        resolution: [2; 3],
        densities: vec![0.0, 0.2, 1.3, 2.0, 0.7, 1.8, 0.3, 1.1],
        sigma_a_base: 0.0,
        sigma_s_base: 0.7,
        g: 0.4,
    });
    job.scene.environment = Some(
        Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: [1.0; 3],
            zenith_linear_rgb: [1.0; 3],
            ..Default::default()
        })
        .unwrap(),
    );
    let ray = Ray::new(Vector3::new(0.5, 0.5, 3.0), -Vector3::z()).unwrap();
    for (mis, samples) in [
        (EnvironmentMisHeuristic::Balance, 1),
        (EnvironmentMisHeuristic::Power, 3),
    ] {
        job.camera_options.environment_mis_heuristic = mis;
        job.camera_options.environment_samples = samples;
        let mut means = Vec::new();
        for budget in [0, 1, 2, 8, 32] {
            job.camera_options.max_volume_scatters = budget;
            let mut rng = StdRng::seed_from_u64(313);
            let mut mean = 0.0;
            for _ in 0..12_000 {
                let mut state = CameraPathState::new(ray, 0);
                trace(&job, &mut state, (Vector3::z(), 0.0), &mut rng);
                mean += state.radiance.direct.x / 12_000.0;
                assert!(state.volume_scatters <= budget);
            }
            // Constant boundary radiance and no absorption have exact full L=1,
            // regardless of density. Finite budgets omit positive higher orders.
            assert!(
                mean <= 1.025,
                "energy gain: {mis:?}, budget={budget}: {mean}"
            );
            means.push(mean);
        }
        let tr = job
            .scene
            .medium
            .as_ref()
            .unwrap()
            .transmittance_ray(&ray, f64::INFINITY);
        assert!((means[0] - tr).abs() < 1e-12);
        assert!(means[1] > means[0] && means[2] > means[1] && means[3] > means[2]);
        assert!((means[4] - 1.0).abs() < 0.025, "{mis:?}: {means:?}");
    }
}

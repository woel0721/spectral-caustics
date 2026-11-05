use crate::integrator::camera::{
    path::trace_camera_path,
    transport::Caustics,
    types::{CameraPathRngs, CameraPathState},
};
// Controlled Diffuse -> Delta+ -> Environment paths and the NEE partition.
use super::SequenceRng;
use super::close;
use super::glass_patch;
use super::sun;
use super::two_planes;
use crate::config::RenderJob;
use crate::config::scene::EnvironmentConfig;
use crate::config::scene::EnvironmentMisHeuristic;
use crate::environment::Environment;
use crate::environment::EnvironmentMap;
use crate::geometry::ray::Ray;
use crate::geometry::triangle::Triangle;
use crate::integrator::camera::path::russian_roulette;
use crate::material::dielectric::Dielectric;
use nalgebra::Vector3;
use rand::SeedableRng;
use rand::rngs::StdRng;

const ALBEDO: Vector3<f64> = Vector3::new(0.8, 0.5, 0.2);
const SKY: Vector3<f64> = Vector3::new(2.0, 4.0, 8.0);

fn escape_scene(interfaces: usize) -> RenderJob {
    let mut scene = two_planes(ALBEDO, Vector3::repeat(0.7));
    scene.scene.objects.remove(1); // Remove the wall so the glass continuation escapes.
    scene.scene.environment = Some(
        Environment::try_from(&EnvironmentConfig {
            horizon_linear_rgb: SKY.into(),
            zenith_linear_rgb: SKY.into(),
            ..Default::default()
        })
        .unwrap(),
    );
    for (x, outward_x) in [(0.25, false), (0.35, true)].into_iter().take(interfaces) {
        scene.scene.add(
            Box::new(glass_patch(x, x * 3.0_f64.sqrt(), outward_x)),
            Box::new(Dielectric::new(1.5).unwrap()),
        );
    }
    scene
}

fn trace(scene: &RenderJob, environment_words: &[f64], diffuse_words: &[f64]) -> CameraPathState {
    let mut state = CameraPathState::new(
        Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(),
        scene.camera_options.max_diffuse_bounces,
    );
    let mut camera = SequenceRng::new(&[]);
    let mut environment = SequenceRng::new(environment_words);
    let mut diffuse = SequenceRng::new(diffuse_words);
    trace_camera_path(
        scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut diffuse,
        },
        sun().0,
        0.0,
        (
            // Isolate the environment from the analytic sun.
            None
        )
            .map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();
    assert_eq!(camera.draws, 0);
    assert_eq!(environment.draws, environment_words.len());
    assert_eq!(diffuse.draws, diffuse_words.len());
    close(state.radiance.caustics, Vector3::zeros());
    state
}

#[test]
fn transmission_escape_uses_rgb_throughput_and_final_gradient_direction() {
    let mut scene = escape_scene(1);
    {
        let env = scene.scene.environment.as_mut().unwrap();
        env.set_gradient(env.horizon_color(), Vector3::new(10.0, 6.0, 2.0))
            .unwrap();
    }
    for intensity in [0.0, 0.25, 3.0] {
        scene
            .scene
            .environment
            .as_mut()
            .unwrap()
            .set_intensity(intensity)
            .unwrap();
        // NEE and continuation choose the same direction. NEE is blocked by glass.
        let state = trace(&scene, &[0.25, 0.0], &[0.25, 0.0, 0.99]);
        assert_eq!((state.non_delta_bounces, state.delta_events()), (1, 1));
        assert_eq!(
            state.last_scatter,
            Some(crate::material::ScatterType::SpecularTransmission)
        );
        assert!(scene.scene.intersect(&state.ray).is_none());
        let direction = Vector3::new((2.0_f64 / 3.0).sqrt(), 0.0, 1.0 / 3.0_f64.sqrt());
        close(state.ray.direction, direction);
        close(state.throughput, ALBEDO * (4.0 / 9.0));
        let li =
            (SKY * (1.0 - direction.z) + Vector3::new(10.0, 6.0, 2.0) * direction.z) * intensity;
        close(
            state.radiance.direct,
            (ALBEDO * (4.0 / 9.0)).component_mul(&li),
        );
    }
}

#[test]
fn reflection_escape_has_unit_fresnel_weight_and_no_extra_probability_factor() {
    let scene = escape_scene(2);
    let state = trace(&scene, &[0.25, 0.0], &[0.25, 0.0, 0.0]);
    assert_eq!((state.non_delta_bounces, state.delta_events()), (1, 1));
    assert_eq!(
        state.last_scatter,
        Some(crate::material::ScatterType::SpecularReflection)
    );
    close(
        state.ray.direction,
        Vector3::new(-0.5, 0.0, 0.75_f64.sqrt()),
    );
    close(state.throughput, ALBEDO);
    close(state.radiance.direct, ALBEDO.component_mul(&SKY));
}

#[test]
fn two_transmission_interfaces_cancel_eta_squared_in_escape_radiance() {
    let scene = escape_scene(2);
    let state = trace(&scene, &[0.25, 0.0], &[0.25, 0.0, 0.99, 0.99]);
    assert_eq!((state.non_delta_bounces, state.delta_events()), (1, 2));
    assert_eq!(
        state.last_scatter,
        Some(crate::material::ScatterType::SpecularTransmission)
    );
    close(state.ray.direction, Vector3::new(0.5, 0.0, 0.75_f64.sqrt()));
    close(state.throughput, ALBEDO); // (1/1.5)^2 * (1.5/1)^2 = 1.
    close(state.radiance.direct, ALBEDO.component_mul(&SKY));
}

#[test]
fn no_environment_keeps_camera_background_out_of_mixed_escape_lighting() {
    let mut scene = escape_scene(2);
    scene.scene.environment = None;
    scene.scene.background_color = Vector3::repeat(1000.0);
    for words in [&[0.25, 0.0, 0.0][..], &[0.25, 0.0, 0.99, 0.99][..]] {
        let state = trace(&scene, &[], words);
        assert_eq!(
            state.last_scatter,
            Some(if words.len() == 3 {
                crate::material::ScatterType::SpecularReflection
            } else {
                crate::material::ScatterType::SpecularTransmission
            })
        );
        assert_eq!(state.non_delta_bounces, 1);
        close(state.radiance.direct, Vector3::zeros());
    }
}

#[test]
fn diffuse_escape_is_counted_only_by_nee_even_with_a_glass_prefix() {
    for prefix in [false, true] {
        let mut scene = escape_scene(0);
        if prefix {
            // Primary transmission hits the floor; its cosine ray misses this small patch.
            scene.scene.add(
                Box::new(
                    Triangle::new(
                        Vector3::new(-0.05, -0.05, 0.25),
                        Vector3::new(0.05, -0.05, 0.25),
                        Vector3::new(0.0, 0.05, 0.25),
                    )
                    .unwrap(),
                ),
                Box::new(Dielectric::new(1.5).unwrap()),
            );
        }
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 1);
        let mut camera = SequenceRng::new(if prefix { &[0.99] } else { &[] });
        let mut environment = SequenceRng::new(&[0.25, 0.0]);
        let mut diffuse = SequenceRng::new(&[0.25, 0.0]);
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut camera,
                environment: &mut environment,
                diffuse: &mut diffuse,
            },
            sun().0,
            0.0,
            (None).map_or(Caustics::None, Caustics::Rgb),
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        assert_eq!(
            (state.non_delta_bounces, state.delta_events()),
            (1, usize::from(prefix))
        );
        assert_eq!(
            state.last_scatter,
            Some(crate::material::ScatterType::DiffuseReflection)
        );
        close(
            state.radiance.direct,
            ALBEDO.component_mul(&SKY) * if prefix { 4.0 / 9.0 } else { 1.0 },
        );
        assert_eq!(
            (camera.draws, environment.draws, diffuse.draws),
            (usize::from(prefix), 2, 2)
        );
    }
}

#[test]
fn diffuse_after_glass_resets_escape_eligibility_and_keeps_secondary_nee() {
    let b = Vector3::new(0.4, 0.7, 0.9);
    let mut scene = escape_scene(2);
    let mut wall_scene = two_planes(ALBEDO, b);
    let wall = wall_scene.scene.objects.pop().unwrap().geometry;
    scene.scene.add(
        wall,
        Box::new(crate::material::lambertian::Lambertian::new(b).unwrap()),
    );
    scene.camera_options.max_diffuse_bounces = 2;
    let state = trace(
        &scene,
        &[0.25, 0.0],
        &[
            0.25, 0.0, 0.99, 0.99, // A -> two glass interfaces -> B
            0.25, 0.75, // B's environment NEE escapes above the glass.
            0.25, 0.75, // B's continuation also escapes; must not add it again.
        ],
    );
    assert_eq!((state.non_delta_bounces, state.delta_events()), (2, 2));
    assert_eq!(
        state.last_scatter,
        Some(crate::material::ScatterType::DiffuseReflection)
    );
    assert!(scene.scene.intersect(&state.ray).is_none());
    close(
        state.radiance.direct,
        ALBEDO.component_mul(&b).component_mul(&SKY),
    );
}

#[test]
fn glass_escape_preserves_prior_radiance_and_roulette_compensated_throughput() {
    let scene = escape_scene(2);
    let mut state = CameraPathState::new(
        Ray::new(
            Vector3::new(0.0, 0.0, 1e-5),
            Vector3::new(0.5, 0.0, 0.75_f64.sqrt()),
        )
        .unwrap(),
        5,
    );
    for _ in 0..3 {
        assert!(
            state
                .apply_scatter(crate::material::ScatterType::DiffuseReflection, ALBEDO)
                .unwrap()
        );
    }
    let prefix = ALBEDO.component_mul(&ALBEDO).component_mul(&ALBEDO);
    let mut diffuse = SequenceRng::new(&[0.0, 0.99, 0.99]); // RR survives, then two transmissions.
    assert!(russian_roulette(&mut state, &mut diffuse).unwrap());
    state.radiance.direct = Vector3::repeat(17.0);
    state.radiance.caustics = Vector3::repeat(3.0);
    let mut camera = SequenceRng::new(&[]);
    let mut environment = SequenceRng::new(&[]);
    trace_camera_path(
        &scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut diffuse,
        },
        sun().0,
        0.0,
        (None).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();
    assert_eq!((state.non_delta_bounces, state.delta_events()), (3, 2));
    close(state.throughput, prefix / prefix.max());
    close(
        state.radiance.direct,
        Vector3::repeat(17.0) + (prefix / prefix.max()).component_mul(&SKY),
    );
    close(state.radiance.caustics, Vector3::repeat(3.0));
    assert_eq!((camera.draws, environment.draws, diffuse.draws), (0, 0, 3));
}

#[test]
fn mixed_escape_queries_rotated_hdri_with_intensity_without_new_rng_draws() {
    let path = std::env::temp_dir().join(format!("f2b-escape-{}.hdr", std::process::id()));
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n".to_vec();
    for _ in 0..2 {
        for rgb in [
            [128, 32, 16, 132],
            [16, 128, 32, 132],
            [32, 16, 128, 132],
            [64, 64, 64, 132],
        ] {
            bytes.extend_from_slice(&rgb);
        }
    }
    std::fs::write(&path, bytes).unwrap();
    let map = EnvironmentMap::load(&path).unwrap();
    let direction = Vector3::new(-0.5, 0.0, 0.75_f64.sqrt());
    let mut results = Vec::new();
    for rotation in [0.0_f64, 90.0] {
        for heuristic in [
            EnvironmentMisHeuristic::Balance,
            EnvironmentMisHeuristic::Power,
        ] {
            let mut scene = escape_scene(2);
            scene.scene.environment = Some(
                Environment::try_from(&EnvironmentConfig {
                    map_path: Some(path.clone()),
                    rotation_degrees: rotation,
                    intensity: 2.5,
                    ..Default::default()
                })
                .unwrap(),
            );
            scene.camera_options.environment_mis_heuristic = heuristic;
            // N=1 selects the BSDF strategy, whose shadow ray hits glass.
            let state = trace(&scene, &[0.0, 0.25, 0.0], &[0.25, 0.0, 0.0]);
            let (sin, cos) = rotation.to_radians().sin_cos();
            let local = Vector3::new(cos * direction.x, -sin * direction.x, direction.z);
            let expected = ALBEDO.component_mul(&(map.radiance(local).unwrap() * 2.5));
            close(state.ray.direction, direction);
            close(state.radiance.direct, expected);
            results.push(state.radiance.direct);
        }
    }
    close(results[0], results[1]);
    close(results[2], results[3]);
    assert!((results[0] - results[2]).norm() > 0.1);

    // With both MIS strategies active, keep all local NEE and add exactly the
    // glass escape. The same seeded environment samples provide the depth-0 reference.
    for samples in [3, 8] {
        for heuristic in [
            EnvironmentMisHeuristic::Balance,
            EnvironmentMisHeuristic::Power,
        ] {
            let mut scene = escape_scene(2);
            scene.scene.environment = Some(
                Environment::try_from(&EnvironmentConfig {
                    map_path: Some(path.clone()),
                    rotation_degrees: 90.0,
                    intensity: 2.5,
                    ..Default::default()
                })
                .unwrap(),
            );
            scene.camera_options.environment_samples = samples;
            scene.camera_options.environment_mis_heuristic = heuristic;
            let mut nee = Vector3::zeros();
            for depth in [0, 1] {
                scene.camera_options.max_diffuse_bounces = depth;
                let mut state = CameraPathState::new(
                    Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(),
                    depth,
                );
                let mut camera = SequenceRng::new(&[]);
                let mut environment = StdRng::seed_from_u64(77);
                let mut diffuse =
                    SequenceRng::new(if depth == 0 { &[] } else { &[0.25, 0.0, 0.0] });
                trace_camera_path(
                    &scene,
                    &mut state,
                    &mut CameraPathRngs {
                        camera: &mut camera,
                        environment: &mut environment,
                        diffuse: &mut diffuse,
                    },
                    sun().0,
                    0.0,
                    (None).map_or(Caustics::None, Caustics::Rgb),
                    crate::optics::dispersion::IorMode::Constant,
                    #[cfg(feature = "diagnostics")]
                    None,
                )
                .unwrap();
                if depth == 0 {
                    nee = state.radiance.direct;
                    assert!(nee.norm() > 0.0);
                } else {
                    close(state.radiance.direct, nee + results[2]);
                }
                assert_eq!(camera.draws, 0);
                assert_eq!(diffuse.draws, depth * 3);
            }
        }
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mixed_environment_escape_respects_the_delta_event_limit() {
    for count in [
        crate::integrator::camera::path::MAX_DELTA_EVENTS,
        crate::integrator::camera::path::MAX_DELTA_EVENTS + 1,
    ] {
        let mut scene = escape_scene(0);
        for i in 1..=count {
            let x = i as f64 / (2.0 * crate::integrator::camera::path::MAX_DELTA_EVENTS as f64);
            scene.scene.add(
                Box::new(glass_patch(x, x * 3.0_f64.sqrt(), false)),
                Box::new(Dielectric::new(1.0).unwrap()),
            );
        }
        let state = trace(&scene, &[0.25, 0.0], &[0.25, 0.0]);
        assert_eq!(
            (state.non_delta_bounces, state.delta_events()),
            (1, crate::integrator::camera::path::MAX_DELTA_EVENTS)
        );
        assert_eq!(
            state.last_scatter,
            Some(crate::material::ScatterType::SpecularTransmission)
        );
        close(
            state.radiance.direct,
            if count == crate::integrator::camera::path::MAX_DELTA_EVENTS {
                ALBEDO.component_mul(&SKY)
            } else {
                Vector3::zeros()
            },
        );
    }
}

#[test]
fn mixed_escape_reports_radiance_overflow() {
    {
        let (intensity, throughput) = (1.0, f64::MAX);
        let mut scene = RenderJob::new();
        let sky = scene.scene.environment.as_mut().unwrap();
        sky.set_intensity(intensity).unwrap();
        sky.set_gradient(SKY, SKY).unwrap();
        let mut state = CameraPathState::new(Ray::new(Vector3::zeros(), Vector3::z()).unwrap(), 1);
        assert!(
            state
                .apply_scatter(
                    crate::material::ScatterType::DiffuseReflection,
                    Vector3::repeat(throughput)
                )
                .unwrap()
        );
        assert!(
            state
                .apply_scatter(
                    crate::material::ScatterType::SpecularTransmission,
                    Vector3::repeat(1.0)
                )
                .unwrap()
        );
        let error = trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut SequenceRng::new(&[]),
                environment: &mut SequenceRng::new(&[]),
                diffuse: &mut SequenceRng::new(&[]),
            },
            sun().0,
            0.0,
            (None).map_or(Caustics::None, Caustics::Rgb),
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}

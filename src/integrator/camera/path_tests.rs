use crate::integrator::camera::{
    path::trace_camera_path,
    transport::Caustics,
    types::{CameraPathRngs, CameraPathState},
};
// Unified path state, mixed glass/diffuse transport and independent budgets.
use super::diffuse_tests::SequenceRng;
use super::diffuse_tests::close;
use super::diffuse_tests::sun;
use super::diffuse_tests::two_planes;
use crate::geometry::ray::INTERSECTION_EPSILON;
use crate::geometry::ray::Ray;
use crate::geometry::triangle::Triangle;
use crate::material::dielectric::Dielectric;
use nalgebra::Vector3;

mod environment_escape;

#[test]
fn glass_prefix_and_deep_diffuse_keep_independent_counts_and_roulette_order() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    scene.camera_options.max_diffuse_bounces = 5;
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
    let (wi, light) = sun();
    for survive in [false, true] {
        let mut words = vec![0.25, 0.0, 0.75, 0.25, 0.25, 0.0];
        if survive {
            words.extend([0.0, 0.75, 0.25, 0.0, 0.25, 0.0]);
        } else {
            words.push(0.99);
        }
        let mut camera = SequenceRng::new(&[0.9]);
        let mut environment = SequenceRng::new(&[]);
        let mut diffuse = SequenceRng::new(&words);
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 5);
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut camera,
                environment: &mut environment,
                diffuse: &mut diffuse,
            },
            wi,
            light,
            (None).map_or(Caustics::None, Caustics::Rgb),
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        assert_eq!(state.delta_events(), 1);
        assert_eq!(state.non_delta_bounces, if survive { 5 } else { 3 });
        assert_eq!(
            state.last_scatter,
            Some(crate::material::ScatterType::DiffuseReflection)
        );
        assert_eq!(camera.draws, 1);
        assert_eq!(environment.draws, 0);
        assert_eq!(diffuse.draws, if survive { 12 } else { 7 });
        assert!(
            state
                .radiance
                .direct
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
        );
        close(state.radiance.caustics, Vector3::zeros());
    }
}

#[test]
fn diffuse_to_glass_continues_with_an_independent_glass_event() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    scene.camera_options.max_diffuse_bounces = 2;
    scene.scene.add(
        Box::new(
            Triangle::new(
                Vector3::new(-0.9, -0.3, 0.05),
                Vector3::new(-0.5, -0.3, 0.05),
                Vector3::new(-0.7, 0.3, 0.05),
            )
            .unwrap(),
        ),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let mut camera = SequenceRng::new(&[]);
    let mut environment = SequenceRng::new(&[]);
    let mut diffuse = SequenceRng::new(&[0.25, 0.0, 0.5, 0.25, 0.99]);
    let mut state = CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 2);
    let (wi, light) = sun();
    trace_camera_path(
        &scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut diffuse,
        },
        wi,
        light,
        (None).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();
    assert_eq!(state.non_delta_bounces, 2);
    assert_eq!(state.delta_events(), 1);
    assert_eq!(
        state.last_scatter,
        Some(crate::material::ScatterType::SpecularTransmission)
    );
    assert_eq!(camera.draws, 0);
    assert_eq!(environment.draws, 0);
    assert_eq!(diffuse.draws, 5);
    assert!(state.radiance.direct.norm() > 0.0);
}

#[test]
fn first_surface_is_shaded_before_continuation_sampling() {
    use crate::material::BsdfSample;
    use crate::material::Material;
    use rand::RngCore;
    struct InvalidDirect;
    impl Material for InvalidDirect {
        fn evaluate_rgb(&self, _: Vector3<f64>, _: Vector3<f64>, _: Vector3<f64>) -> Vector3<f64> {
            Vector3::repeat(f64::NAN)
        }
        fn sample(
            &self,
            _: Vector3<f64>,
            _: Vector3<f64>,
            _: &mut dyn RngCore,
        ) -> Option<BsdfSample> {
            panic!("a failed first-surface evaluation must not issue a continuation");
        }
    }
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    let geometry = scene.scene.objects.remove(0).geometry;
    scene.scene.add(geometry, Box::new(InvalidDirect));
    scene.camera_options.max_diffuse_bounces = 4;
    let mut state = CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 4);
    let mut camera = SequenceRng::new(&[]);
    let mut environment = SequenceRng::new(&[]);
    let mut diffuse = SequenceRng::new(&[]);
    let error = trace_camera_path(
        &scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut diffuse,
        },
        sun().0,
        sun().1,
        (None).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("BSDF"));
    assert_eq!(state.non_delta_bounces, 0);
    assert_eq!(state.last_scatter, None);
    assert_eq!(diffuse.draws, 0);
}

#[test]
fn event_state_tracks_camera_delta_and_diffuse_with_independent_budgets() {
    for glass_prefix in [false, true] {
        for depth in [0_usize, 1, 2, 4] {
            let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
            scene.camera_options.max_diffuse_bounces = depth;
            if glass_prefix {
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
                CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), depth);
            assert_eq!(state.last_scatter, None);
            let mut camera = SequenceRng::new(if glass_prefix { &[0.9] } else { &[] });
            let mut environment = SequenceRng::new(&[]);
            // Fourth diffuse connection is issued only after a surviving RR draw.
            let mut diffuse =
                SequenceRng::new(&[0.25, 0.0, 0.75, 0.25, 0.25, 0.0, 0.0, 0.75, 0.25]);
            trace_camera_path(
                &scene,
                &mut state,
                &mut CameraPathRngs {
                    camera: &mut camera,
                    environment: &mut environment,
                    diffuse: &mut diffuse,
                },
                sun().0,
                sun().1,
                (None).map_or(Caustics::None, Caustics::Rgb),
                crate::optics::dispersion::IorMode::Constant,
                #[cfg(feature = "diagnostics")]
                None,
            )
            .unwrap();
            assert_eq!(state.delta_events(), usize::from(glass_prefix));
            assert_eq!(state.non_delta_bounces, depth);
            assert_eq!(
                state.last_scatter,
                if depth > 0 {
                    Some(crate::material::ScatterType::DiffuseReflection)
                } else if glass_prefix {
                    Some(crate::material::ScatterType::SpecularTransmission)
                } else {
                    None
                }
            );
            assert_eq!(camera.draws, usize::from(glass_prefix));
            assert_eq!(environment.draws, 0);
            assert_eq!(diffuse.draws, depth * 2 + depth.saturating_sub(3));
            close(state.radiance.caustics, Vector3::zeros());
        }
    }
}

#[test]
fn glass_event_limit_does_not_consume_the_diffuse_budget() {
    use crate::config::RenderJob;
    use crate::geometry::ground::Ground;
    for count in [
        super::path::MAX_DELTA_EVENTS,
        super::path::MAX_DELTA_EVENTS + 1,
    ] {
        let mut scene = RenderJob::new();
        scene.scene.environment = None;
        scene.camera_options.max_diffuse_bounces = 4;
        for height in 1..=count {
            scene.scene.add(
                Box::new(Ground::new(height as f64).unwrap()),
                Box::new(Dielectric::new(1.0).unwrap()),
            );
        }
        let mut state = CameraPathState::new(
            Ray::new(
                Vector3::z() * (super::path::MAX_DELTA_EVENTS as f64 + 4.0),
                -Vector3::z(),
            )
            .unwrap(),
            4,
        );
        let mut camera = SequenceRng::new(&[]); // IOR=1 transmits deterministically.
        let mut environment = SequenceRng::new(&[]);
        let mut diffuse = SequenceRng::new(&[]);
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut camera,
                environment: &mut environment,
                diffuse: &mut diffuse,
            },
            sun().0,
            sun().1,
            (None).map_or(Caustics::None, Caustics::Rgb),
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        assert_eq!(
            state.delta_events(),
            crate::integrator::camera::path::MAX_DELTA_EVENTS
        );
        assert_eq!(state.non_delta_bounces, 0);
        assert_eq!(
            state.last_scatter,
            Some(crate::material::ScatterType::SpecularTransmission)
        );
        close(
            state.radiance.direct,
            if count == crate::integrator::camera::path::MAX_DELTA_EVENTS {
                scene.scene.background_color
            } else {
                Vector3::zeros()
            },
        );
        close(state.radiance.caustics, Vector3::zeros());
        assert_eq!((camera.draws, environment.draws, diffuse.draws), (0, 0, 0));
    }
}

/// A thin glass patch seen by the cosine-sampled ray from the floor.
fn glass_patch(x: f64, height: f64, positive_x_normal: bool) -> Triangle {
    let a = Vector3::new(x, -0.5, height - 0.2);
    let top = Vector3::new(x, 0.0, height + 0.2);
    let b = Vector3::new(x, 0.5, height - 0.2);
    if positive_x_normal {
        Triangle::new(a, b, top).unwrap()
    } else {
        Triangle::new(a, top, b).unwrap()
    }
}

#[test]
fn diffuse_glass_diffuse_crosses_two_fresnel_interfaces_and_preserves_rgb_weight() {
    let a = Vector3::new(0.8, 0.5, 0.2);
    let b = Vector3::new(0.4, 0.7, 0.9);
    let mut scene = two_planes(a, b);
    scene.camera_options.max_diffuse_bounces = 1;
    // The primary camera ray hits A. A's chosen cosine ray crosses both
    // interfaces (air -> glass -> air) before it reaches the lit wall B.
    for (x, outward_x) in [(0.25, false), (0.35, true)] {
        scene.scene.add(
            Box::new(glass_patch(x, x * 3.0_f64.sqrt(), outward_x)),
            Box::new(Dielectric::new(1.5).unwrap()),
        );
    }
    let mut camera = SequenceRng::new(&[]);
    let mut environment = SequenceRng::new(&[]);
    let mut diffuse = SequenceRng::new(&[0.25, 0.0, 0.99, 0.99]); // Diffuse, then two transmissions.
    let mut state = CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 1);
    let (wi, light) = sun();
    trace_camera_path(
        &scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut diffuse,
        },
        wi,
        light,
        (None).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();

    assert_eq!(state.non_delta_bounces, 1);
    assert_eq!(state.delta_events(), 2);
    assert_eq!(
        state.last_scatter,
        Some(crate::material::ScatterType::SpecularTransmission)
    );
    assert_eq!((camera.draws, environment.draws, diffuse.draws), (0, 0, 4));
    // Lambertian A contributes albedo A; entering and leaving the slab
    // multiply by (1/1.5)^2 and (1.5/1)^2, canceling in radiance transport.
    close(state.throughput, a);
    // Snell gives dz/dx=1/sqrt(2) inside this slab, not the incident sqrt(3).
    // Check the actual exit point and the restored outgoing direction, including offsets.
    let epsilon = 10.0 * INTERSECTION_EPSILON;
    let exit_z = epsilon + 0.25 * 3.0_f64.sqrt() + (0.1 - epsilon) / 2.0_f64.sqrt();
    close(state.ray.origin, Vector3::new(0.35 + epsilon, 0.0, exit_z));
    close(state.ray.direction, Vector3::new(0.5, 0.0, 0.75_f64.sqrt()));
    assert_eq!(scene.scene.intersect(&state.ray).unwrap().object_id, 1);
    let k = light * wi.z / std::f64::consts::PI;
    close(state.radiance.direct, (a + a.component_mul(&b)) * k);
    close(state.radiance.caustics, Vector3::zeros());
}

#[test]
fn glass_budget_after_diffuse_still_respects_the_delta_event_limit() {
    let a = Vector3::new(0.8, 0.5, 0.2);
    let b = Vector3::new(0.4, 0.7, 0.9);
    let (wi, light) = sun();
    let k = light * wi.z / std::f64::consts::PI;
    for (prefix, total) in [
        (false, super::path::MAX_DELTA_EVENTS),
        (false, super::path::MAX_DELTA_EVENTS + 1),
        (true, super::path::MAX_DELTA_EVENTS),
        (true, super::path::MAX_DELTA_EVENTS + 1),
    ] {
        let mut scene = two_planes(a, b);
        scene.camera_options.max_diffuse_bounces = 1;
        if prefix {
            scene.scene.add(
                Box::new(
                    Triangle::new(
                        Vector3::new(-0.05, -0.05, 0.25),
                        Vector3::new(0.05, -0.05, 0.25),
                        Vector3::new(0.0, 0.05, 0.25),
                    )
                    .unwrap(),
                ),
                Box::new(Dielectric::new(1.0).unwrap()),
            );
        }
        for i in 1..=total - usize::from(prefix) {
            let x = i as f64 / (2.0 * super::path::MAX_DELTA_EVENTS as f64);
            scene.scene.add(
                Box::new(glass_patch(x, x * 3.0_f64.sqrt(), false)),
                Box::new(Dielectric::new(1.0).unwrap()),
            );
        }
        let mut camera = SequenceRng::new(&[]); // Equal IORs: no Fresnel RNG draw.
        let mut environment = SequenceRng::new(&[]);
        let mut diffuse = SequenceRng::new(&[0.25, 0.0]);
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 1);
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut camera,
                environment: &mut environment,
                diffuse: &mut diffuse,
            },
            wi,
            light,
            (None).map_or(Caustics::None, Caustics::Rgb),
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        assert_eq!(state.non_delta_bounces, 1);
        assert_eq!(
            state.delta_events(),
            crate::integrator::camera::path::MAX_DELTA_EVENTS
        );
        assert_eq!(
            state.last_scatter,
            Some(crate::material::ScatterType::SpecularTransmission)
        );
        assert_eq!((camera.draws, environment.draws, diffuse.draws), (0, 0, 2));
        close(state.throughput, a);
        let expected = if total == crate::integrator::camera::path::MAX_DELTA_EVENTS {
            a + a.component_mul(&b)
        } else {
            a
        };
        close(state.radiance.direct, expected * k);
        close(state.radiance.caustics, Vector3::zeros());
    }
}

#[test]
fn diffuse_glass_reflection_keeps_unit_weight_and_excludes_escape_background() {
    let a = Vector3::new(0.8, 0.5, 0.2);
    let mut scene = two_planes(a, Vector3::new(0.4, 0.7, 0.9));
    scene.scene.background_color = Vector3::repeat(1000.0);
    for (x, outward_x) in [(0.25, false), (0.35, true)] {
        scene.scene.add(
            Box::new(glass_patch(x, x * 3.0_f64.sqrt(), outward_x)),
            Box::new(Dielectric::new(1.5).unwrap()),
        );
    }
    let mut camera = SequenceRng::new(&[]);
    let mut environment = SequenceRng::new(&[]);
    let mut diffuse = SequenceRng::new(&[0.25, 0.0, 0.0]); // Select reflection, not transmission.
    let mut state = CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 1);
    trace_camera_path(
        &scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut diffuse,
        },
        sun().0,
        sun().1,
        (None).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();
    assert_eq!((state.non_delta_bounces, state.delta_events()), (1, 1));
    assert_eq!(
        state.last_scatter,
        Some(crate::material::ScatterType::SpecularReflection)
    );
    assert_eq!((camera.draws, environment.draws, diffuse.draws), (0, 0, 3));
    close(state.throughput, a); // Reflection weight is F/F=1, with no extra Fresnel factor.
    close(
        state.ray.direction,
        Vector3::new(-0.5, 0.0, 0.75_f64.sqrt()),
    );
    assert!(scene.scene.intersect(&state.ray).is_none());
    // Without an environment, a later Delta must not turn camera background into illumination.
    close(
        state.radiance.direct,
        a * (sun().1 * sun().0.z / std::f64::consts::PI),
    );
    close(state.radiance.caustics, Vector3::zeros());
}

#[test]
fn diffuse_mirror_escape_uses_rgb_sample_weight_once_and_keeps_rng_streams() {
    use crate::material::mirror::Mirror;
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let reflectance = Vector3::new(0.9, 0.6, 0.3);
    let sky = Vector3::new(1.2, 1.5, 2.0);
    let mut scene = two_planes(albedo, Vector3::repeat(0.7));
    let mut environment = crate::config::RenderJob::new().scene.environment.unwrap();
    environment.set_gradient(sky, sky).unwrap();
    scene.scene.environment = Some(environment);
    scene.scene.add(
        Box::new(glass_patch(0.2, 0.2 * 3.0_f64.sqrt(), false)),
        Box::new(Mirror::new(reflectance).unwrap()),
    );
    let mut state = CameraPathState::new(
        crate::geometry::ray::Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(),
        1,
    );
    let (mut camera, mut environment, mut continuation) = (
        SequenceRng::new(&[]),
        SequenceRng::new(&[0.25, 0.5]),
        SequenceRng::new(&[0.25, 0.0]),
    );
    trace_camera_path(
        &scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut continuation,
        },
        sun().0,
        0.0,
        (None).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();
    assert_eq!(state.path_depth, 2);
    assert_eq!(state.non_delta_bounces, 1);
    assert_eq!(
        state.last_scatter,
        Some(crate::material::ScatterType::SpecularReflection)
    );
    close(state.throughput, albedo.component_mul(&reflectance));
    close(
        state.radiance.direct,
        albedo.component_mul(&sky) + albedo.component_mul(&reflectance).component_mul(&sky),
    );
    close(state.radiance.caustics, Vector3::zeros());
    assert_eq!(
        (camera.draws, environment.draws, continuation.draws),
        (0, 2, 2)
    );
}

mod materials;

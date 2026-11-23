use crate::integrator::camera::{
    path::trace_camera_path,
    transport::Caustics,
    types::{CameraPathRngs, CameraPathState},
};
// Unified path state, mixed glass/diffuse transport and independent budgets.
use super::support::SequenceRng;
use super::support::close;
use super::support::sun;
use super::support::two_planes;
use crate::geometry::ray::INTERSECTION_EPSILON;
use crate::geometry::ray::Ray;
use crate::geometry::sphere::Sphere;
use crate::geometry::triangle::Triangle;
use crate::geometry::vertical_prism::VerticalPrism;
use crate::material::dielectric::Dielectric;
use crate::medium::{HomogeneousMedium, HomogeneousMediumConfig};
use crate::scene::{MediumInterface, MediumRegion};
use nalgebra::{Vector2, Vector3};

#[test]
fn volume_scatter_near_sphere_enters_before_crossing_the_interior() {
    let mut job = crate::config::RenderJob::new();
    job.scene.environment = None;
    job.camera_options.max_volume_scatters = 1;
    let sphere = job.scene.add(
        Box::new(Sphere::new(Vector3::zeros(), 1.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let fog = HomogeneousMediumConfig {
        min_m: [-3.0; 3],
        max_m: [3.0; 3],
        sigma_a: 0.0.into(),
        sigma_s: 1.0.into(),
        g: (-0.5).into(),
    };
    job.scene.medium = Some(Box::new(HomogeneousMedium::try_from(&fog).unwrap()));
    let absorption = HomogeneousMediumConfig {
        sigma_a: 0.7.into(),
        sigma_s: 0.0.into(),
        ..fog
    };
    job.scene
        .bind_medium_interface(
            sphere,
            MediumInterface {
                interior: Some(Box::new(HomogeneousMedium::try_from(&absorption).unwrap())),
            },
        )
        .unwrap();

    let ray = Ray::new(Vector3::z() * 2.0, -Vector3::z()).unwrap();
    let mut state = CameraPathState::new(ray, 0);
    // Scatter half an epsilon before the sphere, then sample exact forward HG.
    let gap = 0.5 * INTERSECTION_EPSILON;
    let free_flight = 1.0 - gap;
    let mut medium = SequenceRng::new(&[-(-free_flight).exp_m1(), 0.0, 0.0]);
    trace_camera_path(
        &job,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut SequenceRng::new(&[]),
            environment: &mut SequenceRng::new(&[]),
            diffuse: &mut SequenceRng::new(&[0.9, 0.9]),
            medium: &mut medium,
        },
        Vector3::z(),
        0.0,
        Caustics::None,
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();

    assert_eq!(medium.draws, 3);
    assert_eq!(state.volume_scatters, 1);
    assert_eq!(state.delta_events(), 2);
    assert_eq!(state.medium_region, Some(MediumRegion::Exterior));
    // After the collision the deterministic optical depth is the remaining
    // entry gap, 2 m of interior absorption, and 2 m from glass to fog exit.
    let expected = (-(gap + 2.0 * 0.7 + 2.0)).exp();
    close(state.throughput, Vector3::repeat(expected));
}

#[test]
fn volume_scatter_near_prism_cap_sees_exit_and_opaque_shadow() {
    let mut job = crate::config::RenderJob::new();
    job.scene.environment = None;
    job.camera_options.max_volume_scatters = 1;
    let top = 1.0 + 5e-6;
    let prism = job.scene.add(
        Box::new(
            VerticalPrism::new(
                [
                    Vector2::new(-1.0, -1.0),
                    Vector2::new(1.0, -1.0),
                    Vector2::new(0.0, 1.0),
                ],
                1.0,
                top,
            )
            .unwrap(),
        ),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let medium = HomogeneousMedium::try_from(&HomogeneousMediumConfig {
        min_m: [-3.0; 3],
        max_m: [3.0; 3],
        sigma_a: 0.0.into(),
        sigma_s: 100_000.0.into(),
        g: (-0.5).into(),
    })
    .unwrap();
    job.scene
        .bind_medium_interface(
            prism,
            MediumInterface {
                interior: Some(Box::new(medium)),
            },
        )
        .unwrap();

    let origin = 1.0 + 2.5e-6;
    let ray = Ray::new(Vector3::z() * origin, Vector3::z()).unwrap();
    let mut state = CameraPathState::new(ray, 0);
    let gap = 0.5 * INTERSECTION_EPSILON;
    let depth = (top - origin - gap) * 100_000.0;
    trace_camera_path(
        &job,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut SequenceRng::new(&[]),
            environment: &mut SequenceRng::new(&[]),
            diffuse: &mut SequenceRng::new(&[0.9]),
            medium: &mut SequenceRng::new(&[-(-depth).exp_m1(), 0.0, 0.0]),
        },
        Vector3::z(),
        1.0,
        Caustics::None,
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();

    assert_eq!(state.volume_scatters, 1);
    assert_eq!(state.delta_events(), 1);
    assert_eq!(state.medium_region, Some(MediumRegion::Exterior));
    assert_eq!(state.radiance.direct, Vector3::zeros());
}

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
                medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
            medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
                    medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
        super::super::path::MAX_DELTA_EVENTS,
        super::super::path::MAX_DELTA_EVENTS + 1,
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
                Vector3::z() * (super::super::path::MAX_DELTA_EVENTS as f64 + 4.0),
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
                medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
            medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
        (false, super::super::path::MAX_DELTA_EVENTS),
        (false, super::super::path::MAX_DELTA_EVENTS + 1),
        (true, super::super::path::MAX_DELTA_EVENTS),
        (true, super::super::path::MAX_DELTA_EVENTS + 1),
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
            let x = i as f64 / (2.0 * super::super::path::MAX_DELTA_EVENTS as f64);
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
                medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
            medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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

mod environment_escape {
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

    fn trace(
        scene: &RenderJob,
        environment_words: &[f64],
        diffuse_words: &[f64],
    ) -> CameraPathState {
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
                medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
            let li = (SKY * (1.0 - direction.z) + Vector3::new(10.0, 6.0, 2.0) * direction.z)
                * intensity;
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
                    medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
                medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
        let hdr = super::super::support::TemporaryHdr::from_rgbe(
            "f2b-escape",
            4,
            2,
            &[
                [128, 32, 16, 132],
                [16, 128, 32, 132],
                [32, 16, 128, 132],
                [64, 64, 64, 132],
                [128, 32, 16, 132],
                [16, 128, 32, 132],
                [32, 16, 128, 132],
                [64, 64, 64, 132],
            ],
        );
        let path = &hdr.0;
        let map = EnvironmentMap::load(path).unwrap();
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
                            medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(
                                0,
                            ),
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
            let mut state =
                CameraPathState::new(Ray::new(Vector3::zeros(), Vector3::z()).unwrap(), 1);
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
                    medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
}

mod materials {
    use super::*;
    use crate::config::RenderJob;
    use crate::geometry::ground::Ground;
    use crate::integrator::camera::{
        path::trace_camera_path,
        transport::Caustics,
        types::{CameraPathRngs, CameraPathState},
    };
    use crate::material::BsdfSample;
    use crate::material::Material;
    use crate::material::ScatterType;
    use crate::material::dielectric::Dielectric;
    use crate::material::ggx_metal::GgxMetal;
    use crate::material::lambertian::Lambertian;
    use crate::material::mirror::Mirror;
    use rand::Rng;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn metal() -> GgxMetal {
        GgxMetal::new(
            Vector3::new(0.2, 0.8, 1.5),
            Vector3::new(3.0, 2.0, 1.0),
            0.5,
        )
        .unwrap()
    }
    fn constant_sky(scene: &mut RenderJob, color: Vector3<f64>) {
        let sky = scene.scene.environment.as_mut().unwrap();
        sky.set_gradient(color, color).unwrap();
    }
    #[test]
    fn glossy_escape_is_already_estimated_by_nee_and_is_not_added_again() {
        let sky = Vector3::new(1.0, 2.0, 3.0);
        let mut scene = RenderJob::new();
        constant_sky(&mut scene, sky);
        scene
            .scene
            .add(Box::new(Ground::new(0.0).unwrap()), Box::new(metal()));
        let mut reference = None;
        for depth in [0, 1, 4] {
            scene.camera_options.max_diffuse_bounces = depth;
            let mut state =
                CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), depth);
            let (mut camera, mut environment, mut continuation) = (
                SequenceRng::new(&[]),
                SequenceRng::new(&[0.0, 0.0]),
                SequenceRng::new(&[0.0, 0.0]),
            );
            trace_camera_path(
                &scene,
                &mut state,
                &mut CameraPathRngs {
                    camera: &mut camera,
                    environment: &mut environment,
                    diffuse: &mut continuation,
                    medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
                },
                sun().0,
                0.0,
                (None).map_or(Caustics::None, Caustics::Rgb),
                crate::optics::dispersion::IorMode::Constant,
                #[cfg(feature = "diagnostics")]
                None,
            )
            .unwrap();
            if let Some(expected) = reference {
                close(state.radiance.direct, expected);
            } else {
                reference = Some(state.radiance.direct);
            }
            assert!(state.radiance.direct.norm() > 0.0);
            assert_eq!(continuation.draws, if depth == 0 { 0 } else { 2 });
            if depth > 0 {
                assert_eq!(state.last_scatter, Some(ScatterType::GlossyReflection));
            }
        }
    }
    #[test]
    fn actual_ggx_mirror_dielectric_chain_shades_a_lambertian_terminal_vertex() {
        let mut scene = RenderJob::new();
        scene.scene.environment = None;
        let m = metal();
        let wo = Vector3::z();
        let f = m
            .sample(wo, wo, &mut SequenceRng::new(&[0.0, 0.0]))
            .unwrap()
            .weight;
        let r = Vector3::new(0.9, 0.6, 0.3);
        let albedo = Vector3::new(0.2, 0.5, 0.8);
        let (light_wi, light) = sun();
        let first_bsdf = m.evaluate_rgb(light_wi, wo, wo);
        let first_direct = first_bsdf * (light * light_wi.z);
        scene
            .scene
            .add(Box::new(Ground::new(0.0).unwrap()), Box::new(m));
        let center = Vector3::z();
        let v = (Vector3::x() + Vector3::z()).normalize() * 0.2;
        scene.scene.add(
            Box::new(
                Triangle::new(
                    center - Vector3::y() * 0.2 - v,
                    center + Vector3::y() * 0.2 - v,
                    center + v,
                )
                .unwrap(),
            ),
            Box::new(Mirror::new(r).unwrap()),
        );
        for (x, outward) in [(0.3, false), (0.4, true)] {
            scene.scene.add(
                Box::new(glass_patch(x, 1.0, outward)),
                Box::new(Dielectric::new(1.5).unwrap()),
            );
        }
        scene.scene.add(
            Box::new(glass_patch(1.0, 1.0, false)),
            Box::new(Lambertian::new(albedo).unwrap()),
        );
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 1);
        let (mut camera, mut environment, mut continuation) = (
            SequenceRng::new(&[]),
            SequenceRng::new(&[]),
            SequenceRng::new(&[0.0, 0.0, 0.99, 0.99]),
        );
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut camera,
                environment: &mut environment,
                diffuse: &mut continuation,
                medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
            },
            light_wi,
            light,
            (None).map_or(Caustics::None, Caustics::Rgb),
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        assert_eq!(
            (
                state.path_depth,
                state.non_delta_bounces,
                state.delta_events()
            ),
            (4, 1, 3)
        );
        close(state.throughput, f.component_mul(&r)); // two eta² factors cancel.
        let terminal = albedo * (light * (-light_wi.x) / std::f64::consts::PI);
        close(
            state.radiance.direct,
            first_direct + f.component_mul(&r).component_mul(&terminal),
        );
        close(state.radiance.caustics, Vector3::zeros());
        assert_eq!(
            (camera.draws, environment.draws, continuation.draws),
            (0, 0, 4)
        );
    }
    #[test]
    fn repeated_ggx_bounces_use_rr_once_and_compensate_rgb() {
        let mut scene = RenderJob::new();
        scene.scene.environment = None;
        scene.camera_options.max_diffuse_bounces = 4;
        let m = metal();
        let f = m
            .sample(
                Vector3::z(),
                Vector3::z(),
                &mut SequenceRng::new(&[0.0, 0.0]),
            )
            .unwrap()
            .weight;
        scene
            .scene
            .add(Box::new(Ground::new(0.0).unwrap()), Box::new(m.clone()));
        scene.scene.add(
            Box::new(
                Triangle::new(
                    Vector3::new(-10.0, -10.0, 1.0),
                    Vector3::new(0.0, 10.0, 1.0),
                    Vector3::new(10.0, -10.0, 1.0),
                )
                .unwrap(),
            ),
            Box::new(m),
        );
        for survive in [false, true] {
            let mut words = vec![0.0; 6];
            words.push(if survive { 0.0 } else { 0.999 });
            if survive {
                words.extend([0.0, 0.0]);
            }
            let (mut camera, mut environment, mut continuation) = (
                SequenceRng::new(&[]),
                SequenceRng::new(&[]),
                SequenceRng::new(&words),
            );
            let mut state =
                CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 4);
            trace_camera_path(
                &scene,
                &mut state,
                &mut CameraPathRngs {
                    camera: &mut camera,
                    environment: &mut environment,
                    diffuse: &mut continuation,
                    medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
                },
                sun().0,
                0.0,
                (None).map_or(Caustics::None, Caustics::Rgb),
                crate::optics::dispersion::IorMode::Constant,
                #[cfg(feature = "diagnostics")]
                None,
            )
            .unwrap();
            let p = f.map(|v| v.powi(3)).max().clamp(0.05, 0.95);
            close(
                state.throughput,
                if survive {
                    f.map(|v| v.powi(4)) / p
                } else {
                    f.map(|v| v.powi(3))
                },
            );
            assert_eq!(state.path_depth, if survive { 4 } else { 3 });
            assert_eq!(state.last_scatter, Some(ScatterType::GlossyReflection));
            assert_eq!(continuation.draws, if survive { 9 } else { 7 });
        }
    }

    /// Test-only 50/50 continuous + delta material: full selection probability in pdf,
    /// both branch weights already compensate the branch probability.
    struct Mixed {
        diffuse: Lambertian,
        mirror: Mirror,
    }
    impl Mixed {
        fn new() -> Self {
            Self {
                diffuse: Lambertian::new(Vector3::new(0.2, 0.5, 0.8)).unwrap(),
                mirror: Mirror::new(Vector3::new(0.9, 0.6, 0.3)).unwrap(),
            }
        }
    }
    impl Material for Mixed {
        fn evaluate_rgb(
            &self,
            wi: Vector3<f64>,
            wo: Vector3<f64>,
            n: Vector3<f64>,
        ) -> Vector3<f64> {
            self.diffuse.evaluate_rgb(wi, wo, n) * 0.5
        }
        fn pdf(&self, wi: Vector3<f64>, wo: Vector3<f64>, n: Vector3<f64>) -> f64 {
            0.5 * self.diffuse.pdf(wi, wo, n)
        }
        fn has_delta(&self) -> bool {
            true
        }
        fn sample(
            &self,
            wo: Vector3<f64>,
            n: Vector3<f64>,
            rng: &mut dyn rand::RngCore,
        ) -> Option<BsdfSample> {
            if rng.random::<f64>() < 0.5 {
                self.mirror.sample(wo, n, rng)
            } else {
                let mut sample = self.diffuse.sample(wo, n, rng)?;
                sample.pdf = sample.pdf.map(|p| p * 0.5);
                Some(sample)
            }
        }
    }
    #[test]
    fn mixed_delta_nee_quota_is_null_and_continuation_adds_delta_escape_once() {
        let sky = Vector3::new(1.0, 2.0, 3.0);
        let mut scene = RenderJob::new();
        constant_sky(&mut scene, sky);
        scene
            .scene
            .add(Box::new(Ground::new(0.0).unwrap()), Box::new(Mixed::new()));
        for choose_delta in [true, false] {
            let mut state =
                CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 0);
            let words = if choose_delta {
                vec![0.0]
            } else {
                vec![0.9, 0.25, 0.25]
            };
            let (mut camera, mut environment, mut continuation) = (
                SequenceRng::new(&[]),
                SequenceRng::new(&[0.0]),
                SequenceRng::new(&words),
            );
            trace_camera_path(
                &scene,
                &mut state,
                &mut CameraPathRngs {
                    camera: &mut camera,
                    environment: &mut environment,
                    diffuse: &mut continuation,
                    medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
                },
                sun().0,
                0.0,
                (None).map_or(Caustics::None, Caustics::Rgb),
                crate::optics::dispersion::IorMode::Constant,
                #[cfg(feature = "diagnostics")]
                None,
            )
            .unwrap();
            assert_eq!(environment.draws, 1); // no replacement for the delta NEE sample.
            assert_eq!(continuation.draws, words.len()); // no replacement for exhausted continuous budget.
            close(
                state.radiance.direct,
                if choose_delta {
                    Vector3::new(0.9, 0.6, 0.3).component_mul(&sky)
                } else {
                    Vector3::zeros()
                },
            );
        }
    }
    #[test]
    fn mixed_event_mean_matches_half_diffuse_plus_half_mirror_without_double_environment() {
        let sky = Vector3::new(1.0, 2.0, 3.0);
        let mut scene = RenderJob::new();
        scene.camera_options.max_diffuse_bounces = 1;
        constant_sky(&mut scene, sky);
        scene
            .scene
            .add(Box::new(Ground::new(0.0).unwrap()), Box::new(Mixed::new()));
        let (mut camera, mut environment, mut continuation) = (
            StdRng::seed_from_u64(1),
            StdRng::seed_from_u64(2),
            StdRng::seed_from_u64(3),
        );
        let mut sum = Vector3::zeros();
        for _ in 0..16384 {
            let mut state =
                CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 1);
            trace_camera_path(
                &scene,
                &mut state,
                &mut CameraPathRngs {
                    camera: &mut camera,
                    environment: &mut environment,
                    diffuse: &mut continuation,
                    medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
                },
                sun().0,
                0.0,
                (None).map_or(Caustics::None, Caustics::Rgb),
                crate::optics::dispersion::IorMode::Constant,
                #[cfg(feature = "diagnostics")]
                None,
            )
            .unwrap();
            sum += state.radiance.direct;
        }
        let expected =
            (Vector3::new(0.2, 0.5, 0.8) + Vector3::new(0.9, 0.6, 0.3)).component_mul(&sky) * 0.5;
        assert!((sum / 16384.0 - expected).norm() < 0.03);
    }
}

#[test]
fn path_state_validate_rejects_non_unit_or_overflowing_ray_direction() {
    let mut state = CameraPathState::new(Ray::new(Vector3::z(), -Vector3::z()).unwrap(), 0);
    assert!(state.validate().is_ok());
    for bad in [
        Vector3::new(2.0, 0.0, 0.0),
        Vector3::new(0.5, 0.0, 0.0),
        Vector3::repeat(1e200),
    ] {
        state.ray.direction = bad;
        assert!(state.validate().is_err());
    }
}

#[test]
fn camera_path_detects_medium_throughput_overflow_and_short_circuits_zero_weight() {
    use crate::config::RenderJob;
    use crate::medium::{Medium, MediumCoefficients, MediumSample, phase::HenyeyGreenstein};
    use rand::{SeedableRng, rngs::StdRng};
    struct ExtremeMedium(f64);
    impl Medium for ExtremeMedium {
        fn density_at(&self, _: Vector3<f64>) -> f64 {
            1.0
        }
        fn coefficients_at(&self, _: Vector3<f64>, _: f64) -> MediumCoefficients {
            MediumCoefficients {
                sigma_a: 0.0,
                sigma_s: 1.0,
            }
        }
        fn sigma_t_majorant(&self, _: f64) -> f64 {
            1.0
        }
        fn phase(&self, _: f64) -> HenyeyGreenstein {
            HenyeyGreenstein::new(0.0).unwrap()
        }
        fn transmittance_ray(&self, _: &Ray, _: f64, _: f64) -> f64 {
            1.0
        }
        fn sample_segment(
            &self,
            ray: &Ray,
            _: f64,
            _: f64,
            allow_scattering: bool,
            _: &mut dyn rand::RngCore,
        ) -> Result<MediumSample, String> {
            Ok(MediumSample {
                position: allow_scattering.then(|| ray.at(0.5)),
                weight: self.0,
            })
        }
    }

    // Overflow when multiplying throughput * sample.weight
    let mut scene = RenderJob::new();
    scene.scene.environment = None;
    scene.scene.medium = Some(Box::new(ExtremeMedium(1e200)));
    let mut state = CameraPathState::new(Ray::new(Vector3::z(), -Vector3::z()).unwrap(), 0);
    state.throughput = Vector3::repeat(1e200);
    let mut rng = StdRng::seed_from_u64(1);
    let mut rng2 = StdRng::seed_from_u64(2);
    let mut rng3 = StdRng::seed_from_u64(3);
    let mut rng4 = StdRng::seed_from_u64(4);
    assert!(
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut rng,
                environment: &mut rng2,
                diffuse: &mut rng3,
                medium: &mut rng4,
            },
            sun().0,
            sun().1,
            Caustics::None,
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .is_err()
    );

    // Zero weight with Some(position) short-circuits without consuming phase/volume work
    scene.scene.medium = Some(Box::new(ExtremeMedium(0.0)));
    let mut state = CameraPathState::new(Ray::new(Vector3::z(), -Vector3::z()).unwrap(), 0);
    let mut medium_rng = SequenceRng::new(&[]);
    trace_camera_path(
        &scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut rng,
            environment: &mut rng2,
            diffuse: &mut rng3,
            medium: &mut medium_rng,
        },
        sun().0,
        sun().1,
        Caustics::None,
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();
    assert_eq!(state.volume_scatters, 0);
    assert_eq!(medium_rng.draws, 0);
}

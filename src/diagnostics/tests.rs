use super::*;
use crate::config::RenderJob;
use crate::geometry::ground::Ground;
use crate::geometry::prism::Prism;
use crate::geometry::ray::Ray;
use crate::geometry::sphere::Sphere;
use crate::integrator::camera::path::MAX_DELTA_EVENTS;
use crate::integrator::camera::tests::support::SequenceRng;
use crate::integrator::camera::tests::support::sun;
use crate::integrator::camera::tests::support::two_planes;
use crate::integrator::camera::{
    path::trace_camera_path,
    transport::Caustics,
    types::{CameraPathRngs, CameraPathState},
};
use crate::material::dielectric::Dielectric;
use crate::material::ggx_metal::GgxMetal;
use crate::material::lambertian::Lambertian;
use crate::material::mirror::Mirror;

fn surface(material: Box<dyn Material>) -> RenderJob {
    let mut scene = RenderJob::new();
    scene.scene.environment = None;
    scene.ground_checkerboard = None;
    scene.camera_options.max_diffuse_bounces = 1;
    scene.diagnostic_target = Some(
        scene
            .scene
            .add(Box::new(Ground::new(0.0).unwrap()), material),
    );
    scene
}

fn trace(
    scene: &RenderJob,
    ray: Ray,
    camera: &[f64],
    environment: &[f64],
    diffuse: &[f64],
) -> (PathDebugSample, CameraPathState, [usize; 3]) {
    let mut camera = SequenceRng::new(camera);
    let mut environment = SequenceRng::new(environment);
    let mut diffuse = SequenceRng::new(diffuse);
    let mut state = CameraPathState::new(ray, scene.camera_options.max_diffuse_bounces);
    let mut debug = PathDebugSample::default();
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
        sun().1,
        (None).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        Some(&mut debug),
    )
    .unwrap();
    (
        debug,
        state,
        [camera.draws, environment.draws, diffuse.draws],
    )
}

fn downward() -> Ray {
    Ray::new(Vector3::z(), -Vector3::z()).unwrap()
}

#[test]
fn mirror_reflection_records_first_geometry_and_has_no_tir_or_extra_rng() {
    let mut scene = surface(Box::new(Mirror::new(Vector3::repeat(0.95)).unwrap()));
    scene.scene.environment = RenderJob::new().scene.environment;
    let (debug, state, draws) = trace(&scene, downward(), &[], &[], &[]);
    assert_eq!(debug.first_hit_object_id, scene.diagnostic_target);
    assert_eq!(debug.first_hit_normal, Some([0.0, 0.0, 1.0]));
    assert_eq!(debug.events.specular_reflection, 1);
    assert_eq!(debug.events.specular_transmission, 0);
    assert!(!debug.total_internal_reflection);
    assert!(debug.escaped_environment);
    assert_eq!(debug.termination, Some(StopReason::EnvironmentEscape));
    assert_eq!(draws, [0; 3]);
    assert_eq!(state.throughput, Vector3::repeat(0.95));
}

#[test]
fn dielectric_transmission_and_fresnel_reflection_are_not_tir() {
    let scene = surface(Box::new(Dielectric::new(1.52).unwrap()));
    for (ray, value, transmission) in [
        (downward(), 0.99, true),
        (downward(), 0.0, false),
        (Ray::new(-Vector3::z(), Vector3::z()).unwrap(), 0.0, false),
        (
            Ray::new(Vector3::z(), Vector3::new(0.9, 0.0, -0.19_f64.sqrt())).unwrap(),
            0.0,
            false,
        ),
    ] {
        let (debug, _, draws) = trace(&scene, ray, &[value], &[], &[]);
        assert_eq!(debug.events.specular_transmission, u64::from(transmission));
        assert_eq!(debug.events.specular_reflection, u64::from(!transmission));
        assert!(!debug.total_internal_reflection);
        assert_eq!(draws, [1, 0, 0]);
    }
}

#[test]
fn dielectric_internal_supercritical_ray_records_real_tir_without_rng() {
    let scene = surface(Box::new(Dielectric::new(1.52).unwrap()));
    let ray = Ray::new(-Vector3::z(), Vector3::new(0.9, 0.0, 0.19_f64.sqrt())).unwrap();
    let (debug, state, draws) = trace(&scene, ray, &[], &[], &[]);
    assert_eq!(debug.events.specular_reflection, 1);
    assert_eq!(debug.events.specular_transmission, 0);
    assert!(debug.total_internal_reflection);
    assert!(state.ray.direction.z < 0.0);
    assert_eq!(draws, [0; 3]);
    assert_eq!(debug.termination, Some(StopReason::BackgroundEscape));
}

fn glass_sphere() -> RenderJob {
    let mut scene = RenderJob::new();
    scene.ground_checkerboard = None;
    scene.diagnostic_target = Some(scene.scene.add(
        Box::new(Sphere::new(Vector3::zeros(), 1.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    ));
    assert_eq!(scene.photon.caustics_object_id, None);
    scene
}

#[test]
fn sphere_camera_double_transmission_records_outward_entry_and_environment_once() {
    let scene = glass_sphere();
    let ray = Ray::new(Vector3::new(-3.0, 0.6, 0.0), Vector3::x()).unwrap();
    let (debug, state, draws) = trace(&scene, ray, &[0.99, 0.99], &[], &[]);
    let angle = 2.0 * ((0.6_f64 / 1.5).asin() - 0.6_f64.asin());
    let expected_direction = Vector3::new(angle.cos(), angle.sin(), 0.0);
    // 真实路径使用现有法线偏移，曲面上的第二交点因此存在 O(epsilon) 偏差。
    assert!((state.ray.direction - expected_direction).norm() < 1e-5);
    assert!((state.throughput - Vector3::repeat(1.0)).norm() < 1e-12);
    let background = scene
        .scene
        .background_radiance(state.ray.direction)
        .unwrap();
    assert!((state.radiance.direct - background).norm() < 1e-12);
    assert_eq!(state.radiance.caustics, Vector3::zeros());
    assert_eq!(debug.events.specular_transmission, 2);
    assert_eq!(debug.events.specular_reflection, 0);
    assert!(!debug.total_internal_reflection);
    assert_eq!(debug.termination, Some(StopReason::EnvironmentEscape));
    assert_eq!(draws, [2, 0, 0]);
    let normal = Vector3::from(debug.first_hit_normal.unwrap());
    assert!((normal - Vector3::new(-0.8, 0.6, 0.0)).norm() < 1e-12);
    let mut acc = PathDebugAccumulator::default();
    acc.record(&debug, scene.diagnostic_target).unwrap();
    let pixel = acc.finish();
    assert_eq!(pixel.target_samples, 1);
    assert_eq!(pixel.escaped_environment, 1);
    for (actual, expected) in aov_pixel(&pixel, 3).into_iter().zip([26, 204, 128]) {
        // -0.8 对应 25.5 的量化边界，允许浮点舍入造成一个码值差。
        assert!(actual.abs_diff(expected) <= 1);
    }
    // 同一真实路径对其他物体过滤时，不得冒用 prism_id 或纳入事件。
    let mut masked_acc = PathDebugAccumulator::default();
    masked_acc.record(&debug, Some(1)).unwrap();
    let masked = masked_acc.finish();
    assert_eq!(masked.target_samples, 0);
    assert_eq!(aov_pixel(&masked, 0), [0; 3]);
}

#[test]
fn sphere_internal_tir_repeats_until_delta_budget_without_rng_or_escape() {
    let scene = glass_sphere();
    // 光线到球心的垂距 b=0.9 > 1/1.5；封闭球内每次碰撞都超过临界角。
    let ray = Ray::new(Vector3::new(0.0, 0.9, 0.0), Vector3::x()).unwrap();
    let (debug, state, draws) = trace(&scene, ray, &[], &[], &[]);
    assert_eq!(debug.events.specular_reflection, MAX_DELTA_EVENTS as u64);
    assert_eq!(debug.events.specular_transmission, 0);
    assert!(debug.total_internal_reflection);
    assert!(!debug.escaped_environment);
    assert_eq!(debug.termination, Some(StopReason::DeltaBudgetExhausted));
    assert_eq!(draws, [0; 3]);
    assert_eq!(state.throughput, Vector3::repeat(1.0));
    assert_eq!(state.radiance.direct, Vector3::zeros());
    assert!(state.ray.origin.norm() < 1.0);
    let mut acc = PathDebugAccumulator::default();
    acc.record(&debug, scene.diagnostic_target).unwrap();
    let pixel = acc.finish();
    assert_eq!(aov_pixel(&pixel, 0), [0, 0, 255]);
    assert_eq!(
        pixel.terminations.values().sum::<u64>(),
        pixel.target_samples
    );
}

#[test]
fn ggx_records_glossy_or_delta_and_preserves_rejected_ndf_samples() {
    for (roughness, words, event, rejected) in [
        (0.0, vec![], ScatterType::SpecularReflection, false),
        (0.4, vec![0.0, 0.0], ScatterType::GlossyReflection, false),
        (0.75, vec![0.9999, 0.0], ScatterType::GlossyReflection, true),
    ] {
        let scene = surface(Box::new(
            GgxMetal::new(Vector3::repeat(0.2), Vector3::repeat(3.0), roughness).unwrap(),
        ));
        let (debug, _, draws) = trace(&scene, downward(), &[], &[], &words);
        assert_eq!(
            debug.events.glossy_reflection,
            u64::from(!rejected && event == ScatterType::GlossyReflection)
        );
        assert_eq!(
            debug.events.specular_reflection,
            u64::from(event == ScatterType::SpecularReflection)
        );
        assert!(!debug.total_internal_reflection);
        assert_eq!(draws, [0, 0, words.len()]);
        assert_eq!(
            debug.termination,
            Some(if rejected {
                StopReason::SampleRejected
            } else {
                StopReason::BackgroundEscape
            })
        );
    }
}

#[test]
fn ground_visit_and_geometric_environment_escape_are_independent_of_radiance_termination() {
    let mut scene = surface(Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()));
    scene.photon.ground_id = scene.diagnostic_target;
    scene.scene.environment = RenderJob::new().scene.environment;
    scene.camera_options.environment_samples = 1;
    let (debug, state, draws) = trace(&scene, downward(), &[], &[0.25, 0.0], &[0.0, 0.0]);
    assert!(debug.reached_ground);
    assert!(debug.escaped_environment); // Non-Delta miss: geometric escape, no extra radiance term.
    assert_eq!(debug.events.diffuse_reflection, 1);
    assert_eq!(state.non_delta_bounces, 1);
    assert_eq!(debug.termination, Some(StopReason::EnvironmentEscape));
    assert_eq!(draws, [0, 2, 2]);
    scene.camera_options.max_diffuse_bounces = 0;
    let (terminal, _, _) = trace(&scene, downward(), &[], &[0.25, 0.0], &[]);
    assert!(terminal.reached_ground);
    assert!(!terminal.escaped_environment);
    assert_eq!(
        terminal.termination,
        Some(StopReason::NonDeltaBudgetExhausted)
    );
}

#[test]
fn rr_budget_backface_missing_material_and_zero_weight_have_distinct_reasons() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    scene.camera_options.max_diffuse_bounces = 5;
    scene.photon.ground_id = Some(0);
    let (rr, _, draws) = trace(
        &scene,
        Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(),
        &[],
        &[],
        &[0.25, 0.0, 0.75, 0.25, 0.25, 0.0, 0.99],
    );
    assert!(rr.reached_ground);
    assert_eq!(rr.events.diffuse_reflection, 3);
    assert_eq!(rr.termination, Some(StopReason::RussianRoulette));
    assert_eq!(draws, [0, 0, 7]);

    let scene = surface(Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()));
    let (backface, _, _) = trace(
        &scene,
        Ray::new(-Vector3::z(), Vector3::z()).unwrap(),
        &[],
        &[],
        &[],
    );
    assert_eq!(backface.termination, Some(StopReason::Backface));
    let scene = surface(Box::new(Mirror::new(Vector3::zeros()).unwrap()));
    let (zero, _, _) = trace(&scene, downward(), &[], &[], &[]);
    assert_eq!(zero.termination, Some(StopReason::ZeroThroughput));
    assert_eq!(zero.events.specular_reflection, 1);

    let mut scene = RenderJob::new();
    scene.scene.environment = None;
    for height in 1..=MAX_DELTA_EVENTS + 1 {
        scene.scene.add(
            Box::new(Ground::new(height as f64).unwrap()),
            Box::new(Dielectric::new(1.0).unwrap()),
        );
    }
    let (budget, _, draws) = trace(
        &scene,
        Ray::new(
            Vector3::z() * (MAX_DELTA_EVENTS as f64 + 2.0),
            -Vector3::z(),
        )
        .unwrap(),
        &[],
        &[],
        &[],
    );
    assert_eq!(budget.events.specular_transmission, MAX_DELTA_EVENTS as u64);
    assert_eq!(budget.termination, Some(StopReason::DeltaBudgetExhausted));
    assert_eq!(draws, [0; 3]);
}

#[test]
fn coplanar_prism_triangles_produce_identical_first_hit_geometric_normals() {
    let mut scene = RenderJob::new();
    scene.diagnostic_target = Some(scene.scene.add(
        Box::new(Prism::new().unwrap()),
        Box::new(Mirror::new(Vector3::repeat(0.95)).unwrap()),
    ));
    for (points, normal) in [
        (
            [Vector3::new(-0.2, 0.6, 1.0), Vector3::new(0.2, -0.6, 1.0)],
            -Vector3::z(),
        ),
        (
            [
                Vector3::new(0.375, 0.6, 1.2),
                Vector3::new(0.125, -0.6, 1.6),
            ],
            Vector3::new(1.6, 0.0, 1.0).normalize(),
        ),
        (
            [
                Vector3::new(-0.125, 0.6, 1.6),
                Vector3::new(-0.375, -0.6, 1.2),
            ],
            Vector3::new(-1.6, 0.0, 1.0).normalize(),
        ),
    ] {
        let mut normals = Vec::new();
        for point in points {
            let (debug, _, _) = trace(
                &scene,
                Ray::new(point + normal * 0.1, -normal).unwrap(),
                &[],
                &[],
                &[],
            );
            let actual = Vector3::from(debug.first_hit_normal.unwrap());
            assert!((actual - normal).norm() < 1e-14);
            assert_eq!(debug.first_hit_object_id, scene.diagnostic_target);
            normals.push(actual);
        }
        assert!((normals[0] - normals[1]).norm() < 1e-14);
    }
}

#[test]
fn mixed_both_budgets_and_absent_components_are_not_reported_as_sample_rejection() {
    struct Capabilities(bool);
    impl Material for Capabilities {
        fn evaluate_rgb(&self, _: Vector3<f64>, _: Vector3<f64>, _: Vector3<f64>) -> Vector3<f64> {
            Vector3::zeros()
        }
        fn has_non_delta(&self) -> bool {
            self.0
        }
        fn has_delta(&self) -> bool {
            self.0
        }
    }
    for mixed in [false, true] {
        let mut scene = surface(Box::new(Capabilities(mixed)));
        scene.camera_options.max_diffuse_bounces = 0;
        let mut state = CameraPathState::new(downward(), 0);
        state.path_depth = MAX_DELTA_EVENTS;
        let mut debug = PathDebugSample::default();
        let mut camera = SequenceRng::new(&[]);
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
            Some(&mut debug),
        )
        .unwrap();
        assert_eq!(
            debug.termination,
            Some(if mixed {
                StopReason::AllBudgetsExhausted
            } else {
                StopReason::NoScatteringComponents
            })
        );
        assert_eq!(debug.events, PathEventCounts::default());
        assert_eq!([camera.draws, environment.draws, diffuse.draws], [0; 3]);
    }
}

#[test]
fn pixel_statistics_average_all_spp_and_mask_each_non_target_subsample() {
    let mut acc = PathDebugAccumulator::default();
    let a = PathDebugSample {
        first_hit_object_id: Some(2),
        first_hit_normal: Some([1.0, 0.0, 0.0]),
        events: PathEventCounts {
            specular_reflection: 1,
            ..Default::default()
        },
        reached_ground: true,
        escaped_environment: true,
        termination: Some(StopReason::EnvironmentEscape),
        ..Default::default()
    };
    let b = PathDebugSample {
        first_hit_object_id: Some(2),
        first_hit_normal: Some([0.0, 1.0, 0.0]),
        events: PathEventCounts {
            glossy_reflection: 3,
            specular_transmission: 2,
            ..Default::default()
        },
        total_internal_reflection: true,
        termination: Some(StopReason::RussianRoulette),
        ..Default::default()
    };
    let other = PathDebugSample {
        first_hit_object_id: Some(3),
        reached_ground: true,
        ..Default::default()
    };
    for sample in [a, b, other, PathDebugSample::default()] {
        acc.record(&sample, Some(2)).unwrap();
    }
    let pixel = acc.finish();
    assert_eq!(pixel.samples, 4);
    assert_eq!(pixel.target_samples, 2);
    assert_eq!(pixel.first_hit_objects, BTreeMap::from([(2, 2), (3, 1)]));
    assert_eq!(pixel.terminations.values().sum::<u64>(), 2);
    assert_eq!(aov_pixel(&pixel, 0), [64, 64, 64]);
    assert_eq!(aov_pixel(&pixel, 1), [count_byte(1.0); 3]);
    assert_eq!(aov_pixel(&pixel, 2), [count_byte(0.5); 3]);
    assert_eq!(aov_pixel(&pixel, 3), [96, 96, 64]);
    assert_eq!(aov_pixel(&PathDebugPixel::default(), 0), [0; 3]);
    assert_eq!(count_byte(0.0), 0);
    assert_eq!(count_byte(1.0), 51);
    assert_eq!(count_byte(32.0), 255);
    assert_eq!(count_byte(64.0), 255);
    let mut overflow = PathDebugAccumulator {
        samples: u64::MAX,
        ..Default::default()
    };
    assert!(overflow.record(&PathDebugSample::default(), None).is_err());
}

#[test]
fn dispersive_spectral_internal_tir_uses_wavelength_ior() {
    use crate::optics::dispersion::{Cauchy, IorMode};
    // Constant ior = 1.52 (sin_c ≈ 0.6579); Cauchy at 400 nm has n ≈ 1.546 (sin_c ≈ 0.6467),
    // at 700 nm has n ≈ 1.515 (sin_c ≈ 0.6602). Choose sin_i = 0.652 between blue and constant/red.
    let cauchy = Cauchy {
        a: 1.5046,
        b_um2: 0.0067,
    };
    let glass = Dielectric::with_dispersion(1.52, Some(cauchy)).unwrap();
    let n_blue = cauchy.ior(400.0).unwrap();
    let n_red = cauchy.ior(700.0).unwrap();
    let sin_i = 0.5 * (1.0 / n_blue + 1.0 / 1.52);
    assert!(sin_i > 1.0 / n_blue && sin_i < 1.0 / 1.52 && sin_i < 1.0 / n_red);
    let cos_i = (1.0 - sin_i * sin_i).sqrt();
    let scene = surface(Box::new(glass));
    let ray = Ray::new(-Vector3::z(), Vector3::new(sin_i, 0.0, cos_i)).unwrap();

    for (nm, mode, expect_tir, rng_values) in [
        (400.0, IorMode::Dispersion, true, &[][..]),
        (700.0, IorMode::Dispersion, false, &[0.0][..]),
        (400.0, IorMode::Constant, false, &[0.0][..]),
    ] {
        let mut state = CameraPathState::spectral(ray, nm, 0).unwrap();
        let mut camera = SequenceRng::new(rng_values);
        let mut environment = SequenceRng::new(&[]);
        let mut diffuse = SequenceRng::new(&[]);
        let mut debug = PathDebugSample::default();
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
            Caustics::None,
            mode,
            Some(&mut debug),
        )
        .unwrap();
        assert_eq!(debug.events.specular_reflection, 1);
        assert_eq!(
            debug.total_internal_reflection, expect_tir,
            "nm={nm}, mode={mode:?}"
        );
    }
}

use super::super::path::*;
use super::super::*;
use crate::config::RenderJob;
use crate::geometry::ground::Ground;
use crate::geometry::hit::GeometryHit;
use crate::geometry::hit::Hit;
use crate::geometry::object::Hittable;
use crate::geometry::ray::Ray;
use crate::geometry::sphere::Sphere;
use crate::material::dielectric::Dielectric;
use crate::medium::{
    GridMedium, GridMediumConfig, HomogeneousMedium, HomogeneousMediumConfig, Medium,
};
use crate::optics::dispersion::IorMode;
use crate::optics::interface::{fresnel_dielectric, refract};
use crate::scene::{MediumInterface, MediumRegion};
use nalgebra::Vector3;
use rand::RngCore;
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;

struct Sequence {
    values: Vec<u64>,
    draws: usize,
}

impl Sequence {
    fn new(values: &[u64]) -> Self {
        Self {
            values: values.to_vec(),
            draws: 0,
        }
    }
}

impl RngCore for Sequence {
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
    fn next_u64(&mut self) -> u64 {
        let value = self.values[self.draws];
        self.draws += 1;
        value
    }
    fn fill_bytes(&mut self, _: &mut [u8]) {
        panic!("仅应采样 f64")
    }
}

struct Slab;
impl Hittable for Slab {
    fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
        let top = Ground::new(2.0)?.intersect(ray);
        let bottom = Ground::new(1.0)?.intersect(ray).map(|mut h| {
            h.normal = -h.normal;
            h
        });
        top.into_iter()
            .chain(bottom)
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}

fn slab(ior: f64, max_events: usize) -> (RenderJob, PhotonTracer, Ray) {
    let mut scene = RenderJob::new();
    let object_id = scene
        .scene
        .add(Box::new(Slab), Box::new(Dielectric::new(ior).unwrap()));
    let ground_id = scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    scene.photon.ground_id = Some(ground_id);
    scene.select_caustics_object(object_id).unwrap();
    let ray = Ray::new(Vector3::new(0.0, 0.0, 3.0), -Vector3::z()).unwrap();
    (
        scene,
        PhotonTracer {
            object_id,
            ground_id,
            glass_ior: ior,
            max_events,
        },
        ray,
    )
}

#[test]
fn refracted_slab_accumulates_clipped_air_and_glass_segments() {
    let ior = 1.5;
    let (mut scene, tracer, _) = slab(ior, 4);
    let theta = 30.0_f64.to_radians();
    let glass_theta = (theta.sin() / ior).asin();
    let ray = Ray::new(
        Vector3::z() * 3.0,
        Vector3::new(theta.sin(), 0.0, -theta.cos()),
    )
    .unwrap();
    let min_m = [-10.0, -10.0, 0.5];
    let max_m = [10.0, 10.0, 2.5];
    let homogeneous = |sigma_a: f64, sigma_s: f64| -> Box<dyn Medium> {
        Box::new(
            HomogeneousMedium::try_from(&HomogeneousMediumConfig {
                min_m,
                max_m,
                sigma_a: sigma_a.into(),
                sigma_s: sigma_s.into(),
                g: 0.7.into(),
            })
            .unwrap(),
        )
    };
    let grid = GridMedium::try_from(&GridMediumConfig {
        min_m,
        max_m,
        resolution: [1, 1, 2],
        densities: vec![1.0, 3.0],
        sigma_a_base: 0.2.into(),
        sigma_s_base: 0.3.into(),
        g: 0.7.into(),
    })
    .unwrap();
    // Half a metre per outside leg and three crossings of the full glass slab.
    let optical_depth = 0.5 * (1.0 / theta.cos() + 3.0 / glass_theta.cos());
    for (medium, expected_tr) in [
        (None, 1.0),
        (Some(homogeneous(0.0, 0.0)), 1.0),
        (Some(homogeneous(0.2, 0.3)), (-optical_depth).exp()),
        (Some(Box::new(grid)), (-2.0 * optical_depth).exp()),
        (Some(homogeneous(1400.0, 0.0)), 0.0),
    ] {
        scene.scene.medium = medium;
        // Force entry, two internal reflections, and exit using the original Fresnel draws.
        let mut rng = Sequence::new(&[u64::MAX, 0, 0, u64::MAX]);
        let result = tracer
            .trace(&scene, ray, scene.scene.intersect(&ray), 550.0, &mut rng)
            .unwrap();
        let landing = result.landing().unwrap();
        let expected_x = 2.0 * theta.tan() + 3.0 * glass_theta.tan();
        assert!((landing.position - Vector3::x() * expected_x).norm() < 1e-14);
        if expected_tr == 0.0 || expected_tr == 1.0 {
            assert_eq!(landing.weight.to_bits(), expected_tr.to_bits());
        } else {
            assert!((landing.weight / expected_tr - 1.0).abs() < 1e-14);
        }
        assert_eq!(
            (result.state.transmissions, result.state.reflections),
            (2, 2)
        );
        assert_eq!(result.state.medium, PhotonMedium::Air);
        assert_eq!(rng.draws, 4);
    }
}

#[test]
fn photon_segments_reject_invalid_distance_and_transmittance() {
    use crate::medium::{MediumCoefficients, MediumSample, phase::HenyeyGreenstein};

    struct FixedTransmittance(f64);
    impl Medium for FixedTransmittance {
        fn density_at(&self, _: Vector3<f64>) -> f64 {
            unreachable!()
        }
        fn coefficients_at(&self, _: Vector3<f64>, _: f64) -> MediumCoefficients {
            unreachable!()
        }
        fn sigma_t_majorant(&self, _: f64) -> f64 {
            unreachable!()
        }
        fn phase(&self, _: f64) -> HenyeyGreenstein {
            unreachable!()
        }
        fn sample_segment(
            &self,
            _: &Ray,
            _: f64,
            _: f64,
            _: bool,
            _: &mut dyn RngCore,
        ) -> Result<MediumSample, String> {
            unreachable!()
        }
        fn transmittance_ray(&self, _: &Ray, _: f64, _: f64) -> f64 {
            self.0
        }
    }

    let (mut scene, tracer, ray) = slab(1.5, 2);
    let hit = scene.scene.intersect(&ray).unwrap();
    for tr in [-0.01, 1.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        scene.scene.medium = Some(Box::new(FixedTransmittance(tr)));
        assert_eq!(
            tracer
                .trace(&scene, ray, Some(hit), 550.0, &mut Sequence::new(&[]))
                .unwrap_err(),
            "invalid medium transmittance"
        );
    }
    scene.scene.medium = Some(Box::new(FixedTransmittance(1.0)));
    for distance in [-1.0, 0.0, f64::NAN, f64::INFINITY] {
        let hit = Hit { distance, ..hit };
        assert_eq!(
            tracer
                .trace(&scene, ray, Some(hit), 550.0, &mut Sequence::new(&[]))
                .unwrap_err(),
            "invalid photon segment distance"
        );
    }
}

#[test]
fn four_internal_reflections_then_transmission_deposit_once_and_limit_drops_flux() {
    let (scene, mut tracer, ray) = slab(1.5, 6);
    let mut rng = Sequence::new(&[u64::MAX, 0, 0, 0, 0, u64::MAX]);
    let result = tracer
        .trace(&scene, ray, scene.scene.intersect(&ray), 550.0, &mut rng)
        .unwrap();
    assert_eq!(
        (
            result.state.event_count,
            result.state.reflections,
            result.state.transmissions
        ),
        (6, 4, 2)
    );
    assert_eq!(result.state.medium, PhotonMedium::Air);
    assert_eq!(
        result.landing().unwrap(),
        PhotonLanding {
            position: Vector3::zeros(),
            weight: 1.0
        }
    );
    assert_eq!(rng.draws, 6); // 落地后不再采样或产生第二份贡献。
    tracer.max_events = 2;
    let result = tracer
        .trace(
            &scene,
            ray,
            scene.scene.intersect(&ray),
            550.0,
            &mut Sequence::new(&[u64::MAX, 0]),
        )
        .unwrap();
    assert_eq!(result.termination, PhotonTermination::EventLimit);
    assert_eq!(result.state.medium, PhotonMedium::Glass);
    assert_eq!(result.state.event_count, 2);
    assert!(result.landing().is_none());
}

#[test]
fn tir_keeps_medium_flux_and_unit_direction_without_rng_until_limit() {
    let (scene, tracer, _) = slab(1.5, 8);
    for angle in [50.0_f64, 80.0] {
        let angle = angle.to_radians();
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, 1.5),
            Vector3::new(angle.sin(), 0.0, -angle.cos()),
        )
        .unwrap();
        let mut state = PhotonPathState::new(ray, MediumRegion::Exterior, tracer.object_id, 2.5);
        state.medium = PhotonMedium::Glass;
        let mut rng = Sequence::new(&[]);
        let result = tracer
            .trace_state(
                &scene,
                state,
                scene.scene.intersect(&ray),
                &mut rng,
                |segment| segment.transmittance(&scene.scene, 550.0),
            )
            .unwrap();
        assert_eq!(result.termination, PhotonTermination::EventLimit);
        assert_eq!(
            (result.state.reflections, result.state.transmissions),
            (8, 0)
        );
        assert_eq!(result.state.medium, PhotonMedium::Glass);
        assert_eq!(result.state.flux_weight, 2.5);
        assert!((result.state.ray.direction.norm() - 1.0).abs() < 1e-14);
        assert_eq!(rng.draws, 0);
    }
}

#[test]
fn prism_corner_transmission_does_not_cross_the_adjacent_face_during_ray_spawn() {
    use crate::geometry::vertical_prism::VerticalPrism;
    use nalgebra::Vector2;

    let mut scene = RenderJob::new();
    let object_id = scene.scene.add(
        Box::new(
            VerticalPrism::new(
                [
                    Vector2::new(-0.6, -0.34641016151377546),
                    Vector2::new(0.6, -0.34641016151377546),
                    Vector2::new(0.0, 0.6928203230275509),
                ],
                0.05,
                1.85,
            )
            .unwrap(),
        ),
        Box::new(crate::material::dielectric::Dielectric::new(1.52).unwrap()),
    );
    let ground_id = scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    let tracer = PhotonTracer {
        object_id,
        ground_id,
        glass_ior: 1.5156702734839476,
        max_events: 32,
    };
    let normal = Vector3::new(3.0_f64.sqrt() / 2.0, 0.5, 0.0);
    // 高采样圆盘渲染中实际触发的棱边路径：法线偏移会越过 y_min 面。
    let point =
        Vector3::new(0.5999885804466438, -0.34641038226716264, 1.0474086403116099) + normal * 1e-5;
    let inside = Vector3::new(
        -0.9527120219424032,
        0.09838193295949721,
        -0.2875079103495607,
    );
    let reverse = refract(-inside, -normal, tracer.glass_ior, 1.0).unwrap();
    let ray = Ray::new(point + reverse, -reverse).unwrap();
    let entry = scene.scene.intersect(&ray).unwrap();
    assert!((entry.coords - point).norm() < 1e-12);
    let mut state = PhotonPathState::new(ray, MediumRegion::Exterior, object_id, 1.0);
    tracer
        .scatter(
            &scene.scene,
            &mut state,
            entry,
            &mut Sequence::new(&[u64::MAX]),
        )
        .unwrap();
    assert_eq!(state.medium, PhotonMedium::Glass);
    let exit = scene.scene.intersect(&state.ray).unwrap();
    assert_eq!(exit.object_id, object_id);
    assert!(state.ray.direction.dot(&exit.normal) > 0.0);
    // 独立重现原偏移的错误介质一侧，避免用“跳过错误面”掩盖回归。
    let displaced = Ray::new(entry.coords - entry.normal * 1e-5, state.ray.direction).unwrap();
    let wrong = scene.scene.intersect(&displaced).unwrap();
    assert!(displaced.direction.dot(&wrong.normal) < 0.0);
    let result = tracer
        .trace(
            &scene,
            ray,
            Some(entry),
            550.0,
            &mut Sequence::new(&[u64::MAX; 32]),
        )
        .unwrap();
    assert_eq!(result.state.medium, PhotonMedium::Air);
    assert!(result.landing().is_some());
    assert_eq!(result.state.flux_weight, 1.0);
}

#[test]
fn sampled_slab_flux_matches_analytic_series_and_accounts_for_every_photon() {
    // 透射总和 (1-F)^2 Σ F^(2k) = (1-F)/(1+F)，包含所有内部往返。
    const N: usize = 16_000;
    let tol = |p: f64| 5.0 * (p * (1.0 - p) / N as f64).sqrt() + 1e-4;
    for (ior, angle) in [(1.5, 0.0_f64), (3.0, 0.0), (1.5, 80.0)] {
        let (scene, mut tracer, _) = slab(ior, 32);
        let theta = angle.to_radians();
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, 3.0),
            Vector3::new(theta.sin(), 0.0, -theta.cos()),
        )
        .unwrap();
        let f = fresnel_dielectric(theta.cos(), 1.0, ior).unwrap();
        for limit in [2, 32] {
            tracer.max_events = limit;
            let mut rng = ChaCha12Rng::seed_from_u64(42);
            let mut totals = [0.0; 3]; // 接收、逃逸、截断；三个互斥终态。
            let mut two = 0.0;
            for _ in 0..N {
                let result = tracer
                    .trace(&scene, ray, scene.scene.intersect(&ray), 550.0, &mut rng)
                    .unwrap();
                let bucket = match result.termination {
                    PhotonTermination::Ground(_) => 0,
                    PhotonTermination::Escaped => 1,
                    PhotonTermination::EventLimit => 2,
                    PhotonTermination::Blocked => panic!("玻璃板无其他遮挡"),
                };
                totals[bucket] += result.state.flux_weight;
                if bucket == 0 && result.state.event_count == 2 {
                    two += result.state.flux_weight;
                }
            }
            assert_eq!(totals.iter().sum::<f64>(), N as f64);
            let p_two = (1.0 - f).powi(2);
            assert!((two / N as f64 - p_two).abs() < tol(p_two));
            let expected = if limit == 2 {
                p_two
            } else {
                (1.0 - f) / (1.0 + f)
            };
            assert!((totals[0] / N as f64 - expected).abs() < tol(expected));
            if limit == 2 {
                assert!((totals[1] / N as f64 - f).abs() < tol(f));
                let p_cut = (1.0 - f) * f;
                assert!((totals[2] / N as f64 - p_cut).abs() < tol(p_cut));
            } else {
                assert_eq!(totals[2], 0.0);
            }
        }
    }
}

#[test]
fn external_reflection_can_reach_ground_and_other_objects_absorb() {
    let mut scene = RenderJob::new();
    let object_id = scene.scene.add(
        Box::new(Sphere::new(Vector3::new(0.0, 0.0, 1.5), 0.8).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let ground_id = scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    scene.photon.ground_id = Some(ground_id);
    scene.select_caustics_object(object_id).unwrap();
    let ray = Ray::new(Vector3::new(2.0, 0.0, 1.2), -Vector3::x()).unwrap();
    let result = crate::integrator::photon::trace_photon(
        &scene,
        ray,
        None,
        IorMode::Constant,
        1,
        &mut Sequence::new(&[0]),
    )
    .unwrap();
    assert_eq!(
        (result.state.reflections, result.state.transmissions),
        (1, 0)
    );
    assert_eq!(result.landing().unwrap().weight, 1.0);
    scene.scene.add(
        Box::new(Ground::new(0.5).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    let result = crate::integrator::photon::trace_photon(
        &scene,
        ray,
        None,
        IorMode::Constant,
        1,
        &mut Sequence::new(&[0]),
    )
    .unwrap();
    assert_eq!(result.termination, PhotonTermination::Blocked);
    assert!(result.landing().is_none());
}

#[test]
fn bound_sphere_uses_launch_region_and_preserves_it_on_external_reflection() {
    use crate::integrator::photon::trace_photon;
    use crate::material::lambertian::Lambertian;

    let mut job = RenderJob::new();
    let target = job.scene.add(
        Box::new(Sphere::new(Vector3::z() * 2.0, 1.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let ground = job.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    job.photon.ground_id = Some(ground);
    job.select_caustics_object(target).unwrap();
    let absorption = |sigma_a: f64| {
        Box::new(
            HomogeneousMedium::try_from(&HomogeneousMediumConfig {
                min_m: [-10.0; 3],
                max_m: [10.0; 3],
                sigma_a: sigma_a.into(),
                sigma_s: 0.0.into(),
                g: 0.0.into(),
            })
            .unwrap(),
        )
    };
    job.scene.medium = Some(absorption(0.2));
    job.scene
        .bind_medium_interface(
            target,
            MediumInterface {
                interior: Some(absorption(0.7)),
            },
        )
        .unwrap();

    let trace = |ray, draws: &[u64]| {
        trace_photon(
            &job,
            ray,
            None,
            IorMode::Constant,
            2,
            &mut Sequence::new(draws),
        )
    };
    for (height, dz, draws, events, optical_depth) in [
        (4.0, -1.0, [u64::MAX; 2], (2, 0), 1.8_f64),
        (2.0, -1.0, [u64::MAX; 2], (1, 0), 0.9),
        (2.0, 1.0, [0, u64::MAX], (1, 1), 2.3),
    ] {
        let ray = Ray::new(Vector3::z() * height, Vector3::z() * dz).unwrap();
        let result = trace(ray, &draws).unwrap();
        assert_eq!(
            (result.state.transmissions, result.state.reflections),
            events
        );
        assert_eq!(result.state.medium, PhotonMedium::Air);
        assert_eq!(result.state.medium_region, MediumRegion::Exterior);
        assert!((result.landing().unwrap().weight - (-optical_depth).exp()).abs() < 1e-14);
    }

    // The normal at this front-face hit is (0.8, 0, -0.6). Its reflection
    // travels down to the ground with a vertical component of -0.96.
    let ray = Ray::new(Vector3::new(2.0, 0.0, 1.4), -Vector3::x()).unwrap();
    let result = trace(ray, &[0]).unwrap();
    assert_eq!(result.state.reflections, 1);
    assert_eq!(result.state.medium, PhotonMedium::Air);
    assert_eq!(result.state.medium_region, MediumRegion::Exterior);
    let expected = (-0.2_f64 * (1.2 + 1.4 / 0.96)).exp();
    assert!((result.landing().unwrap().weight - expected).abs() < 1e-14);

    let boundary = Ray::new(Vector3::z(), -Vector3::z()).unwrap();
    assert_eq!(
        trace(boundary, &[]).unwrap_err(),
        "boundary launch requires a departure side"
    );
}

#[test]
fn direct_miss_invalid_inputs_and_inside_launch_have_explicit_outcomes() {
    let (scene, mut tracer, _) = slab(1.5, 2);
    for (origin, direction, expected) in [
        (
            Vector3::new(0.0, 0.0, 0.5),
            -Vector3::z(),
            PhotonTermination::Ground(Vector3::zeros()),
        ),
        (
            Vector3::new(0.0, 0.0, 3.0),
            Vector3::z(),
            PhotonTermination::Escaped,
        ),
    ] {
        let ray = Ray::new(origin, direction).unwrap();
        let result = tracer
            .trace(
                &scene,
                ray,
                scene.scene.intersect(&ray),
                550.0,
                &mut Sequence::new(&[]),
            )
            .unwrap();
        assert_eq!(result.termination, expected);
        assert_eq!(result.state.event_count, 0);
    }
    let ray = Ray::new(Vector3::new(0.0, 0.0, 1.5), Vector3::z()).unwrap();
    assert!(
        tracer
            .trace(
                &scene,
                ray,
                scene.scene.intersect(&ray),
                550.0,
                &mut Sequence::new(&[])
            )
            .is_err()
    );
    for ior in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        tracer.glass_ior = ior;
        assert!(
            PhotonTracer::new(
                &scene,
                tracer.object_id,
                tracer.ground_id,
                tracer.glass_ior,
                tracer.max_events
            )
            .is_err()
        );
    }
    tracer.glass_ior = 1.5;
    for limit in [0, 1025, usize::MAX] {
        tracer.max_events = limit;
        assert!(
            PhotonTracer::new(
                &scene,
                tracer.object_id,
                tracer.ground_id,
                tracer.glass_ior,
                tracer.max_events
            )
            .is_err()
        );
    }
}
#[test]
fn transmitted_slab_matches_analytic_snell_landing_without_eta_squared() {
    for ior in [1.0, 1.5, 2.0] {
        let (scene, tracer, _) = slab(ior, 2);
        for angle in [0.0_f64, 60.0, 80.0] {
            let theta = angle.to_radians();
            let ray = Ray::new(
                Vector3::new(0.0, 0.0, 3.0),
                Vector3::new(theta.sin(), 0.0, -theta.cos()),
            )
            .unwrap();
            let mut rng = Sequence::new(&[u64::MAX; 2]);
            let result = tracer
                .trace(&scene, ray, scene.scene.intersect(&ray), 550.0, &mut rng)
                .unwrap();
            let sin_t = theta.sin() / ior;
            let tan_t = sin_t / (1.0 - sin_t * sin_t).sqrt();
            let analytic = Vector3::new(2.0 * theta.tan() + tan_t, 0.0, 0.0);
            assert!((result.landing().unwrap().position - analytic).norm() < 1e-12);
            assert_eq!(result.state.flux_weight, 1.0);
            assert_eq!(rng.draws, if ior == 1.0 { 0 } else { 2 });
        }
    }
}

fn uniform_draw(u: f64) -> u64 {
    (u * u64::MAX as f64) as u64
}

fn reflecting_sphere() -> (RenderJob, usize, Ray) {
    let mut job = RenderJob::new();
    let target = job.scene.add(
        Box::new(Sphere::new(Vector3::z() * 2.0, 1.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    job.photon.ground_id = Some(job.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(crate::material::lambertian::Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    ));
    job.select_caustics_object(target).unwrap();
    let ray = Ray::new(Vector3::z() * 4.0, -Vector3::z()).unwrap();
    (job, target, ray)
}

#[test]
fn photon_volume_tracks_grid_null_then_real_on_an_escape_segment() {
    use crate::integrator::photon::{trace_photon, trace_photon_with_volume};

    let (mut job, _, ray) = reflecting_sphere();
    job.scene.medium = Some(Box::new(
        GridMedium::try_from(&GridMediumConfig {
            min_m: [-20.0, -20.0, 3.5],
            max_m: [20.0, 20.0, 5.5],
            resolution: [1, 1, 2],
            densities: vec![0.25, 1.0],
            sigma_a_base: 0.6.into(),
            sigma_s_base: 0.4.into(),
            g: 0.0.into(),
        })
        .unwrap(),
    ));
    let prefix_tr = (-0.125_f64).exp();
    for result in [
        trace_photon(
            &job,
            ray,
            None,
            IorMode::Constant,
            1,
            &mut Sequence::new(&[0]),
        )
        .unwrap(),
        trace_photon_with_volume(
            &job,
            ray,
            None,
            IorMode::Constant,
            1,
            0,
            &mut Sequence::new(&[0]),
        )
        .unwrap(),
    ] {
        assert_eq!(result.termination, PhotonTermination::Escaped);
        assert_eq!(result.state.volume_scatters, 0);
        assert_eq!(result.state.flux_weight, prefix_tr);
    }

    // Reflect upward, then reject a candidate at z=3.75 and accept one at z=4.
    // The two candidates are in the same coarse block as the denser voxel.
    let step = -(-0.25_f64).exp_m1();
    let mut rng = Sequence::new(&[0.0, step, 0.75, step, 0.125, 0.375, 0.0].map(uniform_draw));
    let result =
        trace_photon_with_volume(&job, ray, None, IorMode::Constant, 1, 1, &mut rng).unwrap();
    assert_eq!(rng.draws, 7);
    assert_eq!(result.state.event_count, 1);
    assert_eq!(result.state.reflections, 1);
    assert_eq!(result.state.volume_scatters, 1);
    assert!((result.state.ray.origin.z - 4.0).abs() < 1e-12);
    // The HG sample has dz=-1/4 and crosses another 2 m of the low-density
    // voxel before leaving the box. The new direction misses the glass.
    let landing = result.landing().unwrap();
    assert!((landing.position.x - 4.0 * 15.0_f64.sqrt()).abs() < 1e-12);
    assert!((landing.weight - prefix_tr * 0.4 * (-0.5_f64).exp()).abs() < 1e-14);
}

#[test]
fn photon_volume_cannot_deposit_on_the_back_of_the_receiver() {
    use crate::integrator::photon::trace_photon_with_volume;
    use crate::material::lambertian::Lambertian;
    use nalgebra::Vector2;

    let mut job = RenderJob::new();
    let target = job.scene.add(
        Box::new(Sphere::new(Vector3::new(-1.0, 0.0, -1.0), 0.25).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let ground = job.scene.add(
        Box::new(Ground::with_bounds(0.0, Vector2::repeat(-0.25), Vector2::repeat(0.25)).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    job.photon.ground_id = Some(ground);
    job.select_caustics_object(target).unwrap();
    job.scene.medium = Some(Box::new(
        HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: [-2.0, -2.0, -1.5],
            max_m: [2.0, 2.0, -0.01],
            sigma_a: 0.5.into(),
            sigma_s: 0.5.into(),
            g: 0.0.into(),
        })
        .unwrap(),
    ));
    // Miss the finite floor on the way down, reflect off the glass at
    // z=-0.75, then scatter from (-1, 0, -0.5) toward the floor's underside.
    let ray = Ray::new(Vector3::new(-1.0, 0.0, 2.0), -Vector3::z()).unwrap();
    let draws = [
        0.0,
        -(-0.25_f64).exp_m1(),
        (1.0 + 1.0 / 5.0_f64.sqrt()) / 2.0,
        0.0,
    ]
    .map(uniform_draw);
    let result = trace_photon_with_volume(
        &job,
        ray,
        None,
        IorMode::Constant,
        1,
        1,
        &mut Sequence::new(&draws),
    )
    .unwrap();
    assert_eq!(
        (result.state.reflections, result.state.volume_scatters),
        (1, 1)
    );
    let hit = job.scene.intersect(&result.state.ray).unwrap();
    assert_eq!(hit.object_id, ground);
    assert!(hit.coords.norm() < 1e-14);
    assert!(hit.normal.dot(&result.state.ray.direction) > 0.0);
    assert!(result.state.flux_weight > 0.0);
    assert_eq!(result.termination, PhotonTermination::Blocked);
    assert!(result.landing().is_none());
}

#[test]
fn photon_volume_rejects_a_position_rounded_to_the_dielectric_surface() {
    use crate::integrator::photon::trace_photon_with_volume;

    let (mut job, _, ray) = reflecting_sphere();
    // The first RNG word reflects off the sphere at z=3. A nonzero free
    // path below the precision of that coordinate cannot spawn a volume ray.
    job.scene.medium = Some(Box::new(
        HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: [-1.0, -1.0, 3.0],
            max_m: [1.0, 1.0, 5.0],
            sigma_a: 0.0.into(),
            sigma_s: 1.0.into(),
            g: 0.0.into(),
        })
        .unwrap(),
    ));
    let mut rng = Sequence::new(&[0, 0, 0, 0]);
    let error =
        trace_photon_with_volume(&job, ray, None, IorMode::Constant, 2, 1, &mut rng).unwrap_err();
    assert_eq!(error, "medium collision rounds to a ray segment endpoint");
    assert_eq!(rng.draws, 2);
}

#[test]
fn photon_volume_budgets_are_independent_at_a_nearby_glass_hit() {
    use crate::integrator::photon::trace_photon_with_volume;

    let (mut job, target, ray) = reflecting_sphere();
    job.scene.medium = Some(Box::new(
        HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: [-1.0, -1.0, 3.0],
            max_m: [1.0, 1.0, 5.0],
            sigma_a: 0.5.into(),
            sigma_s: 0.5.into(),
            g: 0.0.into(),
        })
        .unwrap(),
    ));
    let distance = 0.25 * crate::geometry::ray::INTERSECTION_EPSILON;
    // Reflect upward, then backscatter toward the same interface.
    let draws = [0.0, -(-distance).exp_m1(), 0.0, 0.0, 0.0].map(uniform_draw);
    for bound in [false, true] {
        if bound {
            job.scene
                .bind_medium_interface(target, MediumInterface { interior: None })
                .unwrap();
        }
        for max_events in [1, 2] {
            let mut rng = Sequence::new(&draws);
            let result = trace_photon_with_volume(
                &job,
                ray,
                None,
                IorMode::Constant,
                max_events,
                1,
                &mut rng,
            )
            .unwrap();
            assert_eq!(result.state.volume_scatters, 1);
            assert_eq!(result.state.event_count, max_events);
            assert_eq!(result.state.reflections, max_events);
            assert_eq!(rng.draws, 3 + max_events);
            let (termination, final_distance) = if max_events == 1 {
                (PhotonTermination::EventLimit, 0.0)
            } else {
                (PhotonTermination::Escaped, 2.0)
            };
            assert_eq!(result.termination, termination);
            assert!(
                (result.state.flux_weight - 0.5 * (-1.0 - distance - final_distance).exp()).abs()
                    < 1e-14
            );
        }
    }
}

#[test]
fn photon_volume_slab_lands_after_one_or_two_real_events() {
    use crate::integrator::photon::trace_photon_with_volume;

    let (mut job, _, ray) = slab(1.0, 2);
    job.scene.medium = Some(Box::new(
        HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: [-10.0, -10.0, 0.0],
            max_m: [10.0, 10.0, 1.0],
            sigma_a: 1.5.into(),
            sigma_s: 0.5.into(),
            g: 0.0.into(),
        })
        .unwrap(),
    ));
    let draws = [
        -(-0.4_f64).exp_m1(),
        0.75,
        0.0,
        -(-0.6_f64).exp_m1(),
        1.0,
        0.0,
    ]
    .map(uniform_draw);
    for (budget, remaining_distance) in [(1, 1.6), (2, 1.3)] {
        let mut rng = Sequence::new(&draws[..3 * budget]);
        let result =
            trace_photon_with_volume(&job, ray, None, IorMode::Constant, 2, budget, &mut rng)
                .unwrap();
        assert_eq!(rng.draws, 3 * budget);
        assert_eq!(result.state.volume_scatters, budget);
        assert_eq!(result.state.transmissions, 2);
        let landing = result.landing().unwrap();
        let expected = 0.25_f64.powi(budget as i32) * (-2.0_f64 * remaining_distance).exp();
        assert!((landing.weight - expected).abs() < 1e-14);
    }

    let height = 0.25 * crate::geometry::ray::INTERSECTION_EPSILON;
    let u = -(-2.0 * (1.0 - height)).exp_m1();
    let result = trace_photon_with_volume(
        &job,
        ray,
        None,
        IorMode::Constant,
        2,
        1,
        &mut Sequence::new(&[u, 0.75, 0.0].map(uniform_draw)),
    )
    .unwrap();
    assert!((result.state.ray.origin.z - height).abs() < 1e-14);
    assert!((result.landing().unwrap().weight - 0.25 * (-4.0 * height).exp()).abs() < 1e-14);

    // A stratified sweep of the distance variate measures the zero-collision
    // share of landed flux. Each surviving sample must retain unit weight.
    let samples = 1024;
    let mut survived = 0;
    for i in 0..samples {
        let u = (i as f64 + 0.5) / samples as f64;
        let result = trace_photon_with_volume(
            &job,
            ray,
            None,
            IorMode::Constant,
            2,
            1,
            &mut Sequence::new(&[uniform_draw(u), u64::MAX, 0]),
        )
        .unwrap();
        if result.state.volume_scatters == 0 {
            assert_eq!(result.landing().unwrap().weight, 1.0);
            survived += 1;
        }
    }
    assert!((survived as f64 / samples as f64 - (-2.0_f64).exp()).abs() < 1.0 / samples as f64);
}

#[test]
fn photon_volume_uses_the_wavelength_of_the_interior_phase() {
    use crate::integrator::photon::trace_photon_with_volume;

    let (mut job, target, ray) = reflecting_sphere();
    job.scene
        .bind_medium_interface(
            target,
            MediumInterface {
                interior: Some(Box::new(
                    HomogeneousMedium::try_from(&HomogeneousMediumConfig {
                        min_m: [-1.0, -1.0, 1.0],
                        max_m: [1.0, 1.0, 3.0],
                        sigma_a: 0.5.into(),
                        sigma_s: 0.5.into(),
                        g: crate::medium::HgAnisotropy::Curve {
                            samples: vec![[380.0, -0.7], [780.0, 0.7]],
                        },
                    })
                    .unwrap(),
                )),
            },
        )
        .unwrap();
    let trace = |nm, budget| {
        let draws = [1.0, -(-0.25_f64).exp_m1(), 0.5, 0.0].map(uniform_draw);
        trace_photon_with_volume(
            &job,
            ray,
            nm,
            IorMode::Constant,
            1,
            budget,
            &mut Sequence::new(&draws),
        )
    };
    for (nm, sign) in [(380.0, 1.0), (780.0, -1.0)] {
        let result = trace(Some(nm), 1).unwrap();
        assert_eq!(result.termination, PhotonTermination::EventLimit);
        assert_eq!(result.state.volume_scatters, 1);
        assert_eq!(result.state.transmissions, 1);
        assert_eq!(result.state.medium, PhotonMedium::Glass);
        assert_eq!(result.state.medium_region, MediumRegion::Interior(target));
        assert!((result.state.ray.origin.z - 2.75).abs() < 1e-14);
        assert!(result.state.ray.direction.z * sign > 0.7);
    }
    for nm in [None, Some(379.0), Some(f64::NAN)] {
        assert!(trace(nm, 1).is_err());
    }
    assert!(trace(Some(550.0), 1025).is_err());
    assert_eq!(trace(None, 0).unwrap().state.volume_scatters, 0);
}

#[test]
fn photon_volume_rejects_out_of_segment_and_off_ray_custom_medium_collisions() {
    use crate::integrator::photon::trace_photon_with_volume;
    use crate::medium::{MediumCoefficients, MediumSample, phase::HenyeyGreenstein};

    struct FixedCollision(Vector3<f64>);
    impl Medium for FixedCollision {
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
            _: &Ray,
            _: f64,
            _: f64,
            allow_scattering: bool,
            _: &mut dyn RngCore,
        ) -> Result<MediumSample, String> {
            Ok(MediumSample {
                position: allow_scattering.then_some(self.0),
                weight: 1.0,
            })
        }
    }

    // Ray hits the top of the unit sphere at z=3 from z=5 and reflects upward (+z),
    // or transmits downward (-z) across z=3..1. Force transmission (draw = 1.0) so
    // the second segment is finite: origin=(0,0,3), dir=(0,0,-1), max_distance=2.
    for (position, expected) in [
        (
            Vector3::new(0.0, 0.0, 3.5),
            "medium collision lies outside the ray segment",
        ),
        (
            Vector3::new(0.0, 0.0, 0.5),
            "medium collision lies outside the ray segment",
        ),
        (
            Vector3::new(0.25, 0.0, 2.0),
            "medium collision lies off the ray",
        ),
    ] {
        let (mut job, _, ray) = reflecting_sphere();
        job.scene.medium = Some(Box::new(FixedCollision(position)));
        let draws = [uniform_draw(1.0)];
        let error = trace_photon_with_volume(
            &job,
            ray,
            None,
            IorMode::Constant,
            2,
            1,
            &mut Sequence::new(&draws),
        )
        .unwrap_err();
        assert_eq!(error, expected);
    }
}

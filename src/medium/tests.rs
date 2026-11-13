use super::*;
use rand::{SeedableRng, rngs::StdRng};
use std::f64::consts::PI;

fn medium(a: f64, s: f64) -> HomogeneousMedium {
    HomogeneousMedium::try_from(&HomogeneousMediumConfig {
        min_m: [-1.0; 3],
        max_m: [1.0; 3],
        sigma_a: a,
        sigma_s: s,
        g: 0.0,
    })
    .unwrap()
}

#[test]
fn intervals_clip_inside_outside_parallel_tangent_and_surface_segments() {
    let m = medium(0.2, 0.3);
    let ray = |o, d| Ray::new(o, d).unwrap();
    let r = ray(Vector3::new(0.0, 0.0, 3.0), -Vector3::z());
    assert_eq!(m.interval(&r, f64::INFINITY), Some((2.0, 4.0)));
    assert_eq!(m.interval(&r, 3.0), Some((2.0, 3.0)));
    assert_eq!(m.interval(&r, 2.0), None);
    assert_eq!(
        m.interval(&ray(Vector3::zeros(), Vector3::x()), 9.0),
        Some((0.0, 1.0))
    );
    assert_eq!(
        m.interval(&ray(Vector3::new(2.0, 0.0, 0.0), Vector3::z()), 9.0),
        None
    );
    assert_eq!(m.interval(&ray(Vector3::z(), Vector3::z()), 9.0), None);
    assert_eq!(
        m.interval(&ray(Vector3::new(1.0, 0.0, 0.0), Vector3::z()), 9.0),
        Some((0.0, 1.0))
    );
    let diagonal = ray(Vector3::repeat(-2.0), Vector3::repeat(1.0));
    let (a, b) = m.interval(&diagonal, 20.0).unwrap();
    assert!((b - a - 2.0 * 3.0_f64.sqrt()).abs() < 1e-12);
    assert!((m.transmittance_ray(&r, 3.0) - (-0.5_f64).exp()).abs() < 1e-15);
}

#[test]
fn beer_lambert_and_configuration_validate_physical_parameters() {
    let m = medium(0.2, 0.3);
    assert_eq!(m.sigma_a(), 0.2);
    assert_eq!(m.sigma_s(), 0.3);
    assert_eq!(m.sigma_t(), 0.5);
    assert_eq!(m.transmittance(0.0), 1.0);
    assert_eq!(m.transmittance(2.0), (-1.0_f64).exp());
    assert_eq!(m.transmittance(f64::INFINITY), 0.0);
    assert_eq!(medium(0.0, 0.0).transmittance(f64::INFINITY), 1.0);
    let c = HomogeneousMediumConfig {
        min_m: [-1.0; 3],
        max_m: [1.0; 3],
        sigma_a: 0.0,
        sigma_s: 0.1,
        g: 0.7,
    };
    for bad in [
        HomogeneousMediumConfig { sigma_a: -0.1, ..c },
        HomogeneousMediumConfig {
            sigma_s: f64::NAN,
            ..c
        },
        HomogeneousMediumConfig {
            sigma_a: f64::MAX,
            sigma_s: f64::MAX,
            ..c
        },
        HomogeneousMediumConfig { g: 1.0, ..c },
        HomogeneousMediumConfig { g: -1.0, ..c },
        HomogeneousMediumConfig {
            min_m: [1.0; 3],
            ..c
        },
        HomogeneousMediumConfig {
            max_m: [f64::INFINITY; 3],
            ..c
        },
    ] {
        assert!(HomogeneousMedium::try_from(&bad).is_err());
    }
}

#[test]
fn free_flight_survival_density_and_implicit_absorption_conserve_probability() {
    let m = medium(0.2, 0.3);
    let r = Ray::new(Vector3::new(0.0, 0.0, 2.0), -Vector3::z()).unwrap();
    let mut rng = StdRng::seed_from_u64(42);
    let n = 150_000;
    let (mut escaped, mut scatter_weight, mut distance) = (0, 0.0, 0.0);
    for _ in 0..n {
        let s = m.sample_segment(&r, 3.0, false, &mut rng);
        if let Some(p) = s.position {
            scatter_weight += s.weight;
            distance += 1.0 - p.z;
        } else {
            escaped += 1;
            assert_eq!(s.weight, 1.0);
        }
    }
    let tr = m.transmittance(2.0);
    assert!((escaped as f64 / n as f64 - tr).abs() < 0.004);
    assert!((scatter_weight / n as f64 - 0.6 * (1.0 - tr)).abs() < 0.003);
    let conditional_mean = 1.0 / m.sigma_t() - 2.0 * tr / (1.0 - tr);
    assert!((distance / (n - escaped) as f64 - conditional_mean).abs() < 0.008);
    let scatter = m.sample_segment(&r, 3.0, true, &mut rng);
    assert!(scatter.position.is_none());
    assert_eq!(scatter.weight, tr);
    let absorption = medium(0.5, 0.0).sample_segment(&r, 3.0, false, &mut rng);
    assert!(absorption.position.is_none());
    assert_eq!(absorption.weight, tr);
}

#[test]
fn hg_is_normalized_reciprocal_and_has_the_pbrt_forward_sign() {
    let wo = Vector3::z();
    for g in [-0.9, -0.7, -1e-8, 0.0, 1e-8, 0.7, 0.9] {
        let phase = HenyeyGreenstein::new(g).unwrap();
        let n = 40_000;
        let mut integral = 0.0;
        for i in 0..n {
            let c = -1.0 + 2.0 * (i as f64 + 0.5) / n as f64;
            let wi = Vector3::new((1.0 - c * c).sqrt(), 0.0, c);
            let p = phase.eval(wo, wi);
            assert_eq!(p, phase.pdf(wo, wi));
            assert_eq!(p, phase.eval(wi, wo));
            if g == 0.0 {
                assert_eq!(p, 1.0 / (4.0 * PI));
            }
            integral += p * 4.0 * PI / n as f64;
        }
        assert!((integral - 1.0).abs() < 2e-5, "g={g}: {integral}");
        if g > 0.0 {
            assert!(phase.eval(wo, -wo) > phase.eval(wo, wo));
        }
    }
}

#[test]
fn hg_samples_match_independent_pdf_bin_integrals_and_moments() {
    let wo = Vector3::new(0.2, -0.3, 0.9).normalize();
    for g in [-0.9, -0.7, -1e-8, 0.0, 1e-8, 0.7, 0.9] {
        let phase = HenyeyGreenstein::new(g).unwrap();
        let mut rng = StdRng::seed_from_u64(918);
        let n = 120_000;
        let mut bins = [0; 20];
        let (mut first, mut second) = (0.0, 0.0);
        for _ in 0..n {
            let s = phase.sample(wo, &mut rng);
            assert!((s.wi.norm() - 1.0).abs() < 1e-12);
            assert_eq!(s.p, s.pdf);
            assert_eq!(s.pdf, phase.eval(wo, s.wi));
            let c = wo.dot(&s.wi).clamp(-1.0, 1.0);
            bins[(((c + 1.0) * 10.0) as usize).min(19)] += 1;
            first += c;
            second += c * c;
        }
        assert!((first / n as f64 + g).abs() < 0.006);
        assert!((second / n as f64 - (1.0 + 2.0 * g * g) / 3.0).abs() < 0.005);
        for (b, count) in bins.iter().enumerate() {
            let mut expected = 0.0;
            for j in 0..2000 {
                let c = -1.0 + 0.1 * (b as f64 + (j as f64 + 0.5) / 2000.0);
                let wi = Vector3::new((1.0 - c * c).sqrt(), 0.0, c);
                expected += phase.pdf(Vector3::z(), wi) * 2.0 * PI * 0.1 / 2000.0;
            }
            assert!(
                (*count as f64 / n as f64 - expected).abs() < 0.006,
                "g={g}, bin={b}"
            );
        }
    }
}

#[test]
fn hg_inverse_cdf_keeps_the_zero_sample_endpoint_near_extreme_asymmetry() {
    struct ZeroRng;
    impl rand::RngCore for ZeroRng {
        fn next_u32(&mut self) -> u32 {
            0
        }
        fn next_u64(&mut self) -> u64 {
            0
        }
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            dest.fill(0);
        }
    }
    for g in [0.0, 0.7, -0.7, 1.0 - 1e-12, -1.0 + 1e-12] {
        let phase = HenyeyGreenstein::new(g).unwrap();
        let sample = phase.sample(Vector3::z(), &mut ZeroRng);
        let expected = if g < 0.0 { -Vector3::z() } else { Vector3::z() };
        assert!(
            (sample.wi - expected).norm() < 1e-12,
            "g={g}: {:?}",
            sample.wi
        );
        assert!(sample.pdf.is_finite() && sample.pdf > 0.0);
    }
}

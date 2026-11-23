use super::*;
use crate::material::to_world;
use crate::optics::interface::fresnel_dielectric;
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::f64::consts::PI;

fn metal(roughness: f64) -> GgxMetal {
    GgxMetal::new(
        Vector3::new(0.2, 0.8, 1.5),
        Vector3::new(3.0, 2.0, 1.0),
        roughness,
    )
    .unwrap()
}
fn rgb(m: &GgxMetal, wi: Vector3<f64>, wo: Vector3<f64>, n: Vector3<f64>) -> Vector3<f64> {
    m.evaluate_rgb(wi, wo, n)
}
#[test]
fn ggx_samples_match_full_solid_angle_pdf_weight_and_replay() {
    for roughness in [0.05, 0.4, 1.0] {
        let m = metal(roughness);
        let mut rng = StdRng::seed_from_u64(42);
        let mut replay = rng.clone();
        for n in [Vector3::z(), Vector3::new(1.0, -2.0, 3.0).normalize()] {
            let wo = to_world(Vector3::new(0.8, 0.0, 0.6), n).unwrap();
            for _ in 0..512 {
                let sample = m.sample(wo, n, &mut rng);
                assert_eq!(sample, m.sample(wo, n, &mut replay));
                let Some(s) = sample else {
                    continue;
                };
                assert_eq!(s.event, ScatterType::GlossyReflection);
                let p = m.pdf(s.wi, wo, n);
                assert_eq!(s.pdf, Some(p));
                assert!(p > 0.0 && p.is_finite());
                assert!((s.wi.norm() - 1.0).abs() < 1e-12);
                let f = rgb(&m, s.wi, wo, n);
                assert!(
                    (s.weight - f * (n.dot(&s.wi) / p)).norm() < 1e-10 * (1.0 + s.weight.norm())
                );
                assert!((f - rgb(&m, wo, s.wi, n)).norm() < 1e-8 * (1.0 + f.norm()));
            }
        }
    }
}
#[test]
fn ggx_evaluation_matches_independent_d_g_and_dielectric_fresnel_reference() {
    let m = GgxMetal::new(Vector3::repeat(1.5), Vector3::zeros(), 0.6).unwrap();
    let (wi, wo) = (
        Vector3::new(0.6, 0.0, 0.8),
        Vector3::new(-0.3, 0.4, 0.75_f64.sqrt()),
    );
    let h = (wi + wo).normalize();
    let alpha2 = 0.6_f64.powi(4);
    let d = alpha2 / (PI * ((alpha2 - 1.0) * h.z * h.z + 1.0).powi(2));
    let lambda = |c: f64| ((1.0 + alpha2 * (1.0 - c * c) / (c * c)).sqrt() - 1.0) / 2.0;
    let g = 1.0 / (1.0 + lambda(wi.z) + lambda(wo.z));
    let f = fresnel_dielectric(wo.dot(&h), 1.0, 1.5).unwrap();
    let expected = Vector3::repeat(f * d * g / (4.0 * wi.z * wo.z));
    assert!((rgb(&m, wi, wo, Vector3::z()) - expected).norm() < 1e-14);
    let p = d * h.z / (4.0 * wo.dot(&h));
    assert!((m.pdf(wi, wo, Vector3::z()) - p).abs() < 1e-14);
}
#[test]
fn ggx_pdf_mass_matches_acceptance_and_weight_mean_matches_hemisphere_quadrature() {
    for roughness in [0.5, 1.0] {
        let m = metal(roughness);
        for wo in [Vector3::z(), Vector3::new(0.8, 0.0, 0.6)] {
            let mut integral = Vector3::zeros();
            let mut pdf_mass = 0.0;
            for z in 0..64 {
                let c = (z as f64 + 0.5) / 64.0;
                for phi in 0..128 {
                    let angle = 2.0 * PI * (phi as f64 + 0.5) / 128.0;
                    let r = (1.0 - c * c).sqrt();
                    let wi = Vector3::new(r * angle.cos(), r * angle.sin(), c);
                    integral += rgb(&m, wi, wo, Vector3::z()) * (c * 2.0 * PI / (64.0 * 128.0));
                    pdf_mass += m.pdf(wi, wo, Vector3::z()) * 2.0 * PI / (64.0 * 128.0);
                }
            }
            let samples = 12_000;
            let mut mean = Vector3::zeros();
            let mut accepted = 0;
            let mut rng = StdRng::seed_from_u64(42);
            for _ in 0..samples {
                if let Some(sample) = m.sample(wo, Vector3::z(), &mut rng) {
                    mean += sample.weight;
                    accepted += 1;
                }
            }
            mean /= samples as f64; // rejected half-vectors remain in the denominator.
            assert!(
                (mean - integral).norm() < 0.025,
                "{roughness} {wo}: {mean} vs {integral}"
            );
            assert!((accepted as f64 / samples as f64 - pdf_mass).abs() < 0.02);
            assert!(integral.iter().all(|v| *v >= 0.0 && *v <= 1.0001));
            assert!(accepted < samples);
        }
    }
}
#[test]
fn smooth_ggx_is_specular_with_conductor_fresnel_and_no_draws_or_regular_density() {
    let wo = Vector3::new(0.6, 0.0, 0.8);
    for roughness in [0.0, SMOOTH_ROUGHNESS / 2.0, SMOOTH_ROUGHNESS] {
        let m = metal(roughness);
        let mut rng = StdRng::seed_from_u64(42);
        let mut unchanged = rng.clone();
        let sample = m.sample(wo, Vector3::z(), &mut rng).unwrap();
        assert_eq!(sample.event, ScatterType::SpecularReflection);
        assert_eq!(sample.pdf, None);
        assert_eq!(sample.weight, ggx::fresnel(0.8, m.eta, m.k));
        assert!((sample.wi - Vector3::new(-0.6, 0.0, 0.8)).norm() < 1e-14);
        assert_eq!(rgb(&m, sample.wi, wo, Vector3::z()), Vector3::zeros());
        assert_eq!(m.pdf(sample.wi, wo, Vector3::z()), 0.0);
        assert_eq!(rng.random::<u64>(), unchanged.random::<u64>());
    }
    assert!(metal(SMOOTH_ROUGHNESS * 1.01).has_non_delta());
}
#[test]
fn ggx_rejects_invalid_parameters_and_geometric_queries_and_has_no_fake_spectrum() {
    for v in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(GgxMetal::new(Vector3::repeat(v), Vector3::repeat(1.0), 0.5).is_err());
        assert!(GgxMetal::new(Vector3::repeat(1.0), Vector3::repeat(v), 0.5).is_err());
    }
    assert!(GgxMetal::new(Vector3::zeros(), Vector3::repeat(1.0), 0.5).is_err());
    for roughness in [-1.0, 1.1, f64::NAN, f64::INFINITY] {
        assert!(GgxMetal::new(Vector3::repeat(1.0), Vector3::repeat(1.0), roughness).is_err());
    }
    let m = metal(0.5);
    let mut rng = StdRng::seed_from_u64(42);
    let mut unchanged = rng.clone();
    for invalid in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
        -Vector3::z(),
        Vector3::x(),
    ] {
        assert!(m.sample(invalid, Vector3::z(), &mut rng).is_none());
        assert_eq!(m.pdf(invalid, Vector3::z(), Vector3::z()), 0.0);
        assert_eq!(
            rgb(&m, invalid, Vector3::z(), Vector3::z()),
            Vector3::zeros()
        );
    }
    assert_eq!(rng.random::<u64>(), unchanged.random::<u64>());
    assert_eq!(
        m.evaluate_spectral(Vector3::z(), Vector3::z(), Vector3::z(), 550.0),
        (0.0)
    );
}

#[test]
fn ggx_near_tangent_and_threshold_queries_remain_finite() {
    for roughness in [SMOOTH_ROUGHNESS * 1.01, 0.3, 1.0] {
        let m = metal(roughness);
        for z in [1e-300_f64, 1e-100, 1e-12] {
            let wo = Vector3::new((1.0 - z * z).sqrt(), 0.0, z);
            for wi in [Vector3::z(), Vector3::new(-wo.x, 0.0, z)] {
                assert!(
                    rgb(&m, wi, wo, Vector3::z())
                        .iter()
                        .all(|v| v.is_finite() && *v >= 0.0)
                );
                assert!(m.pdf(wi, wo, Vector3::z()).is_finite());
            }
        }
    }
}

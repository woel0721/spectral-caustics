//! 各向同性 Trowbridge–Reitz NDF、height-correlated Smith G 与导体 Fresnel。
//! BRDF/Jacobian 参考 PBRT 4e Reflection Models（microfacet / conductor）。
//! 采样完整 NDF p(h)=D(h)|n·h|，不是 VNDF；无效反射不重采样。
use super::{to_world, unit_vector};
use nalgebra::Vector3;
use std::f64::consts::PI;

pub(super) fn d(cos_h: f64, alpha: f64) -> f64 {
    if cos_h <= 0.0 {
        return 0.0;
    }
    let a2 = alpha * alpha;
    // 此形式在 h=n 时保留 alpha²，避免 1+(alpha²-1) 的消去。
    let denominator = (1.0 - cos_h * cos_h).max(0.0) + a2 * cos_h * cos_h;
    a2 / (PI * denominator * denominator)
}

pub(super) fn g(cos_i: f64, cos_o: f64, alpha: f64) -> f64 {
    if cos_i <= 0.0 || cos_o <= 0.0 {
        return 0.0;
    }
    let a2 = alpha * alpha;
    let root = |c: f64| (a2 + (1.0 - a2) * c * c).sqrt();
    let scale = cos_i.max(cos_o);
    let (ci, co) = (cos_i / scale, cos_o / scale);
    2.0 * ci * co * scale / (ci * root(cos_o) + co * root(cos_i))
}

pub(super) fn sample_half_vector(
    normal: Vector3<f64>,
    alpha: f64,
    u: f64,
    v: f64,
) -> Option<Vector3<f64>> {
    // tan²(theta_h)=alpha²*u/(1-u), uniform azimuth; u,v are in [0,1).
    let tan2 = alpha * alpha * u / (1.0 - u);
    let cos = 1.0 / (1.0 + tan2).sqrt();
    let sin = (tan2 / (1.0 + tan2)).sqrt();
    let phi = 2.0 * PI * v;
    unit_vector(to_world(
        Vector3::new(sin * phi.cos(), sin * phi.sin(), cos),
        normal,
    )?)
}

pub(super) fn reflection_pdf(
    wi: Vector3<f64>,
    wo: Vector3<f64>,
    n: Vector3<f64>,
    alpha: f64,
) -> f64 {
    if n.dot(&wi) <= 0.0 || n.dot(&wo) <= 0.0 {
        return 0.0;
    }
    let Some(h) = unit_vector(wi + wo) else {
        return 0.0;
    };
    let cos_h = n.dot(&h).clamp(0.0, 1.0);
    let dot_o = wo.dot(&h);
    if dot_o <= 0.0 {
        return 0.0;
    }
    // dω_h/dω_i = 1/(4|wo·h|).
    d(cos_h, alpha) * cos_h / (4.0 * dot_o)
}

/// 精确非偏振导体 Fresnel：相对于空气的复折射率 eta + i*k。
/// 先按 max(eta,k,1) 缩放，避免平方/四次方溢出；不是 Schlick 或 RGB 光谱插值。
pub(super) fn fresnel(cos: f64, eta: Vector3<f64>, k: Vector3<f64>) -> Vector3<f64> {
    let cos = cos.abs().clamp(0.0, 1.0);
    eta.zip_map(&k, |eta, k| {
        if eta == 1.0 && k == 0.0 {
            return 0.0;
        }
        if cos == 0.0 {
            return 1.0;
        }
        let scale = eta.max(k).max(1.0);
        let (e, k, c) = (eta / scale, k / scale, cos / scale);
        let sin2 = (1.0 - cos * cos).max(0.0);
        let t0 = e * e - k * k - (sin2 / scale) / scale;
        let ab2 = t0.hypot(2.0 * e * k);
        let a = (0.5 * (ab2 + t0).max(0.0)).sqrt();
        let t1 = ab2 + c * c;
        let t2 = 2.0 * c * a;
        if t1 + t2 == 0.0 {
            return 1.0;
        }
        let rs = (t1 - t2) / (t1 + t2);
        let t3 = cos * cos * ab2 + (sin2 / scale).powi(2);
        let t4 = t2 * sin2;
        let rp = if t3 + t4 == 0.0 {
            rs
        } else {
            rs * (t3 - t4) / (t3 + t4)
        };
        (0.5 * (rs + rp)).clamp(0.0, 1.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::dielectric::fresnel_dielectric;
    #[test]
    fn ggx_projected_ndf_normalizes_and_has_the_known_normal_and_smith_values() {
        for alpha in [0.2, 0.5, 1.0] {
            let integral: f64 = (0..32768)
                .map(|i| {
                    let c = (i as f64 + 0.5) / 32768.0;
                    d(c, alpha) * c * 2.0 * PI / 32768.0
                })
                .sum();
            assert!((integral - 1.0).abs() < 2e-6, "{alpha}: {integral}");
            assert!((d(1.0, alpha) - 1.0 / (PI * alpha * alpha)).abs() < 1e-12);
            assert_eq!(g(1.0, 1.0, alpha), 1.0);
            let (ci, co) = (0.3_f64, 0.8_f64);
            let lambda =
                |c: f64| ((1.0 + alpha * alpha * (1.0 - c * c) / (c * c)).sqrt() - 1.0) / 2.0;
            assert!((g(ci, co, alpha) - 1.0 / (1.0 + lambda(ci) + lambda(co))).abs() < 1e-14);
        }
    }
    #[test]
    fn half_vector_inverse_cdf_and_world_rotation_match_the_ndf() {
        let n = Vector3::new(1.0, 2.0, 3.0).normalize();
        for alpha in [0.01, 0.3, 1.0] {
            for u in [0.0, 0.25, 0.5, 0.9, 0.9999] {
                let h = sample_half_vector(n, alpha, u, 0.37).unwrap();
                let c = n.dot(&h);
                let t2 = (1.0 - c * c).max(0.0) / (c * c);
                let recovered = t2 / (alpha * alpha + t2);
                assert!((recovered - u).abs() < 1e-10, "{alpha} {u}: {recovered}");
                assert!((h.norm() - 1.0).abs() < 1e-14);
            }
        }
    }
    #[test]
    fn conductor_fresnel_matches_normal_formula_and_real_index_dielectric_reference() {
        let eta = Vector3::<f64>::new(0.2, 1.5, 3.0);
        let k = Vector3::new(3.0, 2.0, 4.0);
        let expected = eta.zip_map(&k, |e, k| {
            ((e - 1.0).powi(2) + k * k) / ((e + 1.0).powi(2) + k * k)
        });
        assert!((fresnel(1.0, eta, k) - expected).norm() < 1e-14);
        for eta in [0.5, 1.0, 1.5, 3.0] {
            for cos in [0.0, 0.1, 0.3, 0.6, 0.9, 1.0] {
                let expected = fresnel_dielectric(cos, 1.0, eta).unwrap();
                assert!(
                    (fresnel(cos, Vector3::repeat(eta), Vector3::zeros())
                        - Vector3::repeat(expected))
                    .norm()
                        < 1e-12,
                    "{eta} {cos}"
                );
            }
        }
    }
    #[test]
    fn conductor_fresnel_stays_finite_for_extreme_optical_constants_and_grazing() {
        for eta in [f64::MIN_POSITIVE, 0.1, 1.0, f64::MAX] {
            for k in [0.0, f64::MIN_POSITIVE, 3.0, f64::MAX] {
                for cos in [0.0, 1e-300, 1e-12, 0.5, 1.0] {
                    let f = fresnel(cos, Vector3::repeat(eta), Vector3::repeat(k));
                    assert!(
                        f.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                        "{eta} {k} {cos}: {f}"
                    );
                }
            }
        }
    }
}

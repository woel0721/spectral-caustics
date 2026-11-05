//! 各向同性 Trowbridge–Reitz NDF、height-correlated Smith G 与导体 Fresnel。
//! BRDF/Jacobian 参考 PBRT 4e Reflection Models（microfacet / conductor）。
//! 采样完整 NDF p(h)=D(h)|n·h|，不是 VNDF；无效反射不重采样。
use super::to_world;
use super::unit_vector;
use nalgebra::Vector3;
use std::f64::consts::PI;

/// 计算 GGX 微表面法线分布，用于金属 BRDF 和半向量 PDF。
///
/// 输入：cos_h=n·h 和正粗糙参数 α；调用方提供已校验参数。输出：法线分布密度 D(h)，背半球为 0。
///
/// 公式：D=α²/[π·(1-cos_h²+α²cos_h²)²]。
pub(super) fn d(cos_h: f64, alpha: f64) -> f64 {
    if cos_h <= 0.0 {
        return 0.0;
    }
    let a2 = alpha * alpha;
    // 此形式在 h=n 时保留 alpha²，避免 1+(alpha²-1) 的消去。
    let denominator = (1.0 - cos_h * cos_h).max(0.0) + a2 * cos_h * cos_h;
    a2 / (PI * denominator * denominator)
}

/// 计算两个方向之间因微表面相互遮挡而保留下来的比例，供金属 BRDF 使用。
///
/// 输入：入射/出射方向相对法线的余弦 c_i/c_o 和粗糙参数 α；输出：高度相关 Smith 遮蔽项 G，背半球为 0。
///
/// 公式：s(c)=√(α²+(1-α²)c²)，G=2c_i c_o/[c_i s(c_o)+c_o s(c_i)]。
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

/// 按完整 GGX 法线分布生成微表面法线 h，供金属反射采样使用。
///
/// 输入：法线 normal、粗糙参数 α、两个 [0,1) 样本 u/v；输出：单位世界半向量，变换无效时 None。
///
/// 公式：tan²θ_h=α²u/(1-u)，φ=2πv，p(h)=D(h)·|n·h|。
/// 这是完整法线分布采样，不是只采可见微表面的 VNDF。
pub(super) fn sample_half_vector(
    normal: Vector3<f64>,
    alpha: f64,
    u: f64,
    v: f64,
) -> Option<Vector3<f64>> {
    // 逆变换采样：tan²θ_h=α²u/(1-u)，方位角均匀；u/v 均在 [0,1)。
    let tan2 = alpha * alpha * u / (1.0 - u);
    let cos = 1.0 / (1.0 + tan2).sqrt();
    let sin = (tan2 / (1.0 + tan2)).sqrt();
    let phi = 2.0 * PI * v;
    unit_vector(to_world(
        Vector3::new(sin * phi.cos(), sin * phi.sin(), cos),
        normal,
    )?)
}

/// 把半向量的采样密度换算成反射方向的密度，用于 GGX 采样权重和 MIS。
///
/// 输入：单位 wi/wo/n 和粗糙参数 α；输出：p(wi)（sr⁻¹），不在有效半球或半向量退化时为 0。
///
/// 公式：h=normalize(wi+wo)，p(wi)=D(h)·|n·h|/(4|wo·h|)。分母来自半向量到反射方向的 Jacobian。
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

/// 计算金属界面对非偏振光的精确 RGB 反射率，供金属 BRDF 与光滑镜面权重使用。
///
/// 输入：入射角余弦 cos，以及相对空气的 RGB 复折射率 η+iκ；调用方保证 η>0、κ≥0 且有限。
///
/// 输出：逐通道 F∈`[0,1]`；令 c=|cosθ|、s²=1-c²、a²+b²=√((η²-κ²-s²)²+4η²κ²)，
/// a=√((a²+b²+η²-κ²-s²)/2)，则 R_s=(a²+b²+c²-2ac)/(a²+b²+c²+2ac)，
/// R_p=R_s·[c²(a²+b²)+s⁴-2ac s²]/[c²(a²+b²)+s⁴+2ac s²]，F=(R_s+R_p)/2。
/// 实现先缩放参数以避免高次乘法溢出；这里的 RGB 参数不是实测光谱。
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
    use crate::optics::interface::fresnel_dielectric;
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

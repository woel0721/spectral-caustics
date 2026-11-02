//! 理想光滑介质：普通 BSDF 为零，通过 Fresnel 概率采样反射/透射事件。

use super::{BsdfSample, BsdfValue, EvaluateMode, Material, ScatterType, unit_vector};
use crate::optics::refract;
use nalgebra::Vector3;
use rand::{Rng, RngCore};

#[derive(Debug, Clone, PartialEq)]
pub struct Dielectric {
    ior: f64,
}

impl Dielectric {
    /// ior 为介质内部的无量纲折射率，必须有限且大于零。
    pub fn new(ior: f64) -> Result<Self, String> {
        if !ior.is_finite() || ior <= 0.0 {
            return Err("material.ior 必须有限且大于零".into());
        }
        Ok(Self { ior })
    }
}

impl Material for Dielectric {
    fn evaluate(
        &self,
        _wi: Vector3<f64>,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
        mode: EvaluateMode,
    ) -> BsdfValue {
        // Dirac delta 不能在镜面方向返回一个人为的有限值。
        // 与 PBRT 的理想 SpecularReflection/SpecularTransmission::f 一致。
        BsdfValue::zero(mode)
    }

    /// 采样空气与当前玻璃之间的理想镜面反射或透射，使用 radiance 约定。
    ///
    /// wo 从交点指向上一顶点，实际到达界面的射线方向是 -wo。
    /// normal 必须是玻璃朝外的几何法线，不能预先朝向 wo；内部会归一化。
    /// wo·normal > 0 时从空气进入玻璃，< 0 时从玻璃出射，并翻转折射法线。
    /// 非法方向、切线 wo 或数值计算失败返回 None；非法方向不消耗随机数。
    /// 全反射和 F=0 的确定事件不消耗随机数，其余事件使用一次均匀采样。
    /// 当前固定外部折射率为 1，不用于嵌套介质或光源正向光子通量追踪。
    fn sample(
        &self,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        rng: &mut dyn RngCore,
    ) -> Option<BsdfSample> {
        let (wo, normal) = (unit_vector(wo)?, unit_vector(normal)?);
        let cos_o = wo.dot(&normal);
        if cos_o == 0.0 {
            return None;
        }
        let (normal, eta_i, eta_t) = if cos_o > 0.0 {
            (normal, 1.0, self.ior)
        } else {
            (-normal, self.ior, 1.0)
        };
        let cos_i = wo.dot(&normal);
        let f = fresnel_dielectric(cos_i, eta_i, eta_t)?;
        // 全反射直接选反射；F=0 时直接透射，避免采样零概率事件。
        let reflection = f == 1.0 || (f > 0.0 && rng.random::<f64>() < f);
        let sample = if reflection {
            BsdfSample {
                wi: unit_vector(2.0 * cos_i * normal - wo)?,
                event: ScatterType::SpecularReflection,
                pdf: None,
                weight: Vector3::repeat(1.0), // F / p_reflection = 1。
            }
        } else {
            BsdfSample {
                wi: unit_vector(refract(-wo, normal, eta_i, eta_t)?)?,
                event: ScatterType::SpecularTransmission,
                pdf: None,
                // (1-F) / p_transmission 已抵消，保留 radiance 的折射率平方项。
                weight: Vector3::repeat((eta_i / eta_t).powi(2)),
            }
        };
        sample
            .weight
            .iter()
            .all(|v| v.is_finite() && *v > 0.0)
            .then_some(sample)
    }

    fn has_non_delta(&self) -> bool {
        false
    }
    fn has_delta(&self) -> bool {
        true
    }

    fn ior(&self) -> Option<f64> {
        Some(self.ior)
    }
}

/// 理想光滑介质对非偏振光的精确 Fresnel 能量反射率，范围为 [0, 1]。
///
/// cos_i 取绝对值并夹紧到 [0, 1]；eta_i、eta_t 分别为入射、透射介质
/// 的折射率，由调用方按传播方向指定，不随 cos_i 的符号自动交换。
/// 非有限输入或非正折射率返回 None；全反射返回 Some(1.0)。
pub fn fresnel_dielectric(cos_i: f64, eta_i: f64, eta_t: f64) -> Option<f64> {
    if !cos_i.is_finite()
        || !eta_i.is_finite()
        || eta_i <= 0.0
        || !eta_t.is_finite()
        || eta_t <= 0.0
    {
        return None;
    }
    let cos_i = cos_i.abs().min(1.0);
    // 相同介质没有界面；先处理它，避免掠射时出现 0/0。
    if eta_i == eta_t {
        return Some(0.0);
    }
    if cos_i == 0.0 {
        return Some(1.0);
    }

    // 同比缩放不改变 Snell 或 Fresnel 方程，避免大折射率的乘积/和溢出。
    let scale = eta_i.max(eta_t);
    let eta_i = eta_i / scale;
    let eta_t = eta_t / scale;
    let sin_i = (1.0 - cos_i * cos_i).sqrt();
    let eta_sin_i = eta_i * sin_i;
    // Snell: sin(theta_t) = eta_i * sin(theta_i) / eta_t。
    // 先比较再相除，避免全反射时极大的折射率比值溢出。
    if eta_sin_i >= eta_t {
        return Some(1.0);
    }
    let sin_t = eta_sin_i / eta_t;
    let cos_t = (1.0 - sin_t * sin_t).max(0.0).sqrt();
    let r_s = (eta_i * cos_i - eta_t * cos_t) / (eta_i * cos_i + eta_t * cos_t);
    let r_p = (eta_t * cos_i - eta_i * cos_t) / (eta_t * cos_i + eta_i * cos_t);
    Some((0.5 * (r_s * r_s + r_p * r_p)).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{SeedableRng, rngs::StdRng};

    fn glass_result(sample: Option<BsdfSample>) -> Option<BsdfSample> {
        let sample = sample?;
        assert!(sample.event.is_delta());
        assert_eq!(sample.pdf, None);
        Some(sample)
    }

    struct FixedRng {
        value: u64,
        draws: usize,
    }

    impl FixedRng {
        fn new(value: u64) -> Self {
            Self { value, draws: 0 }
        }
    }

    impl RngCore for FixedRng {
        fn next_u32(&mut self) -> u32 {
            self.next_u64() as u32
        }

        fn next_u64(&mut self) -> u64 {
            self.draws += 1;
            self.value
        }

        fn fill_bytes(&mut self, dest: &mut [u8]) {
            for chunk in dest.chunks_mut(8) {
                chunk.copy_from_slice(&self.next_u64().to_le_bytes()[..chunk.len()]);
            }
        }
    }

    #[test]
    fn glass_normal_incidence_samples_both_events_and_cancels_fresnel_probability() {
        let glass = Dielectric::new(1.5).unwrap();
        for (wo, transmission_weight) in [(Vector3::z(), 4.0 / 9.0), (-Vector3::z(), 9.0 / 4.0)] {
            let mut reflect_rng = FixedRng::new(0);
            let reflected =
                glass_result(glass.sample(wo * 3.0, Vector3::z() * 7.0, &mut reflect_rng)).unwrap();
            assert_eq!(reflected.event, ScatterType::SpecularReflection);
            assert!((reflected.wi - wo).norm() < 1e-14);

            assert_eq!(reflected.weight, Vector3::repeat(1.0));
            assert_eq!(reflect_rng.draws, 1);

            let mut transmit_rng = FixedRng::new(u64::MAX);
            let transmitted =
                glass_result(glass.sample(wo, Vector3::z(), &mut transmit_rng)).unwrap();
            assert_eq!(transmitted.event, ScatterType::SpecularTransmission);
            assert!((transmitted.wi + wo).norm() < 1e-14);

            assert!((transmitted.weight - Vector3::repeat(transmission_weight)).norm() < 1e-14);
            assert_eq!(transmit_rng.draws, 1);
        }
    }

    #[test]
    fn glass_oblique_directions_obey_snell_on_both_sides_in_world_space() {
        let glass = Dielectric::new(1.5).unwrap();
        for normal in [
            Vector3::z(),
            -Vector3::z(),
            Vector3::new(1.0, 2.0, 3.0).normalize(),
        ] {
            for (local_wo, expected_reflection, expected_transmission) in [
                (
                    Vector3::new(0.6, 0.0, 0.8),
                    Vector3::new(-0.6, 0.0, 0.8),
                    Vector3::new(-0.4, 0.0, -(1.0_f64 - 0.4 * 0.4).sqrt()),
                ),
                (
                    Vector3::new(0.6, 0.0, -0.8),
                    Vector3::new(-0.6, 0.0, -0.8),
                    Vector3::new(-0.9, 0.0, (1.0_f64 - 0.9 * 0.9).sqrt()),
                ),
            ] {
                let wo = super::super::to_world(local_wo, normal).unwrap();
                for (random, event, local_expected) in [
                    (0, ScatterType::SpecularReflection, expected_reflection),
                    (
                        u64::MAX,
                        ScatterType::SpecularTransmission,
                        expected_transmission,
                    ),
                ] {
                    let sample =
                        glass_result(glass.sample(wo, normal, &mut FixedRng::new(random))).unwrap();
                    let expected = super::super::to_world(local_expected, normal).unwrap();
                    assert_eq!(sample.event, event);
                    assert!((sample.wi - expected).norm() < 1e-12);
                    assert!((sample.wi.norm() - 1.0).abs() < 1e-12);
                    let same_side = sample.wi.dot(&normal) * wo.dot(&normal) > 0.0;
                    assert_eq!(same_side, event == ScatterType::SpecularReflection);
                }
            }
        }
    }

    #[test]
    fn glass_total_internal_reflection_and_equal_indices_are_deterministic() {
        let normal = Vector3::z();
        let glass = Dielectric::new(1.5).unwrap();
        let wo = Vector3::new(3.0_f64.sqrt() / 2.0, 0.0, -0.5);
        for random in [0, u64::MAX] {
            let mut rng = FixedRng::new(random);
            let sample = glass_result(glass.sample(wo, normal, &mut rng)).unwrap();
            assert_eq!(sample.event, ScatterType::SpecularReflection);

            assert_eq!(sample.weight, Vector3::repeat(1.0));
            assert!((sample.wi - Vector3::new(-wo.x, 0.0, wo.z)).norm() < 1e-12);
            assert_eq!(rng.draws, 0);
        }
        let matched = Dielectric::new(1.0).unwrap();
        for wo in [Vector3::new(0.6, 0.0, 0.8), Vector3::new(0.6, 0.0, -0.8)] {
            let mut rng = FixedRng::new(0);
            let sample = glass_result(matched.sample(wo, normal, &mut rng)).unwrap();
            assert_eq!(sample.event, ScatterType::SpecularTransmission);

            assert_eq!(sample.weight, Vector3::repeat(1.0));
            assert!((sample.wi + wo).norm() < 1e-12);
            assert_eq!(rng.draws, 0);
        }
    }

    #[test]
    fn glass_rejects_invalid_and_tangent_directions_without_consuming_randomness() {
        let glass = Dielectric::new(1.5).unwrap();
        let normal = Vector3::z();
        let mut rng = FixedRng::new(0);
        for invalid in [
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
            Vector3::repeat(f64::INFINITY),
            Vector3::repeat(f64::NEG_INFINITY),
            Vector3::repeat(f64::MAX),
            Vector3::repeat(f64::from_bits(1)),
        ] {
            assert!(glass_result(glass.sample(invalid, normal, &mut rng)).is_none());
            assert!(glass_result(glass.sample(normal, invalid, &mut rng)).is_none());
        }
        for wo in [Vector3::x(), -Vector3::x(), Vector3::y()] {
            assert!(glass_result(glass.sample(wo, normal, &mut rng)).is_none());
        }
        assert_eq!(rng.draws, 0);
    }

    #[test]
    fn glass_fresnel_frequencies_and_radiance_estimator_are_reproducible() {
        let glass = Dielectric::new(1.5).unwrap();
        let normal = Vector3::z();
        for (wo, eta_i, eta_t) in [
            (Vector3::new(0.6, 0.0, 0.8), 1.0, 1.5),
            (Vector3::new(0.6, 0.0, -0.8), 1.5, 1.0),
        ] {
            let f = fresnel_dielectric(0.8, eta_i, eta_t).unwrap();
            let mut rng = StdRng::seed_from_u64(42);
            let mut replay = rng.clone();
            let mut reflected = 0;
            let mut reflection_estimate = 0.0;
            let mut transmission_estimate = 0.0;
            for _ in 0..32768 {
                let sample = glass_result(glass.sample(wo, normal, &mut rng)).unwrap();
                let repeated = glass_result(glass.sample(wo, normal, &mut replay)).unwrap();
                assert_eq!(sample.wi, repeated.wi);
                assert_eq!(sample.event, repeated.event);

                assert_eq!(sample.weight, repeated.weight);
                match sample.event {
                    ScatterType::SpecularReflection => {
                        assert_eq!(sample.weight, Vector3::repeat(1.0));
                        reflected += 1;
                        reflection_estimate += sample.weight.x;
                    }
                    ScatterType::SpecularTransmission => {
                        assert_eq!(sample.weight, Vector3::repeat((eta_i / eta_t).powi(2)));
                        transmission_estimate += sample.weight.x;
                    }
                    _ => unreachable!(),
                }
            }
            assert!((f64::from(reflected) / 32768.0 - f).abs() < 0.006);
            assert!((reflection_estimate / 32768.0 - f).abs() < 0.006);
            let expected_transmission = (1.0 - f) * (eta_i / eta_t).powi(2);
            assert!((transmission_estimate / 32768.0 - expected_transmission).abs() < 0.015);
        }
    }

    #[test]
    fn glass_trait_sampling_preserves_direction_event_probability_and_weight() {
        let glass: Box<dyn Material> = Box::new(Dielectric::new(1.5).unwrap());
        let concrete = Dielectric::new(1.5).unwrap();
        let wo = Vector3::new(0.6, 0.0, -0.8);
        for random in [0, u64::MAX] {
            let expected =
                glass_result(concrete.sample(wo, Vector3::z(), &mut FixedRng::new(random)))
                    .unwrap();
            let sample =
                glass_result(glass.sample(wo, Vector3::z(), &mut FixedRng::new(random))).unwrap();
            assert_eq!(sample.wi, expected.wi);
            assert_eq!(sample.event, expected.event);

            assert_eq!(sample.weight, expected.weight);
            assert_eq!(
                glass
                    .sample(wo, Vector3::z(), &mut FixedRng::new(random))
                    .unwrap()
                    .wi,
                sample.wi
            );
            assert_eq!(glass.pdf(sample.wi, wo, Vector3::z()), 0.0);
        }
    }

    #[test]
    fn fresnel_matches_normal_oblique_and_brewster_incidence() {
        // 空气 -> n=1.5 玻璃：法向入射 F=((1-1.5)/(1+1.5))²=0.04。
        assert!((fresnel_dielectric(1.0, 1.0, 1.5).unwrap() - 0.04).abs() < 1e-14);
        assert!((fresnel_dielectric(1.0, 1.5, 1.0).unwrap() - 0.04).abs() < 1e-14);
        let at_45_degrees = fresnel_dielectric(std::f64::consts::FRAC_1_SQRT_2, 1.0, 1.5);
        assert!((at_45_degrees.unwrap() - 0.05023991101223594).abs() < 1e-14);
        // Brewster 角 tan(theta_i)=1.5 时 R_p=0，R_s=25/169。
        let cos_brewster = 1.0 / (1.0_f64 + 1.5 * 1.5).sqrt();
        assert!((fresnel_dielectric(cos_brewster, 1.0, 1.5).unwrap() - 25.0 / 338.0).abs() < 1e-14);
    }

    #[test]
    fn fresnel_handles_critical_angle_total_internal_reflection_and_grazing() {
        let cos_critical = (1.0_f64 - (1.0_f64 / 1.5).powi(2)).sqrt();
        let below_critical = fresnel_dielectric(cos_critical + 1e-8, 1.5, 1.0).unwrap();
        assert!((0.0..1.0).contains(&below_critical));
        assert!((fresnel_dielectric(cos_critical, 1.5, 1.0).unwrap() - 1.0).abs() < 1e-7);
        assert_eq!(fresnel_dielectric(cos_critical - 1e-8, 1.5, 1.0), Some(1.0));
        assert_eq!(fresnel_dielectric(0.5, 1.5, 1.0), Some(1.0));
        assert_eq!(fresnel_dielectric(0.0, 1.0, 1.5), Some(1.0));
        assert_eq!(fresnel_dielectric(0.0, 1.5, 1.0), Some(1.0));
    }

    #[test]
    fn fresnel_has_no_reflection_between_identical_media() {
        for ior in [f64::from_bits(1), 1.0, 1.5, f64::MAX] {
            for cos_i in [0.0, 1e-12, 0.5, 1.0] {
                assert_eq!(fresnel_dielectric(cos_i, ior, ior), Some(0.0));
            }
        }
    }

    #[test]
    fn fresnel_uses_cosine_magnitude_and_clamps_roundoff() {
        assert_eq!(
            fresnel_dielectric(-0.5, 1.0, 1.5),
            fresnel_dielectric(0.5, 1.0, 1.5)
        );
        for cos_i in [1.0 + f64::EPSILON, -1.0 - f64::EPSILON] {
            assert_eq!(
                fresnel_dielectric(cos_i, 1.0, 1.5),
                fresnel_dielectric(1.0, 1.0, 1.5)
            );
        }
    }

    #[test]
    fn fresnel_is_reciprocal_and_invariant_under_ior_scaling() {
        let cos_i = 0.6;
        let cos_t = (1.0_f64 - (1.0_f64 / 1.5).powi(2) * (1.0 - cos_i * cos_i)).sqrt();
        let expected = fresnel_dielectric(cos_i, 1.0, 1.5).unwrap();
        assert!((fresnel_dielectric(cos_t, 1.5, 1.0).unwrap() - expected).abs() < 1e-14);
        for scale in [1e-300, 1e300] {
            assert!(
                (fresnel_dielectric(cos_i, scale, 1.5 * scale).unwrap() - expected).abs() < 1e-14
            );
        }
        for (eta_i, eta_t) in [(f64::MAX, f64::from_bits(1)), (f64::from_bits(1), f64::MAX)] {
            for cos_i in [0.0, 0.5, 1.0] {
                assert_eq!(fresnel_dielectric(cos_i, eta_i, eta_t), Some(1.0));
            }
        }
    }

    #[test]
    fn fresnel_rejects_non_finite_inputs_and_nonpositive_indices() {
        for cos_i in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(fresnel_dielectric(cos_i, 1.0, 1.5), None);
        }
        for ior in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(fresnel_dielectric(0.5, ior, 1.5), None);
            assert_eq!(fresnel_dielectric(0.5, 1.0, ior), None);
        }
    }

    #[test]
    fn ideal_dielectric_preserves_ior_and_has_no_regular_bsdf_component() {
        let glass: Box<dyn Material> = Box::new(Dielectric::new(1.33).unwrap());
        assert_eq!(glass.ior(), Some(1.33));
        assert!(!glass.has_non_delta());
        for wi in [Vector3::z(), -Vector3::z(), Vector3::repeat(f64::NAN)] {
            assert_eq!(
                glass.evaluate(wi, Vector3::z(), Vector3::z(), EvaluateMode::Rgb),
                BsdfValue::Rgb(Vector3::zeros())
            );
            assert_eq!(glass.pdf(wi, Vector3::z(), Vector3::z()), 0.0);
            for wavelength_nm in [450.0, 550.0, 650.0, 0.0, -1.0, f64::NAN, f64::INFINITY] {
                assert_eq!(
                    glass.evaluate(
                        wi,
                        Vector3::z(),
                        Vector3::z(),
                        EvaluateMode::Spectral { wavelength_nm },
                    ),
                    BsdfValue::Spectral(0.0)
                );
            }
        }
    }

    #[test]
    fn dielectric_constructor_rejects_invalid_indices() {
        for ior in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Dielectric::new(ior).is_err());
        }
        assert!(Dielectric::new(1.0).is_ok());
    }
}

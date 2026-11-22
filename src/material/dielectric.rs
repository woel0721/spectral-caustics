//! 理想光滑介质：普通 BSDF 为零，通过 Fresnel 概率采样反射/透射事件。

use super::BsdfSample;
use super::Material;
use super::unit_vector;
use crate::optics::dispersion::Cauchy;
use crate::optics::dispersion::IorMode;
use nalgebra::Vector3;

use rand::RngCore;

#[derive(Debug, Clone, PartialEq)]
pub struct Dielectric {
    ior: f64,
    dispersion: Option<Cauchy>,
}

impl Dielectric {
    /// 创建无色散的理想光滑玻璃，供 RGB 或固定 IOR 传输使用。
    ///
    /// 输入：介质内部的正有限折射率 ior（无量纲）；输出：Dielectric，非法值返回 Err。
    pub fn new(ior: f64) -> Result<Self, String> {
        if !ior.is_finite() || ior <= 0.0 {
            return Err("material.ior 必须有限且大于零".into());
        }
        Ok(Self {
            ior,
            dispersion: None,
        })
    }

    /// 创建带可选色散模型的玻璃，供光子与光谱相机按波长查询折射率。
    ///
    /// 输入：常数 ior 和可选 Cauchy 模型；输出：校验后的 Dielectric，参数无效返回 Err。
    /// RGB 采样仍使用 ior；Dispersion 模式使用 n(λ)=A+B/λ_μm²。
    pub fn with_dispersion(ior: f64, dispersion: Option<Cauchy>) -> Result<Self, String> {
        let mut glass = Self::new(ior)?;
        if let Some(model) = dispersion {
            model.validate()?;
        }
        glass.dispersion = dispersion;
        Ok(glass)
    }
}

impl Material for Dielectric {
    /// 返回零 BSDF：理想玻璃只在离散的反射/透射方向贡献，需用 sample 计算，不能用有限函数值代替。
    fn evaluate_rgb(
        &self,
        _wi: Vector3<f64>,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
    ) -> Vector3<f64> {
        // Dirac delta 不能在镜面方向返回一个人为的有限值。
        // 与 PBRT 的理想 SpecularReflection/SpecularTransmission::f 一致。
        Vector3::zeros()
    }

    /// 为 RGB 相机抽取空气/玻璃界面的反射或透射事件。
    ///
    /// 输入：指向上一顶点的 wo、朝外几何法线和 RNG；实际入射方向为 -wo。
    ///
    /// 输出：BsdfSample，反射权重为 1，透射权重为 (η_i/η_t)²，PDF 为 None；非法方向或数值失败返回 None。
    /// wo·normal>0 表示空气入射，<0 表示玻璃出射；内部归一化，不允许调用方预先翻转几何法线。
    /// 按 Fresnel 以 p_r=F、p_t=1-F 选事件，所以权重已约去 F/p_r 或 (1-F)/p_t。
    /// 全反射或 F=0 不消耗 RNG，其余消耗一次；外部折射率固定为 1，不支持嵌套介质。
    fn sample(
        &self,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        rng: &mut dyn RngCore,
    ) -> Option<BsdfSample> {
        let sample = sample_interface(wo, normal, self.ior, rng)?;
        Some(BsdfSample {
            wi: sample.wi,
            weight: Vector3::repeat(sample.weight),
            pdf: sample.pdf,
            event: sample.event,
        })
    }

    /// 返回 true：此玻璃能按单个波长计算折射率并采样界面事件。
    fn supports_spectral(&self) -> bool {
        true
    }

    /// 为光谱相机采样一次玻璃界面，返回新方向和当前波长的标量 radiance 权重。
    ///
    /// 输入：wo、朝外法线、正有限波长（nm）、IOR 模式和 RNG；非法输入或采样失败返回 None。
    /// Constant 用常数 ior，Dispersion 查询 Cauchy；反射 W=1，透射 W=(η_i/η_t)²。
    fn sample_spectral(
        &self,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        wavelength_nm: f64,
        ior_mode: IorMode,
        rng: &mut dyn RngCore,
    ) -> Option<super::BsdfSample<f64>> {
        if !wavelength_nm.is_finite() || wavelength_nm <= 0.0 {
            return None;
        }
        sample_interface(
            wo,
            normal,
            self.photon_ior(Some(wavelength_nm), ior_mode)?,
            rng,
        )
    }

    /// 返回 false：理想玻璃没有连续方向上的散射，积分器不在此计算普通直接光。
    fn has_non_delta(&self) -> bool {
        false
    }
    /// 返回 true：理想玻璃只通过离散反射或透射事件继续路径。
    fn has_delta(&self) -> bool {
        true
    }

    /// 返回此玻璃保存的常数折射率 Some(ior)，用于 RGB 路径和理想介质能力查询。
    fn ior(&self) -> Option<f64> {
        Some(self.ior)
    }

    /// 按模式查询本次光子/光谱路径的折射率。
    ///
    /// 输入：可选波长（nm）和模式；输出：Some(n)，需要色散波长但缺失或计算无效时返回 None。
    /// Dispersion 且配置模型时用 n=A+B/λ_μm²，其余使用常数 ior。
    fn photon_ior(&self, wavelength_nm: Option<f64>, mode: IorMode) -> Option<f64> {
        match (mode, self.dispersion) {
            (IorMode::Dispersion, Some(model)) => model.ior(wavelength_nm?),
            _ => Some(self.ior),
        }
    }

    fn has_wavelength_dependent_ior(&self, mode: IorMode) -> bool {
        mode == IorMode::Dispersion && self.dispersion.is_some()
    }
}

/// 计算一次理想玻璃界面散射，供 RGB 和光谱采样共用几何及事件选择。
///
/// 输入：wo、朝外法线 n、玻璃折射率和 RNG；输出：标量采样记录，方向或计算无效时 None。
/// 先按 wo·n 判断内外，再以 F/1-F 抽反射/透射；反射 wi=2(wo·n)n-wo，透射按 Snell 定律。
/// 返回的 W 为 radiance 权重：反射为 1，透射为 (η_i/η_t)²；不是光子通量权重。
fn sample_interface(
    wo: Vector3<f64>,
    normal: Vector3<f64>,
    ior: f64,
    rng: &mut dyn RngCore,
) -> Option<super::BsdfSample<f64>> {
    let (wo, normal) = (unit_vector(wo)?, unit_vector(normal)?);
    let cos_o = wo.dot(&normal);
    if cos_o == 0.0 {
        return None;
    }
    let (normal, eta_i, eta_t) = if cos_o > 0.0 {
        (normal, 1.0, ior)
    } else {
        (-normal, ior, 1.0)
    };
    let interface = crate::optics::interface::sample_dielectric(-wo, normal, eta_i, eta_t, rng)?;
    let weight = interface.radiance_weight();
    if !weight.is_finite() || weight <= 0.0 {
        return None;
    }
    Some(super::BsdfSample {
        wi: unit_vector(interface.direction)?,
        event: interface.event.into(),
        pdf: None,
        weight,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::ScatterType;
    use crate::optics::interface::fresnel_dielectric;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

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
                glass.evaluate_rgb(wi, Vector3::z(), Vector3::z()),
                (Vector3::zeros())
            );
            assert_eq!(glass.pdf(wi, Vector3::z(), Vector3::z()), 0.0);
            for wavelength_nm in [450.0, 550.0, 650.0, 0.0, -1.0, f64::NAN, f64::INFINITY] {
                assert_eq!(
                    glass.evaluate_spectral(wi, Vector3::z(), Vector3::z(), wavelength_nm),
                    (0.0)
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

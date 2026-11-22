//! 单面理想镜面，反射率为线性 RGB；无普通立体角密度。
use super::BsdfSample;
use super::Material;
use super::ScatterType;
use super::unit_vector;
use nalgebra::Vector3;
use rand::RngCore;

#[derive(Debug, Clone, PartialEq)]
pub struct Mirror {
    reflectance: Vector3<f64>,
}
impl Mirror {
    /// 创建理想镜面。输入：各分量在 `[0,1]` 的有限线性 RGB 反射率；输出：Mirror，参数无效时 Err。
    pub fn new(reflectance: Vector3<f64>) -> Result<Self, String> {
        if reflectance
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("material.reflectance 的线性 RGB 分量必须有限且位于 [0, 1]".into());
        }
        Ok(Self { reflectance })
    }
}
impl Material for Mirror {
    /// 返回 true：镜面反射率可近似重建为单波长 ρ(λ)。
    fn supports_spectral(&self) -> bool {
        true
    }

    /// 为光谱路径生成确定的镜面反射。
    ///
    /// 输入：wo、法线、波长（nm）和接口要求的 RNG/IOR 模式；不消耗 RNG。
    ///
    /// 输出：镜面方向、W=ρ(λ)、pdf=None；反射率重建失败或方向无效返回 None。
    fn sample_spectral(
        &self,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        wavelength_nm: f64,
        _: crate::optics::dispersion::IorMode,
        rng: &mut dyn RngCore,
    ) -> Option<super::BsdfSample<f64>> {
        let weight = crate::spectrum::rgb_reflectance(self.reflectance, wavelength_nm)?;
        let sample = self.sample(wo, normal, rng)?;
        Some(super::BsdfSample {
            wi: sample.wi,
            weight,
            pdf: None,
            event: sample.event,
        })
    }
    /// 返回零 BSDF：理想镜面贡献属于离散方向，只能通过 sample 的反射记录计算。
    fn evaluate_rgb(&self, _: Vector3<f64>, _: Vector3<f64>, _: Vector3<f64>) -> Vector3<f64> {
        Vector3::zeros()
    }
    /// 为 RGB 路径生成唯一的镜面反射方向，不抽取随机数。
    ///
    /// 输入：wo 和表面法线 n，RNG 参数仅为统一接口；输出：wi=normalize(2(n·wo)n-wo)、W=reflectance、pdf=None。
    /// 方向非法或 wo 位于背面时返回 None。
    fn sample(
        &self,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        _: &mut dyn RngCore,
    ) -> Option<BsdfSample> {
        let (wo, n) = (unit_vector(wo)?, unit_vector(normal)?);
        let cos = n.dot(&wo);
        if cos <= 0.0 {
            return None;
        }
        Some(BsdfSample {
            wi: unit_vector(2.0 * cos * n - wo)?,
            weight: self.reflectance,
            pdf: None,
            event: ScatterType::SpecularReflection,
        })
    }
    /// 返回 false：理想镜面没有连续散射分量。
    fn has_non_delta(&self) -> bool {
        false
    }
    /// 返回 true：镜面反射使用离散事件和对应的路径预算。
    fn has_delta(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    #[test]
    fn mirror_reflects_in_world_space_with_rgb_weight_and_no_rng_or_regular_pdf() {
        let color = Vector3::new(0.2, 0.6, 0.9);
        let m = Mirror::new(color).unwrap();
        let mut rng = StdRng::seed_from_u64(42);
        let mut untouched = rng.clone();
        for n in [Vector3::z(), Vector3::new(1.0, 2.0, 3.0).normalize()] {
            let wo = super::super::to_world(Vector3::new(0.6, 0.0, 0.8), n).unwrap();
            let sample = m.sample(wo * 3.0, n * 2.0, &mut rng).unwrap();
            assert!((sample.wi - (2.0 * wo.dot(&n) * n - wo)).norm() < 1e-12);
            assert_eq!(sample.weight, color);
            assert_eq!(sample.pdf, None);
            assert_eq!(sample.event, ScatterType::SpecularReflection);
            assert_eq!(m.pdf(sample.wi, wo, n), 0.0);
            assert_eq!(m.evaluate_rgb(sample.wi, wo, n), (Vector3::zeros()));
        }
        assert_eq!(rng.random::<u64>(), untouched.random::<u64>());
    }
    #[test]
    fn mirror_validates_reflectance_and_rejects_invalid_or_backside_geometry() {
        for v in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            assert!(Mirror::new(Vector3::repeat(v)).is_err());
        }
        let m = Mirror::new(Vector3::repeat(1.0)).unwrap();
        let mut rng = StdRng::seed_from_u64(0);
        for wo in [
            -Vector3::z(),
            Vector3::x(),
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
        ] {
            assert!(m.sample(wo, Vector3::z(), &mut rng).is_none());
        }
        assert!(m.sample(Vector3::z(), Vector3::zeros(), &mut rng).is_none());
    }
}

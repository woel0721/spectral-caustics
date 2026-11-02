//! RGB 各向同性单次散射金属；缺失的微表面多次散射能量不补偿。
use super::{BsdfSample, BsdfValue, EvaluateMode, Material, ScatterType, ggx, unit_vector};
use nalgebra::Vector3;
use rand::{Rng, RngCore};

/// alpha=roughness²；极小粗糙度切换为理想镜面，避免尖峰的数值不稳定。
pub const SMOOTH_ROUGHNESS: f64 = 1e-3;

#[derive(Debug, Clone, PartialEq)]
pub struct GgxMetal {
    eta: Vector3<f64>,
    k: Vector3<f64>,
    roughness: f64,
}
impl GgxMetal {
    /// eta/k 为相对于空气的 RGB 复折射率参数，不是实测光谱数据。
    /// eta>0，k>=0，roughness∈[0,1]，所有分量必须有限。
    pub fn new(eta: Vector3<f64>, k: Vector3<f64>, roughness: f64) -> Result<Self, String> {
        if eta.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err("material.eta 的三个分量必须有限且大于零".into());
        }
        if k.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err("material.k 的三个分量必须有限且非负".into());
        }
        if !roughness.is_finite() || !(0.0..=1.0).contains(&roughness) {
            return Err("material.roughness 必须有限且位于 [0, 1]".into());
        }
        Ok(Self { eta, k, roughness })
    }
    fn smooth(&self) -> bool {
        self.roughness <= SMOOTH_ROUGHNESS
    }
    fn alpha(&self) -> f64 {
        self.roughness * self.roughness
    }
}
impl Material for GgxMetal {
    fn evaluate(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        mode: EvaluateMode,
    ) -> BsdfValue {
        // RGB eta/k 没有唯一的光谱解释；此材质不提供光谱 BSDF。
        if mode != EvaluateMode::Rgb || self.smooth() {
            return BsdfValue::zero(mode);
        }
        let (Some(wi), Some(wo), Some(n)) = (unit_vector(wi), unit_vector(wo), unit_vector(normal))
        else {
            return BsdfValue::zero(mode);
        };
        let (ci, co) = (n.dot(&wi), n.dot(&wo));
        if ci <= 0.0 || co <= 0.0 {
            return BsdfValue::zero(mode);
        }
        let Some(h) = unit_vector(wi + wo) else {
            return BsdfValue::zero(mode);
        };
        let d = ggx::d(n.dot(&h).clamp(0.0, 1.0), self.alpha());
        let g = ggx::g(ci, co, self.alpha());
        let f = ggx::fresnel(wo.dot(&h), self.eta, self.k);
        // 先约去一个余弦，避免 ci*co 在近切线方向提前下溢。
        let value = f * (d * (g / ci) / (4.0 * co));
        if value.iter().all(|v| v.is_finite()) {
            BsdfValue::Rgb(value)
        } else {
            BsdfValue::zero(mode)
        }
    }
    fn sample(
        &self,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        rng: &mut dyn RngCore,
    ) -> Option<BsdfSample> {
        let (wo, n) = (unit_vector(wo)?, unit_vector(normal)?);
        if n.dot(&wo) <= 0.0 {
            return None;
        }
        if self.smooth() {
            return Some(BsdfSample {
                wi: unit_vector(2.0 * n.dot(&wo) * n - wo)?,
                weight: ggx::fresnel(n.dot(&wo), self.eta, self.k),
                pdf: None,
                event: ScatterType::SpecularReflection,
            });
        }
        let h = ggx::sample_half_vector(n, self.alpha(), rng.random::<f64>(), rng.random::<f64>())?;
        if wo.dot(&h) <= 0.0 {
            return None;
        }
        let wi = unit_vector(2.0 * wo.dot(&h) * h - wo)?;
        if n.dot(&wi) <= 0.0 {
            return None;
        }
        let pdf = self.pdf(wi, wo, n);
        if !pdf.is_finite() || pdf <= 0.0 {
            return None;
        }
        let BsdfValue::Rgb(f) = self.evaluate(wi, wo, n, EvaluateMode::Rgb) else {
            return None;
        };
        let weight = f * (n.dot(&wi) / pdf);
        if !weight.iter().all(|v| v.is_finite() && *v >= 0.0) {
            return None;
        }
        Some(BsdfSample {
            wi,
            weight,
            pdf: Some(pdf),
            event: ScatterType::GlossyReflection,
        })
    }
    fn pdf(&self, wi: Vector3<f64>, wo: Vector3<f64>, normal: Vector3<f64>) -> f64 {
        if self.smooth() {
            return 0.0;
        }
        let (Some(wi), Some(wo), Some(n)) = (unit_vector(wi), unit_vector(wo), unit_vector(normal))
        else {
            return 0.0;
        };
        ggx::reflection_pdf(wi, wo, n, self.alpha())
    }
    fn has_non_delta(&self) -> bool {
        !self.smooth()
    }
    fn has_delta(&self) -> bool {
        self.smooth()
    }
}

#[cfg(test)]
#[path = "ggx_metal_tests.rs"]
mod tests;

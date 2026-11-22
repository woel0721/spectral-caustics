//! RGB 各向同性单次散射金属；缺失的微表面多次散射能量不补偿。
use super::BsdfSample;
use super::Material;
use super::ScatterType;
use super::ggx;
use super::unit_vector;
use nalgebra::Vector3;
use rand::Rng;
use rand::RngCore;

/// alpha=roughness²；极小粗糙度切换为理想镜面，避免尖峰的数值不稳定。
pub const SMOOTH_ROUGHNESS: f64 = 1e-3;

#[derive(Debug, Clone, PartialEq)]
pub struct GgxMetal {
    eta: Vector3<f64>,
    k: Vector3<f64>,
    roughness: f64,
}
impl GgxMetal {
    /// 创建单次散射的各向同性 GGX 金属。
    ///
    /// 输入：RGB 复折射率参数 eta/k（相对空气）和 roughness∈`[0,1]`，所有值有限且 eta>0、k≥0。
    ///
    /// 输出：GgxMetal；参数无效返回 Err。只支持 RGB，没有对应的实测光谱模型。
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
    /// 判断粗糙度是否小到应采用理想镜面；返回 roughness≤1e-3，供求值和采样选择一致的分支。
    fn smooth(&self) -> bool {
        self.roughness <= SMOOTH_ROUGHNESS
    }
    /// 把材质粗糙度转换为 GGX 分布参数，返回 α=roughness²。
    fn alpha(&self) -> f64 {
        self.roughness * self.roughness
    }
}
impl Material for GgxMetal {
    /// 计算金属将 wi 方向入射光反射到 wo 的 RGB BRDF。
    ///
    /// 输入：wi/wo 和法线；输出：f=F·D·G/(4cosθ_i cosθ_o)，单位 sr⁻¹。
    /// F 为导体 Fresnel、D 为 GGX 法线分布、G 为 Smith 遮蔽；不补偿微表面多次散射。
    /// 背面、非法数值或理想镜面分支返回零；镜面分支通过 sample 计光。
    fn evaluate_rgb(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
    ) -> Vector3<f64> {
        // RGB eta/k 没有唯一的光谱解释；此材质不提供光谱 BSDF。
        if self.smooth() {
            return Vector3::zeros();
        }
        let (Some(wi), Some(wo), Some(n)) = (unit_vector(wi), unit_vector(wo), unit_vector(normal))
        else {
            return Vector3::zeros();
        };
        let (ci, co) = (n.dot(&wi), n.dot(&wo));
        if ci <= 0.0 || co <= 0.0 {
            return Vector3::zeros();
        }
        let Some(h) = unit_vector(wi + wo) else {
            return Vector3::zeros();
        };
        let alpha = self.alpha();
        let d = ggx::d(n.dot(&h).clamp(0.0, 1.0), alpha);
        let v = ggx::visibility(ci, co, alpha);
        let f = ggx::fresnel(wo.dot(&h), self.eta, self.k);
        let value = f * (d * v);
        if value.iter().all(|v| v.is_finite()) {
            value
        } else {
            Vector3::zeros()
        }
    }
    /// 给 RGB 金属路径采样下一次反射，并提供完整权重。
    ///
    /// 输入：wo、法线和 RNG；输出：反射方向 wi、权重、PDF 与事件，无效反射返回 None 且不补采。
    /// 光滑分支 wi=2(n·wo)n-wo，W=F，pdf=None；粗糙分支先采 h，再反射，W=f·cosθ_i/p。
    fn sample(
        &self,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        rng: &mut dyn RngCore,
    ) -> Option<BsdfSample> {
        let (wo, n) = (unit_vector(wo)?, unit_vector(normal)?);
        let co = n.dot(&wo);
        if co <= 0.0 {
            return None;
        }
        if self.smooth() {
            return Some(BsdfSample {
                wi: unit_vector(2.0 * co * n - wo)?,
                weight: ggx::fresnel(co, self.eta, self.k),
                pdf: None,
                event: ScatterType::SpecularReflection,
            });
        }
        let alpha = self.alpha();
        let h_sample = ggx::sample_half_vector(n, alpha, rng.random::<f64>(), rng.random::<f64>())?;
        let dot_o_sample = wo.dot(&h_sample);
        if dot_o_sample <= 0.0 {
            return None;
        }
        let wi = unit_vector(2.0 * dot_o_sample * h_sample - wo)?;
        let ci = n.dot(&wi);
        if ci <= 0.0 {
            return None;
        }
        let h = unit_vector(wi + wo)?;
        let cos_h = n.dot(&h).clamp(0.0, 1.0);
        let dot_o = wo.dot(&h);
        if dot_o <= 0.0 {
            return None;
        }
        let d = ggx::d(cos_h, alpha);
        let pdf = d * cos_h / (4.0 * dot_o);
        if !pdf.is_finite() || pdf <= 0.0 {
            return None;
        }
        let v = ggx::visibility(ci, co, alpha);
        let weight = ggx::fresnel(dot_o, self.eta, self.k) * (d * v * (ci / pdf));
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
    /// 查询 GGX 反射方向的采样密度，供路径权重与环境 MIS 使用。
    ///
    /// 输入：wi/wo 和法线；输出：p=D(h)|n·h|/(4|wo·h|)，h=normalize(wi+wo)。
    /// 光滑 Delta 分支、非法方向或无效半球返回 0。
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
    /// 返回是否采用粗糙 GGX 分支；为 true 时可以用 BRDF/PDF 做直接光积分。
    fn has_non_delta(&self) -> bool {
        !self.smooth()
    }
    /// 返回是否采用理想镜面分支；为 true 时路径按离散反射事件推进。
    fn has_delta(&self) -> bool {
        self.smooth()
    }
}

#[cfg(test)]
#[path = "ggx_metal_tests.rs"]
mod tests;

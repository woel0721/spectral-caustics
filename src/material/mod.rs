//! 材质定义 BSDF 与完整采样权重；几何负责求交，积分器负责计光和路径推进。

pub mod dielectric;
mod ggx;
pub mod ggx_metal;
pub mod lambertian;
pub mod mirror;

use crate::config::materialconfig::MaterialConfig;
use dielectric::Dielectric;
use lambertian::Lambertian;
use nalgebra::Vector3;
use rand::RngCore;

/// BSDF 求值模式；光谱模式携带要查询的波长（nm）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EvaluateMode {
    Rgb,
    Spectral { wavelength_nm: f64 },
}

/// BSDF 数值（sr⁻¹），变体与传入的 EvaluateMode 对应。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BsdfValue {
    /// 三通道线性 RGB BSDF。
    Rgb(Vector3<f64>),
    /// 单个波长的标量 BSDF，不含 CIE 或波长积分权重。
    Spectral(f64),
}

impl BsdfValue {
    fn zero(mode: EvaluateMode) -> Self {
        match mode {
            EvaluateMode::Rgb => Self::Rgb(Vector3::zeros()),
            EvaluateMode::Spectral { .. } => Self::Spectral(0.0),
        }
    }
}

/// 采样到的散射事件，描述事件本身而非具体材质。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScatterType {
    DiffuseReflection,
    GlossyReflection,
    SpecularReflection,
    SpecularTransmission,
}
impl ScatterType {
    pub fn is_delta(self) -> bool {
        matches!(self, Self::SpecularReflection | Self::SpecularTransmission)
    }
}

/// RGB radiance 传输的一次完整 BSDF 采样。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BsdfSample {
    /// 从交点向外指的单位世界方向。
    pub wi: Vector3<f64>,
    /// 连续事件为 f * |n·wi| / p；Delta 已除离散选择概率。
    /// Dielectric 透射已包含 (eta_i/eta_t)²，调用方只乘一次。
    pub weight: Vector3<f64>,
    /// 相对于立体角的完整采样密度（含混合分量选择概率）；Delta 为 None。
    pub pdf: Option<f64>,
    pub event: ScatterType,
}

/// 可通过 `Box<dyn Material>` 保存和并行只读访问的运行时材质。
pub trait Material: Send + Sync {
    /// 按 mode 返回给定方向的 BSDF 值，单位为 sr⁻¹。
    ///
    /// wi、wo 均从交点向外指：wi 指向光源，wo 指向观察者。
    /// normal 是世界空间表面法线；调用方可按需要先将它朝向 wo。
    /// 输入方向会归一化；零向量、非有限值或模长溢出返回零。
    /// Rgb 返回 BsdfValue::Rgb；Lambertian 为 albedo / π。
    /// Spectral 返回 BsdfValue::Spectral；Lambertian 为 rho(λ) / π。
    /// wavelength_nm 必须为正有限值，否则返回光谱标量零。
    /// 此值不含入射辐亮度、CIE、余弦项、PDF、波长积分权重、曝光或 sRGB 编码。
    /// 非法输入、无效反射方向及理想 delta 材质返回对应 mode 的零值；
    /// delta 贡献通过 sample 返回的离散记录计算，XYZ 转换由积分器处理。
    fn evaluate(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        mode: EvaluateMode,
    ) -> BsdfValue;

    /// 返回已包含 BSDF、余弦和完整采样概率的 RGB throughput weight。
    /// wo 与 normal 必须有效；normal 保留朝外几何方向，介质自行判断内外。
    /// 混合材质按原始概率选择事件，不能重采样被拒绝的方向。
    fn sample(
        &self,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
        _rng: &mut dyn RngCore,
    ) -> Option<BsdfSample> {
        None
    }

    /// sample 在给定 wo 时生成 wi 的条件概率密度 p(wi | wo)，相对于立体角 dω。
    ///
    /// 单位为 sr⁻¹；输入方向会归一化。非法方向或采样范围外返回 0.0。
    /// Lambertian 正半球内为 max(normal · wi, 0) / π，不含 albedo 或 BSDF 值。
    /// 默认返回 0.0；理想 delta 材质的普通方向密度也为零，不能视为离散事件概率。
    fn pdf(&self, _wi: Vector3<f64>, _wo: Vector3<f64>, _normal: Vector3<f64>) -> f64 {
        0.0
    }

    /// 是否存在可由 evaluate/pdf 处理的连续分量；混合材质可同时返回两种能力。
    fn has_non_delta(&self) -> bool {
        true
    }
    fn has_delta(&self) -> bool {
        false
    }

    /// 方向无关的 Lambertian RGB 反射率；仅用于无方向信息的辐照度图绑定。
    /// 默认不支持该绑定，不能从单个方向的 BSDF 值推测材质类型。
    fn irradiance_reflectance(&self) -> Option<Vector3<f64>> {
        None
    }

    /// 理想双界面介质的折射率；Some 显式允许独立的玻璃光谱焦散算法。
    /// 粗糙/其他透射模型不得借此声明兼容。
    fn ior(&self) -> Option<f64> {
        None
    }
}

/// JSON 保留可序列化的 enum，运行时构建为具体材质的 trait object。
pub fn from_config(config: &MaterialConfig) -> Result<Box<dyn Material>, String> {
    match config {
        MaterialConfig::Lambertian { albedo } => {
            Ok(Box::new(Lambertian::new(Vector3::from(*albedo))?))
        }
        MaterialConfig::Dielectric { ior } => Ok(Box::new(Dielectric::new(*ior)?)),
        MaterialConfig::Mirror { reflectance } => {
            Ok(Box::new(mirror::Mirror::new(Vector3::from(*reflectance))?))
        }
        MaterialConfig::GgxMetal { eta, k, roughness } => Ok(Box::new(ggx_metal::GgxMetal::new(
            Vector3::from(*eta),
            Vector3::from(*k),
            *roughness,
        )?)),
    }
}

/// 将局部方向转换到世界空间，局部 +Z 对应归一化后的表面法线。
///
/// 围绕法线构造确定的右手正交基 (tangent, bitangent, normal)，
/// 返回 local.x * tangent + local.y * bitangent + local.z * normal。
/// 保留方向长度和局部 z 的正负；零向量仍返回零，不做平移或额外归一化。
/// 非有限局部分量、无法归一化的法线或计算溢出返回 None。
pub fn to_world(local: Vector3<f64>, normal: Vector3<f64>) -> Option<Vector3<f64>> {
    if !local.iter().all(|value| value.is_finite()) {
        return None;
    }
    let normal = unit_vector(normal)?;
    // 选择较不平行的辅助轴，避免法线接近某个坐标轴时叉积退化。
    let helper = if normal.x.abs() > normal.y.abs() {
        Vector3::y()
    } else {
        Vector3::x()
    };
    let bitangent = unit_vector(normal.cross(&helper))?;
    let tangent = bitangent.cross(&normal);
    let world = tangent * local.x + bitangent * local.y + normal * local.z;
    world.iter().all(|value| value.is_finite()).then_some(world)
}

pub(crate) fn unit_vector(vector: Vector3<f64>) -> Option<Vector3<f64>> {
    if !vector.iter().all(|value| value.is_finite()) {
        return None;
    }
    let length = vector.norm();
    if !length.is_finite() || length == 0.0 {
        return None;
    }
    Some(vector / length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_world_aligns_local_z_with_the_normal_and_preserves_zero() {
        let local = Vector3::new(1.0, -2.0, 3.0);
        assert_eq!(to_world(local, Vector3::z()), Some(local));
        assert_eq!(to_world(local, Vector3::z() * 4.0), Some(local));
        assert_eq!(
            to_world(Vector3::zeros(), Vector3::y()),
            Some(Vector3::zeros())
        );
        for normal in [
            Vector3::x(),
            Vector3::y(),
            -Vector3::z(),
            Vector3::new(1.0, 2.0, 3.0),
        ] {
            let expected = normal.normalize() * 2.0;
            assert!((to_world(Vector3::z() * 2.0, normal).unwrap() - expected).norm() < 1e-12);
            assert!(
                (to_world(Vector3::z() * 2.0, normal * 5.0).unwrap() - expected).norm() < 1e-12
            );
        }
    }

    #[test]
    fn to_world_forms_a_right_handed_orthonormal_frame_and_preserves_angles() {
        for normal in [
            Vector3::x(),
            -Vector3::x(),
            Vector3::y(),
            -Vector3::y(),
            Vector3::z(),
            -Vector3::z(),
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(1.0, 1e-15, 0.0),
            Vector3::new(0.0, 1.0, 1e-15),
        ] {
            let tangent = to_world(Vector3::x(), normal).unwrap();
            let bitangent = to_world(Vector3::y(), normal).unwrap();
            let n = normal.normalize();
            assert!((tangent.norm() - 1.0).abs() < 1e-12);
            assert!((bitangent.norm() - 1.0).abs() < 1e-12);
            assert!(tangent.dot(&bitangent).abs() < 1e-12);
            assert!(tangent.dot(&n).abs() < 1e-12);
            assert!(bitangent.dot(&n).abs() < 1e-12);
            assert!((tangent.cross(&bitangent) - n).norm() < 1e-12);
            for local in [Vector3::new(0.3, -0.4, 0.5), Vector3::new(-0.7, 0.2, -0.6)] {
                let world = to_world(local, normal).unwrap();
                assert!((world.norm() - local.norm()).abs() < 1e-12);
                assert!((world.dot(&n) - local.z).abs() < 1e-12);
                assert!((world.dot(&tangent) - local.x).abs() < 1e-12);
                assert!((world.dot(&bitangent) - local.y).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn to_world_rejects_invalid_input_and_overflow() {
        for normal in [
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
            Vector3::repeat(f64::INFINITY),
            Vector3::repeat(f64::NEG_INFINITY),
            Vector3::repeat(f64::MAX),
            Vector3::repeat(f64::from_bits(1)),
        ] {
            assert_eq!(to_world(Vector3::z(), normal), None);
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for channel in 0..3 {
                let mut local = Vector3::zeros();
                local[channel] = value;
                assert_eq!(to_world(local, Vector3::z()), None);
            }
        }
        assert_eq!(
            to_world(Vector3::repeat(f64::MAX), Vector3::repeat(1.0)),
            None
        );
    }
}

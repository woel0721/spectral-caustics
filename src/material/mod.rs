//! 描述光碰到表面后怎样反射或透射，向积分器提供散射值和下一条光线的采样记录。
//!
//! BSDF 是表面散射分布函数：给定入射/出射方向，描述光在这些方向之间的分配；
//! 只有反射时也称 BRDF。PDF 是采样的概率密度，方向 PDF 的单位为 sr⁻¹（每球面度）。
//! wi/wo 都从交点向外指，分别指向光源/上一顶点；normal 为世界空间几何法线。
//! Delta 表示理想镜面或透射的离散方向事件，不能用普通方向密度计算其贡献。
//! 采样权重 W 已包含 BSDF、余弦和概率补偿，积分器只需把路径权重 β 乘以 W。

pub mod checkerboard;
pub mod dielectric;
mod ggx;
pub mod ggx_metal;
pub mod lambertian;
pub mod mirror;

use crate::config::material::MaterialConfig;
use dielectric::Dielectric;
use lambertian::Lambertian;
use nalgebra::Vector3;
use rand::RngCore;

/// 采样到的散射事件，描述事件本身而非具体材质。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScatterType {
    DiffuseReflection,
    GlossyReflection,
    SpecularReflection,
    SpecularTransmission,
}
impl From<crate::optics::interface::DielectricEvent> for ScatterType {
    fn from(event: crate::optics::interface::DielectricEvent) -> Self {
        use crate::optics::interface::DielectricEvent;
        match event {
            DielectricEvent::Reflection => Self::SpecularReflection,
            DielectricEvent::Transmission => Self::SpecularTransmission,
        }
    }
}
impl ScatterType {
    /// 判断本次事件是否为理想镜面反射/透射；返回 true 时应使用离散事件权重，而非普通方向 PDF。
    pub fn is_delta(self) -> bool {
        matches!(self, Self::SpecularReflection | Self::SpecularTransmission)
    }
}

/// A sampled direction and fully compensated radiance weight. Delta has no solid-angle PDF.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BsdfSample<V = Vector3<f64>> {
    pub wi: Vector3<f64>,
    pub weight: V,
    pub pdf: Option<f64>,
    pub event: ScatterType,
}
pub trait Material: Send + Sync {
    fn evaluate_rgb(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
    ) -> Vector3<f64>;
    fn evaluate_spectral(
        &self,
        _wi: Vector3<f64>,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
        _wavelength_nm: f64,
    ) -> f64 {
        0.0
    }
    fn sample(
        &self,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
        _rng: &mut dyn RngCore,
    ) -> Option<BsdfSample> {
        None
    }
    fn sample_spectral(
        &self,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
        _wavelength_nm: f64,
        _ior_mode: crate::optics::dispersion::IorMode,
        _rng: &mut dyn RngCore,
    ) -> Option<BsdfSample<f64>> {
        None
    }
    fn pdf(&self, _wi: Vector3<f64>, _wo: Vector3<f64>, _normal: Vector3<f64>) -> f64 {
        0.0
    }
    fn pdf_spectral(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        _wavelength_nm: f64,
    ) -> f64 {
        self.pdf(wi, wo, normal)
    }
    fn supports_spectral(&self) -> bool {
        false
    }
    fn has_non_delta(&self) -> bool {
        true
    }
    fn has_delta(&self) -> bool {
        false
    }
    fn irradiance_reflectance(&self) -> Option<Vector3<f64>> {
        None
    }
    fn ior(&self) -> Option<f64> {
        None
    }
    fn photon_ior(
        &self,
        _wavelength_nm: Option<f64>,
        _mode: crate::optics::dispersion::IorMode,
    ) -> Option<f64> {
        self.ior()
    }
    fn has_wavelength_dependent_ior(&self, _mode: crate::optics::dispersion::IorMode) -> bool {
        false
    }
}
/// 将可序列化的材质配置构建为运行时材质，供场景绑定和并行追踪。
///
/// 输入：MaterialConfig；输出：`Box<dyn Material>`，参数无效返回 Err。
pub fn from_config(config: &MaterialConfig) -> Result<Box<dyn Material>, String> {
    match config {
        MaterialConfig::Lambertian { albedo } => {
            Ok(Box::new(Lambertian::new(Vector3::from(*albedo))?))
        }
        MaterialConfig::Dielectric { ior, dispersion } => {
            Ok(Box::new(Dielectric::with_dispersion(*ior, *dispersion)?))
        }
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

/// 把法线局部坐标系中的向量变为世界向量，供半球方向和圆盘位置采样使用。
///
/// 输入：局部向量 local 和有效法线 normal；输出：世界向量，非有限、法线退化或溢出返回 None。
///
/// 公式：world=local.x·t+local.y·b+local.z·n，(t,b,n) 为围绕单位法线构建的右手正交基。
/// 局部 +Z 对应法线方向；不平移、不额外归一化，保留向量长度及局部 z 的正负。
pub fn to_world(local: Vector3<f64>, normal: Vector3<f64>) -> Option<Vector3<f64>> {
    let normal = unit_vector(normal)?;
    to_world_unit(local, normal)
}

/// 把局部向量变换到围绕已知单位法线构建的右手正交基中，避免重复归一化法线。
pub(crate) fn to_world_unit(local: Vector3<f64>, normal: Vector3<f64>) -> Option<Vector3<f64>> {
    if !local.iter().all(|value| value.is_finite()) {
        return None;
    }
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

/// 将输入向量归一化，供材质和照明共用方向校验。
///
/// 输入：有限非零 vector；输出：vector/‖vector‖，长度为零、非有限或模长溢出返回 None。
pub(crate) fn unit_vector(vector: Vector3<f64>) -> Option<Vector3<f64>> {
    let norm_squared = vector.norm_squared();
    if !norm_squared.is_finite() || norm_squared == 0.0 {
        return None;
    }
    if (norm_squared - 1.0).abs() <= 4.0 * f64::EPSILON {
        return Some(vector);
    }
    let length = norm_squared.sqrt();
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

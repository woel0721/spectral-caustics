//! 材质定义 BSDF；几何只负责求交，积分器负责光照、余弦项和采样权重。

pub mod dielectric;
pub mod lambertian;

use crate::config::materialconfig::MaterialConfig;
use dielectric::Dielectric;
use lambertian::Lambertian;
use nalgebra::Vector3;
use rand::RngCore;

/// 可通过 `Box<dyn Material>` 保存和并行只读访问的运行时材质。
pub trait Material: Send + Sync {
    /// 返回给定方向的 BSDF 值，三个分量为线性 RGB，单位为 sr⁻¹。
    ///
    /// wi、wo 均从交点向外指：wi 指向光源，wo 指向观察者。
    /// normal 是世界空间表面法线；调用方可按需要先将它朝向 wo。
    /// 输入方向会归一化；零向量、非有限值或模长溢出返回零。
    /// 此值不含入射辐亮度、余弦项、PDF、曝光或 sRGB 编码。
    /// 理想镜面/透射的 delta 分布不能用普通数值表示，evaluate_rgb 返回零；
    /// 它们的贡献需要后续通过散射采样接口计算。
    fn evaluate_rgb(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
    ) -> Vector3<f64>;

    /// 返回给定波长的标量 BSDF 值，单位为 sr⁻¹。
    ///
    /// wavelength_nm 的单位为 nm，必须为正有限值；方向约定同 evaluate_rgb。
    /// Lambertian 仅返回 rho(λ) / π；非法输入或无效反射方向返回 0.0。
    /// 理想 delta 材质的普通求值也返回 0.0，其贡献需通过采样计算。
    /// 不含 CIE 匹配函数、光源 SPD、余弦、PDF 或波长积分权重。
    /// 光谱吞吐量和 XYZ 转换由调用方的积分器处理。
    fn evaluate_spectral(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        wavelength_nm: f64,
    ) -> f64;

    /// 给定已知的世界方向 wo，随机生成新的单位世界方向 wi。
    ///
    /// wi、wo 都从交点向外指，约定同 evaluate_rgb。
    /// rng 由调用方持有和设种子；Lambertian 使用法线正半球的余弦加权采样。
    /// 非法 wo/normal 或 wo 位于背面/切线时返回 None，且不消耗随机数。
    /// 默认没有方向采样实现；理想 Dielectric 暂沿用此行为。
    fn sample(
        &self,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
        _rng: &mut dyn RngCore,
    ) -> Option<Vector3<f64>> {
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

    /// 是否只有理想镜面/透射分量，需通过采样而非普通方向求值。
    fn is_delta(&self) -> bool {
        false
    }

    /// 介质内部折射率；非透射材质没有此参数。
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

fn unit_vector(vector: Vector3<f64>) -> Option<Vector3<f64>> {
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

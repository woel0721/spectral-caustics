//! 材质定义 BSDF；几何只负责求交，积分器负责光照、余弦项和采样权重。

pub mod dielectric;
pub mod lambertian;

use crate::config::materialconfig::MaterialConfig;
use dielectric::Dielectric;
use lambertian::Lambertian;
use nalgebra::Vector3;

/// 可通过 `Box<dyn Material>` 保存和并行只读访问的运行时材质。
pub trait Material: Send + Sync {
    /// 返回给定方向的 BSDF 值，三个分量为线性 RGB，单位为 sr⁻¹。
    ///
    /// wi、wo 均从交点向外指：wi 指向光源，wo 指向观察者。
    /// normal 是世界空间表面法线；调用方可按需要先将它朝向 wo。
    /// 输入方向会归一化；零向量、非有限值或模长溢出返回零。
    /// 此值不含入射辐亮度、余弦项、PDF、曝光或 sRGB 编码。
    /// 理想镜面/透射的 delta 分布不能用普通数值表示，evaluate 返回零；
    /// 它们的贡献需要后续通过散射采样接口计算。
    fn evaluate(&self, wi: Vector3<f64>, wo: Vector3<f64>, normal: Vector3<f64>) -> Vector3<f64>;

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

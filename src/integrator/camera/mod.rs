//! 从相机发射光线，沿表面反射/透射路径累计照明，最后输出线性 XYZ 图像。
//!
//! 路径权重 β（throughput）是到达当前交点前所有散射权重的乘积；局部贡献按 β·L 加入图像。
//! NEE 是在交点主动抽取光源位置或方向来估计直接光；MIS 将材质和环境两种采样策略组合，
//! 用各自 PDF 修正贡献，减少噪声。RR（Russian Roulette）随机截断较深路径，再补偿存活权重。
//! Direct 包含太阳/圆盘、环境、背景及允许的后续路径贡献；Caustics 只读取首个连续地面顶点的焦散图。
//! RGB 路径传输三个线性颜色分量；Hero 路径固定一个波长并传输标量，最终再做波长积分。

mod ground;
mod lighting;
pub mod path;
mod rgb;
pub mod spectral;
pub mod transport;
pub mod types;

pub use ground::{lambertian_radiance, sample_ground_irradiance};
pub use rgb::integrate_rgb;
pub use spectral::integrate_spectral;
pub use types::CameraLightingImages;

/// 将表面点沿法线略微移动，避免相机续追和阴影光线立刻再次命中自身。
///
/// 输入：世界位置 position、单位几何法线 normal 和侧别 side（+1 外侧，-1 内侧）；不在此校验。
///
/// 输出：新起点 P′=P+side·10ε·normal，ε=1e-6 m；只返回坐标，不构造 Ray。
fn offset_ray_origin(
    position: nalgebra::Vector3<f64>,
    normal: nalgebra::Vector3<f64>,
    side: f64,
) -> nalgebra::Vector3<f64> {
    position + normal * (side * 10.0 * crate::geometry::ray::INTERSECTION_EPSILON)
}

#[cfg(test)]
pub(crate) mod diffuse_tests;
#[cfg(test)]
mod environment_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod bounce_tests;
#[cfg(test)]
mod path_tests;

#[cfg(test)]
pub(crate) mod test_support;

pub use types::CameraPathState;

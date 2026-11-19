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

/// 有介质边界时从实际交点出发，由几何求交的 epsilon 排除自交。
/// 这也保护普通表面发出的射线，法线偏移可能越过紧邻的玻璃。
/// 其余场景沿单位法线移动 side·10ε，side 为 +1 或 -1。
fn surface_ray_origin(
    scene: &crate::scene::Scene,
    position: nalgebra::Vector3<f64>,
    normal: nalgebra::Vector3<f64>,
    side: f64,
) -> nalgebra::Vector3<f64> {
    if scene.has_medium_interface() {
        position
    } else {
        position + normal * (side * 10.0 * crate::geometry::ray::INTERSECTION_EPSILON)
    }
}

#[cfg(test)]
pub(crate) mod tests;

pub use types::CameraPathState;

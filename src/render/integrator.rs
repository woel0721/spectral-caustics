//! 相机直接光照、有界玻璃路径与 E2 有限深度 Lambertian 路径。
//! 对外保留原入口路径，具体实现按职责拆入私有子模块。

mod camera;
mod diffuse;
mod direct;
mod environment;
mod ground;
mod path;
mod rng;
mod types;

pub use camera::{
    render_camera_glass, render_camera_glass_components, render_camera_glass_with_ground,
    render_camera_glass_with_lighting, render_camera_lit,
};
pub use ground::{lambertian_radiance, sample_ground_irradiance};
pub use types::{CameraLightingImages, GroundLightingComponent};

/// 沿调用方选定的几何法线侧偏移；不改变法线、不做校验或构造 Ray。
/// side 为 +1/-1；保留玻璃原有乘法顺序，非 delta/阴影使用 +1。
fn offset_ray_origin(
    position: nalgebra::Vector3<f64>,
    normal: nalgebra::Vector3<f64>,
    side: f64,
) -> nalgebra::Vector3<f64> {
    position + normal * (side * 10.0 * crate::geometry::ray::INTERSECTION_EPSILON)
}

#[cfg(test)]
mod diffuse_tests;
#[cfg(test)]
mod environment_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod e2_tests;

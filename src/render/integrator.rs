//! 独立 RGB / Hero Wavelength 相机路径、MIS 环境照明与 XYZ 输出。

mod camera;
pub mod debug;
mod diffuse;
mod direct;
mod environment;
mod ground;
mod path;
mod rng;
pub mod spectral;
mod types;

pub(crate) use camera::render_camera_glass_components_impl;
pub use camera::{render_camera_glass, render_camera_glass_components};
pub use ground::{lambertian_radiance, sample_ground_irradiance};
pub use spectral::{
    SpectralPathState, render_spectral_camera, render_spectral_camera_with_packet,
    trace_spectral_camera_path,
};
pub use types::CameraLightingImages;

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
mod bounce_tests;
#[cfg(test)]
mod path_tests;

#[cfg(test)]
mod test_support;

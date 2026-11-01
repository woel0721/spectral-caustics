//! 相机直接光照、有界玻璃路径与 E1 单次漫反射间接光。
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

#[cfg(test)]
mod diffuse_tests;
#[cfg(test)]
mod environment_tests;
#[cfg(test)]
mod tests;

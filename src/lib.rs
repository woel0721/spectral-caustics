//! 配置驱动的渲染器模块；main 从 JSON 构建运行时 RenderJob、Renderer 并输出图片。

pub mod camera;
pub mod caustics;
pub mod color;
pub mod config;
pub mod environment;
pub mod film;
pub mod geometry;
pub mod integrator;
pub mod light;
pub mod material;
pub mod optics;
pub mod output;
pub mod renderer;
pub mod sampling;
pub mod scene;
pub mod spectrum;

#[cfg(feature = "diagnostics")]
pub mod diagnostics;

//! 配置驱动的渲染器模块；main 从 JSON 构建运行时 Scene、Render 并输出图片。

pub mod caustics;
pub mod config;
pub mod framebuffer;
pub mod ground;
pub mod hit;
pub mod iormode;
pub mod object;
pub mod prism;
pub mod ray;
pub mod render;
pub mod rendersetting;
pub mod scene;
pub mod sceneconfig;
pub mod spectrum;
pub mod triangle;
pub mod utils;

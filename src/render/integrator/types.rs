//! 相机输出分量、单条路径的 RGB 状态与随机流借用。

use crate::{geometry::ray::Ray, render::framebuffer::Framebuffer};
use nalgebra::Vector3;

/// 同一批相机路径的三个结果，均为未曝光的线性 XYZ 辐亮度。
pub struct CameraLightingImages {
    pub direct: Framebuffer,
    pub caustics: Framebuffer,
    pub combined: Framebuffer,
}

/// 单条路径或像素均值的线性 RGB 分量，共享玻璃路径和吞吐量。
#[derive(Default)]
pub(super) struct CameraPathComponents {
    pub(super) direct: Vector3<f64>,
    pub(super) caustics: Vector3<f64>,
}

pub(super) struct CameraPathRngs<'a> {
    pub(super) camera: &'a mut dyn rand::RngCore,
    pub(super) environment: &'a mut dyn rand::RngCore,
    /// 供漫反射 BSDF、后续顶点环境与 RR 使用；深度 0 时不消费。
    pub(super) diffuse: &'a mut dyn rand::RngCore,
}

pub(super) struct DiffuseRay {
    pub(super) ray: Ray,
    pub(super) throughput: Vector3<f64>,
}

/// 一条有界相机路径；throughput 为线性 RGB，两个事件计数彼此独立。
pub(super) struct CameraPathState {
    pub(super) ray: Ray,
    pub(super) throughput: Vector3<f64>,
    pub(super) radiance: CameraPathComponents,
    pub(super) diffuse_bounces: usize,
    pub(super) glass_events: usize,
    pub(super) max_diffuse_bounces: usize,
}

impl CameraPathState {
    pub(super) fn new(ray: Ray, max_diffuse_bounces: usize) -> Self {
        Self {
            ray,
            throughput: Vector3::repeat(1.0),
            radiance: CameraPathComponents::default(),
            diffuse_bounces: 0,
            glass_events: 0,
            max_diffuse_bounces,
        }
    }
}

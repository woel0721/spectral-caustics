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
    /// 像素抖动与首次漫反射之前的玻璃事件。
    pub(super) camera: &'a mut dyn rand::RngCore,
    pub(super) environment: &'a mut dyn rand::RngCore,
    /// 漫反射 BSDF、后续玻璃/环境与 RR；深度 0 时不消费。
    pub(super) diffuse: &'a mut dyn rand::RngCore,
}

pub(super) struct DiffuseRay {
    pub(super) ray: Ray,
    pub(super) throughput: Vector3<f64>,
}

/// 最近接受的路径事件；Camera 只用于尚未发生表面散射的主光线。
/// 它不代表整条路径历史，是否经历过漫反射仍由 diffuse_bounces 判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LastScatter {
    Camera,
    Diffuse,
    Delta,
}

/// 一条有界相机路径；throughput 为线性 RGB，两个事件计数彼此独立。
pub(super) struct CameraPathState {
    pub(super) ray: Ray,
    pub(super) last_scatter: LastScatter,
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
            last_scatter: LastScatter::Camera,
            throughput: Vector3::repeat(1.0),
            radiance: CameraPathComponents::default(),
            diffuse_bounces: 0,
            glass_events: 0,
            max_diffuse_bounces,
        }
    }

    /// 共用吞吐量和事件更新；返回 false 表示零吞吐量终止，不再发出射线。
    /// 调用者负责采样、构造下一条 Ray，并在成功后统一写回 ray。
    pub(super) fn apply_scatter(
        &mut self,
        scatter: LastScatter,
        weight: Vector3<f64>,
    ) -> Result<bool, &'static str> {
        self.throughput = self.throughput.component_mul(&weight);
        match scatter {
            LastScatter::Delta => {
                // 玻璃计数记录已采样事件，零权重也计一次；不消费漫反射预算。
                self.glass_events += 1;
                self.last_scatter = scatter;
                if !self.throughput.iter().all(|value| value.is_finite()) {
                    return Err("玻璃路径吞吐量计算溢出");
                }
            }
            LastScatter::Diffuse => {}
            LastScatter::Camera => unreachable!("主光线发射不是材质散射"),
        }
        if self.throughput == Vector3::zeros() {
            return Ok(false);
        }
        if scatter == LastScatter::Diffuse {
            // 漫反射计数记录非零吞吐量的续追连接，不消费玻璃预算。
            self.diffuse_bounces += 1;
            self.last_scatter = scatter;
        }
        Ok(true)
    }
}

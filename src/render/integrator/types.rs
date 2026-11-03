//! 相机输出分量、单条路径的 RGB 状态与随机流借用。

use crate::{geometry::ray::Ray, material::ScatterType, render::framebuffer::Framebuffer};
use nalgebra::Vector3;

/// 同一批相机路径的三个结果，均为未曝光的线性 XYZ 辐亮度。
pub struct CameraLightingImages {
    pub direct: Framebuffer,
    pub caustics: Framebuffer,
    pub combined: Framebuffer,
    /// 路径事件统计，不是 XYZ 辐亮度；仅 RenderSetting::debug_path 开启时分配。
    pub path_debug: Option<super::debug::PathDebugBuffer>,
}

/// 单条路径或像素均值的线性 RGB 分量，共享玻璃路径和吞吐量。
#[derive(Default)]
pub(super) struct CameraPathComponents {
    pub(super) direct: Vector3<f64>,
    pub(super) caustics: Vector3<f64>,
}

pub(super) struct CameraPathRngs<'a> {
    /// 像素抖动与首次连续着色顶点之前的 Delta 事件。
    pub(super) camera: &'a mut dyn rand::RngCore,
    pub(super) environment: &'a mut dyn rand::RngCore,
    /// 连续 BSDF、后续 Delta/环境与 RR；混合材质也使用此流。
    pub(super) diffuse: &'a mut dyn rand::RngCore,
}

pub(super) struct ScatterRay {
    pub(super) ray: Ray,
    pub(super) throughput: Vector3<f64>,
    pub(super) event: ScatterType,
}

/// RGB 路径状态；预算描述连续/Delta 事件，不依赖材质类型。
pub(super) struct CameraPathState {
    pub(super) ray: Ray,
    pub(super) last_scatter: Option<ScatterType>,
    pub(super) throughput: Vector3<f64>,
    pub(super) radiance: CameraPathComponents,
    pub(super) path_depth: usize,
    pub(super) has_non_delta_vertex: bool,
    pub(super) non_delta_bounces: usize,
    pub(super) max_non_delta_bounces: usize,
}
impl CameraPathState {
    pub(super) fn new(ray: Ray, max_non_delta_bounces: usize) -> Self {
        Self {
            ray,
            last_scatter: None,
            throughput: Vector3::repeat(1.0),
            radiance: CameraPathComponents::default(),
            path_depth: 0,
            has_non_delta_vertex: false,
            non_delta_bounces: 0,
            max_non_delta_bounces,
        }
    }
    pub(super) fn delta_events(&self) -> usize {
        self.path_depth - self.non_delta_bounces
    }

    pub(super) fn apply_scatter(
        &mut self,
        scatter: ScatterType,
        weight: Vector3<f64>,
    ) -> Result<bool, &'static str> {
        if !weight.iter().all(|v| v.is_finite() && *v >= 0.0) {
            return Err("BSDF 采样权重必须有限且非负");
        }
        self.throughput = self.throughput.component_mul(&weight);
        if !self.throughput.iter().all(|v| v.is_finite()) {
            return Err("路径吞吐量计算溢出");
        }
        if scatter.is_delta() || self.throughput != Vector3::zeros() {
            self.path_depth += 1;
            self.last_scatter = Some(scatter);
            if !scatter.is_delta() {
                self.non_delta_bounces += 1;
            }
        }
        Ok(self.throughput != Vector3::zeros())
    }
}

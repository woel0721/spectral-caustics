//! 平行太阳光、地面接收区域及场景背景查询。

use super::Scene;
use crate::geometry::ray::PARALLEL_EPSILON;
use nalgebra::{Vector2, Vector3};
// 保留原 scene::Environment 的公共访问路径。
pub use crate::environment::Environment;

/// 平行太阳光；水平窗口仅用于有限数量的光子采样，不裁切解析直射光。
pub struct ParallelLight {
    pub height_m: f64,
    pub direction: Vector3<f64>,
    pub aperture_min_m: Vector2<f64>,
    pub aperture_max_m: Vector2<f64>,
    /// 穿过水平窗口的相对通量 Phi，D65 积分以 Y=1 归一化，无绝对瓦特标定。
    /// 水平面未遮挡辐照度为 Phi/A；垂直于光束的辐照度为 Phi/(A*|direction.z|)。
    pub total_flux: f64,
}

impl ParallelLight {
    /// 阶段 14 的太阳光约定：(指向太阳的单位方向，光束法向相对辐照度)。
    /// 水平发射面要求光束向下；水平/近水平/向上方向无法表示此约定，返回 None。
    pub fn sunlight(&self) -> Option<(Vector3<f64>, f64)> {
        let length = self.direction.norm();
        let span = self.aperture_max_m - self.aperture_min_m;
        let area = span.x * span.y;
        if !self.height_m.is_finite()
            || !length.is_finite()
            || length == 0.0
            || self
                .aperture_min_m
                .iter()
                .chain(self.aperture_max_m.iter())
                .any(|v| !v.is_finite())
            || span.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || !area.is_finite()
            || area <= 0.0
            || !self.total_flux.is_finite()
            || self.total_flux < 0.0
        {
            return None;
        }
        let wi = -self.direction / length;
        if wi.z <= PARALLEL_EPSILON {
            return None;
        }
        let irradiance = (self.total_flux / area) / wi.z;
        irradiance.is_finite().then_some((wi, irradiance))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GroundReceiver {
    pub min_m: Vector2<f64>,
    pub max_m: Vector2<f64>,
}

impl GroundReceiver {
    pub fn area(&self) -> f64 {
        let span = self.max_m - self.min_m;
        span.x * span.y
    }
}

impl Scene {
    /// 使用当前射线方向查询天空；关闭天空时回退到旧常量背景。
    pub fn background_radiance(&self, direction: Vector3<f64>) -> Option<Vector3<f64>> {
        match &self.environment {
            Some(environment) => environment.radiance(direction),
            None => self
                .background_color
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)
                .then_some(self.background_color),
        }
    }
}

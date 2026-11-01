//! 程序化地面棋盘格及其单次反射率调制。

use super::Scene;
use crate::{config::sceneconfig::CheckerboardConfig, geometry::hit::Hit};
use nalgebra::Vector3;

/// 世界 x/y 坐标中的程序化灰度棋盘格，输出地面 albedo 的乘数。
#[derive(Debug, Clone, Copy)]
pub struct Checkerboard {
    cell_size_m: f64,
    even_scale: f64,
    odd_scale: f64,
}

impl TryFrom<&CheckerboardConfig> for Checkerboard {
    type Error = String;

    fn try_from(config: &CheckerboardConfig) -> Result<Self, Self::Error> {
        config.validate()?;
        Ok(Self {
            cell_size_m: config.cell_size_m,
            even_scale: config.even_scale,
            odd_scale: config.odd_scale,
        })
    }
}

impl Checkerboard {
    /// 每格为 [i*s,(i+1)*s)，负坐标同样向下取整；不使用会饱和的整数转换。
    /// 拒绝非法坐标、除法溢出或超出 f64 整数奇偶可分辨范围的格子。
    pub fn scale_at(&self, position: Vector3<f64>) -> Option<f64> {
        if !position.iter().all(|value| value.is_finite()) {
            return None;
        }
        let x = (position.x / self.cell_size_m).floor();
        let y = (position.y / self.cell_size_m).floor();
        const MAX_EXACT_CELL: f64 = (1_u64 << 53) as f64;
        if !x.is_finite()
            || !y.is_finite()
            || x.abs() >= MAX_EXACT_CELL
            || y.abs() >= MAX_EXACT_CELL
        {
            return None;
        }
        Some(if x.rem_euclid(2.0) == y.rem_euclid(2.0) {
            self.even_scale
        } else {
            self.odd_scale
        })
    }
}

impl Scene {
    /// 在相机的漫反射终点调用；反射率调制与光照线性等价，不能写回入射 E 图。
    pub fn ground_reflectance_scale(&self, hit: &Hit) -> Option<f64> {
        if self.ground_id.is_some()
            && hit.object_id == self.ground_id
            && let Some(checkerboard) = &self.ground_checkerboard
        {
            return checkerboard.scale_at(hit.coords);
        }
        Some(1.0)
    }
}

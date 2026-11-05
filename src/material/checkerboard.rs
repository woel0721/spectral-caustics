//! 程序化地面棋盘格及其单次反射率调制。

use crate::config::RenderJob;
use crate::config::scene::CheckerboardConfig;
use crate::geometry::hit::Hit;
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

    /// 将棋盘格配置变为运行时纹理。输入：CheckerboardConfig；输出：Checkerboard，边长或倍率无效时 Err。
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
    /// 根据世界位置判断落在哪种棋盘格，供地面反射率调制。
    ///
    /// 输入：世界坐标 position（米）；输出：偶/奇格的反射率倍率，无效坐标或格子编号超出精度时 None。
    ///
    /// 公式：i=floor(x/s)，j=floor(y/s)，s 为格子边长；i/j 奇偶相同时选 even_scale，否则 odd_scale。
    /// 负坐标同样向下取整，每格为 [i·s,(i+1)·s)。
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

impl RenderJob {
    /// 为当前交点取得地面纹理倍率，供着色和路径权重共用。
    ///
    /// 输入：Hit；输出：地面棋盘格倍率，非地面或未开纹理返回 Some(1)，坐标无效时 None。
    /// 倍率只作用一次于反射率，不写回表示入射辐照度的焦散图。
    pub fn ground_reflectance_scale(&self, hit: &Hit) -> Option<f64> {
        if self.photon.ground_id.is_some()
            && Some(hit.object_id) == self.photon.ground_id
            && let Some(checkerboard) = &self.ground_checkerboard
        {
            return checkerboard.scale_at(hit.coords);
        }
        Some(1.0)
    }
}

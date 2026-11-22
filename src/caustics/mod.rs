//! Irradiance data and receiver layout; no photon transport lives here.
pub mod map;
use crate::film::Framebuffer;
pub use map::{SpectralCausticsMap, ground_to_pixel};
use nalgebra::{Vector2, Vector3};
use std::error::Error;
pub struct SpectralLighting {
    pub direct: Option<Framebuffer>,
    pub caustics: Framebuffer,
    /// 生成图时的坐标范围，只限制焦散数据查询。
    pub receiver: GroundReceiver,
    /// 指向太阳的单位向量；圆盘时解析太阳方向为 +Z，且太阳强度为零。
    pub light_wi: Vector3<f64>,
    /// 平行光束法向辐照度，使用与光子相同的 D65/波长积分和相对通量标尺；圆盘为零。
    pub sun_irradiance_xyz: Vector3<f64>,
}

impl SpectralLighting {
    /// 把光源端的直接落地与焦散两张图相加，供地面总辐照度预览。
    ///
    /// 输入：self.direct/caustics；输出：E_total=E_direct+E_caustics 的新线性 XYZ 图，尺寸/颜色无效返回 Err。
    /// 该预览已含直射，不能再与解析太阳直射重复叠加。
    pub fn combined(&self) -> Result<Framebuffer, Box<dyn Error + Send + Sync>> {
        let a = self
            .direct
            .as_ref()
            .ok_or("direct photon preview was not requested")?;
        let b = &self.caustics;
        if a.width != b.width || a.height != b.height {
            return Err("拆分地面图的尺寸不匹配".into());
        }
        let mut result = Framebuffer::new(a.width, a.height)?;
        for (index, (direct, caustics)) in a.pixels.iter().zip(&b.pixels).enumerate() {
            if direct
                .iter()
                .chain(caustics.iter())
                .any(|v| !v.is_finite() || *v < 0.0)
                || !result.set(index % a.width, index / a.width, direct + caustics)
            {
                return Err("拆分地面 XYZ 合并失败".into());
            }
        }
        Ok(result)
    }
}

// 按直接落地/经选中介质落地分别累积，合并入口直接相加。

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundReceiver {
    pub min_m: Vector2<f64>,
    pub max_m: Vector2<f64>,
}

impl GroundReceiver {
    /// 读取接收区 XY 边界并返回面积 A=(max.x-min.x)(max.y-min.y)，单位 m²；调用方负责先校验边界。
    pub fn area(&self) -> f64 {
        let span = self.max_m - self.min_m;
        span.x * span.y
    }

    /// 校验矩形边界与正有限面积，返回 `(span, area)`。
    pub(crate) fn checked_area(&self) -> Result<(Vector2<f64>, f64), &'static str> {
        let span = self.max_m - self.min_m;
        let area = span.x * span.y;
        if self
            .min_m
            .iter()
            .chain(self.max_m.iter())
            .any(|v| !v.is_finite())
            || span.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || !area.is_finite()
            || area <= 0.0
        {
            return Err("invalid photon receiver bounds");
        }
        Ok((span, area))
    }
}

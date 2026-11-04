//! 有限圆盘与平行太阳的统一入口；光谱密度由照明/光子积分层单独施加。

mod disk;
pub use disk::{DiskAreaLight, DiskDirectSample, DiskEmissionSample, DiskPositionSample};

use crate::{geometry::ray::Ray, scene::ParallelLight};
use nalgebra::{Vector2, Vector3};

#[derive(Debug, Clone)]
pub enum Light {
    Parallel(ParallelLight),
    DiskArea(DiskAreaLight),
}

impl Light {
    /// 圆盘与 Hero 直射共用连续 D65；波长节点只负责求积，不能改变发射光谱。
    /// 旧平行光保留原离散节点 Y=1 归一化，以保持既有流程和测试。
    pub(crate) fn spectral_normalization(&self, samples: &[(f64, f64)]) -> Option<f64> {
        match self {
            Self::Parallel(_) => crate::optics::spectrum::d65_normalization(samples),
            Self::DiskArea(_) => Some(crate::optics::emission::d65_normalization_continuous()),
        }
    }
    pub fn as_parallel(&self) -> Option<&ParallelLight> {
        match self {
            Self::Parallel(light) => Some(light),
            Self::DiskArea(_) => None,
        }
    }
    pub fn as_parallel_mut(&mut self) -> Option<&mut ParallelLight> {
        match self {
            Self::Parallel(light) => Some(light),
            Self::DiskArea(_) => None,
        }
    }
    pub fn as_disk(&self) -> Option<&DiskAreaLight> {
        match self {
            Self::DiskArea(light) => Some(light),
            Self::Parallel(_) => None,
        }
    }
    pub fn total_flux(&self) -> f64 {
        match self {
            Self::Parallel(light) => light.total_flux,
            Self::DiskArea(light) => light.total_flux,
        }
    }
    pub fn sunlight(&self) -> Option<(Vector3<f64>, f64)> {
        self.as_parallel()?.sunlight()
    }
    pub fn validate_emission(&self) -> Result<(), String> {
        match self {
            Self::Parallel(light) => light
                .sunlight()
                .map(|_| ())
                .ok_or("平行光子发射要求有效的向下方向、窗口与通量".into()),
            Self::DiskArea(light) => light.validate(),
        }
    }
    /// 平行光的解析项；圆盘只有有限面积 NEE，不产生解析太阳项。
    pub fn analytic_sun(&self) -> Result<Option<(Vector3<f64>, f64)>, String> {
        self.validate_emission()?;
        Ok(self.sunlight())
    }
    /// RGB 单图沿用原平行光 Φ/A 约定（含向上/水平测试）；圆盘走独立 NEE。
    pub(crate) fn camera_parameters(&self) -> Result<(Vector3<f64>, f64), String> {
        match self {
            Self::Parallel(light) => {
                let length = light.direction.norm();
                let span = light.aperture_max_m - light.aperture_min_m;
                let area = span.x * span.y;
                let strength = light.total_flux / area;
                if !length.is_finite()
                    || length == 0.0
                    || span.iter().any(|v| !v.is_finite() || *v <= 0.0)
                    || !area.is_finite()
                    || area <= 0.0
                    || !strength.is_finite()
                    || light.total_flux < 0.0
                {
                    return Err("平行光方向、孔径或通量无效".into());
                }
                Ok((-light.direction / length, strength))
            }
            Self::DiskArea(light) => {
                light.validate()?;
                Ok((Vector3::z(), 0.0))
            }
        }
    }
    /// 匹配光源的发射采样；两类均携带 Φ/N，无波长积分或额外 PDF 权重。
    pub fn emission_ray(&self, position_u: [f64; 2], direction_u: [f64; 2]) -> Option<Ray> {
        match self {
            Self::Parallel(light) => {
                let span = light.aperture_max_m - light.aperture_min_m;
                let p = light.aperture_min_m
                    + Vector2::new(position_u[0] * span.x, position_u[1] * span.y);
                Ray::new(Vector3::new(p.x, p.y, light.height_m), light.direction)
            }
            Self::DiskArea(light) => Some(light.sample_emission(position_u, direction_u)?.ray),
        }
    }
    pub(crate) fn emission_dimensions(&self) -> usize {
        if self.as_disk().is_some() { 4 } else { 2 }
    }
}

impl From<ParallelLight> for Light {
    fn from(value: ParallelLight) -> Self {
        Self::Parallel(value)
    }
}
impl From<DiskAreaLight> for Light {
    fn from(value: DiskAreaLight) -> Self {
        Self::DiskArea(value)
    }
}

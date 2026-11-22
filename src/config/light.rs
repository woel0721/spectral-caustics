//! 显式带 `type` 标签的光源配置（`parallel` 或 `disk_area`）。

use super::scene::validate_bounds;
use crate::geometry::ray::Ray;
use crate::light::DiskAreaLight;
use crate::light::Light;
use crate::light::ParallelLight;
use nalgebra::Vector2;
use nalgebra::Vector3;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LightConfig {
    Parallel(ParallelLightConfig),
    DiskArea(DiskAreaLightConfig),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParallelLightConfig {
    /// 平行光从水平发射孔径出发，长度单位为米。
    pub height_m: f64,
    /// 原始方向向量，由运行时光线构造器归一化。
    pub direction: [f64; 3],
    pub aperture_min_m: [f64; 2],
    pub aperture_max_m: [f64; 2],
    /// 穿过水平发射窗口的相对通量 Phi；默认 8/16=0.5 为水平面辐照度。
    /// 光束法向辐照度为 Phi/(窗口面积*|归一化 direction.z|)，无瓦特标定。
    pub total_flux: f64,
}

impl Default for ParallelLightConfig {
    /// 返回内置平行光配置：发射高度 3 m、方向 [0.45,0,-1]、4×4 m 窗口及相对通量 8。
    fn default() -> Self {
        Self {
            height_m: 3.0,
            direction: [0.45, 0.0, -1.0],
            aperture_min_m: [-2.0; 2],
            aperture_max_m: [2.0; 2],
            total_flux: 8.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskAreaLightConfig {
    pub center: [f64; 3],
    pub normal: [f64; 3],
    pub radius: f64,
    pub total_flux: f64,
}

impl Default for LightConfig {
    /// 返回内置平行光配置，供缺省场景构建使用。
    fn default() -> Self {
        Self::Parallel(ParallelLightConfig::default())
    }
}
impl From<ParallelLightConfig> for LightConfig {
    /// 把平行光配置包装为 LightConfig::Parallel，不额外校验。
    fn from(value: ParallelLightConfig) -> Self {
        Self::Parallel(value)
    }
}
impl From<DiskAreaLightConfig> for LightConfig {
    /// 把圆盘配置包装为 LightConfig::DiskArea，不额外校验。
    fn from(value: DiskAreaLightConfig) -> Self {
        Self::DiskArea(value)
    }
}
impl LightConfig {
    /// 读取平行光配置；当前类型为 Parallel 时返回只读引用，否则 None。
    pub fn as_parallel(&self) -> Option<&ParallelLightConfig> {
        match self {
            Self::Parallel(light) => Some(light),
            Self::DiskArea(_) => None,
        }
    }
    /// 修改平行光配置；当前类型为 Parallel 时返回可变引用，否则 None。
    pub fn as_parallel_mut(&mut self) -> Option<&mut ParallelLightConfig> {
        match self {
            Self::Parallel(light) => Some(light),
            Self::DiskArea(_) => None,
        }
    }
    /// 检查当前类型的光源参数，供场景配置加载使用。
    ///
    /// 输入：self；输出：有效时 Ok(())，平行光窗口/方向/通量或圆盘几何无效时 Err。
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Parallel(light) => {
                let norm_squared: f64 = light.direction.iter().map(|v| v * v).sum();
                if !light.height_m.is_finite()
                    || !norm_squared.is_finite()
                    || norm_squared <= 0.0
                    || !light.total_flux.is_finite()
                    || light.total_flux < 0.0
                {
                    return Err(
                        "scene.light 的高度须有限、方向须有限且非零、总通量须有限且非负".into(),
                    );
                }
                validate_bounds(light.aperture_min_m, light.aperture_max_m)
                    .map_err(|message| format!("scene.light 发射孔径：{message}"))?;
                let parallel = ParallelLight {
                    height_m: light.height_m,
                    direction: light.direction.into(),
                    aperture_min_m: Vector2::from(light.aperture_min_m),
                    aperture_max_m: Vector2::from(light.aperture_max_m),
                    total_flux: light.total_flux,
                };
                parallel
                    .sunlight()
                    .map(|_| ())
                    .ok_or_else(|| "scene.light 平行光要求有效的向下方向、窗口与有限辐照度".into())
            }
            Self::DiskArea(light) => DiskAreaLight::new(
                light.center.into(),
                light.normal.into(),
                light.radius,
                light.total_flux,
            )
            .map(|_| ()),
        }
    }
}
impl TryFrom<&LightConfig> for Light {
    type Error = String;
    /// 将光源配置构建为运行时 Light，供直接光和光子发射共用。
    ///
    /// 输入：LightConfig；输出：方向/法线已归一化的 Light，校验或构造失败时 Err。
    fn try_from(value: &LightConfig) -> Result<Self, Self::Error> {
        if matches!(value, LightConfig::Parallel(_)) {
            value.validate()?;
        }
        Ok(match value {
            LightConfig::Parallel(light) => Self::Parallel(ParallelLight {
                height_m: light.height_m,
                direction: Ray::new(Vector3::zeros(), light.direction.into())
                    .ok_or("光源方向无效")?
                    .direction,
                aperture_min_m: Vector2::from(light.aperture_min_m),
                aperture_max_m: Vector2::from(light.aperture_max_m),
                total_flux: light.total_flux,
            }),
            LightConfig::DiskArea(light) => Self::DiskArea(DiskAreaLight::new(
                light.center.into(),
                light.normal.into(),
                light.radius,
                light.total_flux,
            )?),
        })
    }
}

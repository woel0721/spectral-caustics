//! 显式光源类型；旧无 type 的 JSON 由场景字段反序列化入口补为 parallel。

pub use super::scene::ParallelLightConfig;
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
pub struct DiskAreaLightConfig {
    pub center: [f64; 3],
    pub normal: [f64; 3],
    pub radius: f64,
    pub total_flux: f64,
}

/// 将场景 light 字段解码成光源类型，支持省略 type 的平行光 JSON。
///
/// 输入：Serde 反序列化器；输出：LightConfig 或字段解析错误，缺少 type 时按 parallel 解析。
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
    /// 返回配置中的相对总通量 Φ，供两种光源共用能量归一化。
    pub fn total_flux(&self) -> f64 {
        match self {
            Self::Parallel(light) => light.total_flux,
            Self::DiskArea(light) => light.total_flux,
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
                        "scene.scene.light 的高度须有限、方向须有限且非零、总通量须有限且非负"
                            .into(),
                    );
                }
                validate_bounds(light.aperture_min_m, light.aperture_max_m)
                    .map_err(|message| format!("scene.scene.light 发射孔径：{message}"))
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

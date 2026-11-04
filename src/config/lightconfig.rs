//! 显式光源类型；旧无 type 的 JSON 由场景字段反序列化入口补为 parallel。

use super::sceneconfig::{ParallelLightConfig, validate_bounds};
use crate::{
    geometry::ray::Ray,
    light::{DiskAreaLight, Light},
    scene::ParallelLight,
};
use nalgebra::{Vector2, Vector3};
use serde::{Deserialize, Deserializer, Serialize, de::Error};

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

pub(super) fn deserialize_light<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<LightConfig, D::Error> {
    let mut value = serde_json::Value::deserialize(deserializer)?;
    if let Some(object) = value.as_object_mut() {
        object
            .entry("type")
            .or_insert_with(|| serde_json::json!("parallel"));
    }
    serde_json::from_value(value).map_err(D::Error::custom)
}

impl Default for LightConfig {
    fn default() -> Self {
        Self::Parallel(ParallelLightConfig::default())
    }
}
impl From<ParallelLightConfig> for LightConfig {
    fn from(value: ParallelLightConfig) -> Self {
        Self::Parallel(value)
    }
}
impl From<DiskAreaLightConfig> for LightConfig {
    fn from(value: DiskAreaLightConfig) -> Self {
        Self::DiskArea(value)
    }
}
impl LightConfig {
    pub fn as_parallel(&self) -> Option<&ParallelLightConfig> {
        match self {
            Self::Parallel(light) => Some(light),
            Self::DiskArea(_) => None,
        }
    }
    pub fn as_parallel_mut(&mut self) -> Option<&mut ParallelLightConfig> {
        match self {
            Self::Parallel(light) => Some(light),
            Self::DiskArea(_) => None,
        }
    }
    pub fn total_flux(&self) -> f64 {
        match self {
            Self::Parallel(light) => light.total_flux,
            Self::DiskArea(light) => light.total_flux,
        }
    }
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
                    .map_err(|message| format!("scene.light 发射孔径：{message}"))
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
    fn try_from(value: &LightConfig) -> Result<Self, Self::Error> {
        value.validate()?;
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

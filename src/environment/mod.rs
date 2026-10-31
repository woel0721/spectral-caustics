//! 背景与漫反射照明共用的线性环境辐亮度；优先查询 HDR 贴图，否则使用渐变。

pub mod map;

pub use map::EnvironmentMap;

use crate::{config::sceneconfig::EnvironmentConfig, material::unit_vector};
use nalgebra::Vector3;
use std::error::Error;

/// 拥有一次加载的贴图像素，不实现 Copy/Clone，查询期间不进行文件 IO。
/// 全部为相对线性 RGB 辐亮度，解析太阳仍由 Scene.light 独立提供。
#[derive(Debug)]
pub struct Environment {
    pub horizon_color: Vector3<f64>,
    pub zenith_color: Vector3<f64>,
    pub intensity: f64,
    map: Option<EnvironmentMap>,
    /// 构建时缓存正旋转角的 (sin, cos)，查询时使用其逆变换。
    rotation_sin_cos: (f64, f64),
}

impl TryFrom<&EnvironmentConfig> for Environment {
    type Error = Box<dyn Error>;

    fn try_from(config: &EnvironmentConfig) -> Result<Self, Self::Error> {
        config.validate()?;
        let map = config
            .map_path
            .as_ref()
            .map(|path| {
                EnvironmentMap::load(path).map_err(|error| {
                    format!(
                        "无法加载 scene.environment.map_path '{}': {error}",
                        path.display()
                    )
                })
            })
            .transpose()?;
        // 先约减角度，巨大但有限的度数也不会在角度换算中溢出。
        let rotation_sin_cos = config
            .rotation_degrees
            .rem_euclid(360.0)
            .to_radians()
            .sin_cos();
        Ok(Self {
            horizon_color: Vector3::from(config.horizon_linear_rgb),
            zenith_color: Vector3::from(config.zenith_linear_rgb),
            intensity: config.intensity,
            map,
            rotation_sin_cos,
        })
    }
}

impl Environment {
    pub fn is_valid(&self) -> bool {
        self.horizon_color
            .iter()
            .chain(self.zenith_color.iter())
            .all(|value| value.is_finite() && *value >= 0.0)
            && self.intensity.is_finite()
            && self.intensity >= 0.0
            && self.rotation_sin_cos.0.is_finite()
            && self.rotation_sin_cos.1.is_finite()
    }

    /// 相机背景和环境照明都查询这里；保留 HDR，不加曝光、sRGB 编码或裁剪。
    /// 正角度表示环境绕 +Z 旋转，贴图查询方向为 Rz(-rotation) * direction。
    pub fn radiance(&self, direction: Vector3<f64>) -> Option<Vector3<f64>> {
        if !self.is_valid() {
            return None;
        }
        let sampled = if let Some(map) = &self.map {
            if !direction.iter().all(|value| value.is_finite()) {
                return None;
            }
            let scale = direction.amax();
            if scale == 0.0 {
                return None;
            }
            // 先缩放再归一化，避免旋转巨大/极小有限方向时上溢或下溢。
            let direction = direction / scale;
            let direction = direction / direction.norm();
            let (sin, cos) = self.rotation_sin_cos;
            let local = Vector3::new(
                cos * direction.x + sin * direction.y,
                -sin * direction.x + cos * direction.y,
                direction.z,
            );
            if !local.iter().all(|value| value.is_finite()) {
                return None;
            }
            map.radiance(local)?
        } else {
            // 保留原渐变的归一化和下半球约定；绕 Z 轴旋转不改变 z。
            let direction = unit_vector(direction)?;
            let t = direction.z.clamp(0.0, 1.0);
            self.horizon_color * (1.0 - t) + self.zenith_color * t
        };
        let rgb = sampled * self.intensity;
        rgb.iter().all(|value| value.is_finite()).then_some(rgb)
    }
}

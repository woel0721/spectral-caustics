//! 背景与漫反射照明共用的线性环境辐亮度；优先查询 HDR 贴图，否则使用渐变。

pub mod map;

pub use map::EnvironmentMap;

use crate::{config::sceneconfig::EnvironmentConfig, material::unit_vector};
use nalgebra::Vector3;
use rand::RngCore;
use std::error::Error;

/// 环境方向样本；方向从着色点指向环境，不包含辐亮度或 BSDF 权重。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvironmentSample {
    /// 单位方向；Environment 返回世界空间，EnvironmentMap 返回贴图局部空间。
    pub direction: Vector3<f64>,
    /// 相对于立体角 dω 的完整采样密度，单位 sr^-1。
    pub pdf: f64,
}

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
    /// HDRI 局部方向经 Rz(+rotation) 转到世界；渐变天空暂用均匀球面采样。
    /// 采样缓存只读，强度不参与归一化 PDF；非法运行时参数不消耗 RNG。
    pub fn sample_direction(&self, rng: &mut dyn RngCore) -> Option<EnvironmentSample> {
        if !self.is_valid() {
            return None;
        }
        let Some(map) = &self.map else {
            return Some(map::sample_uniform_sphere(rng));
        };
        let local = map.sample_direction(rng)?.direction;
        let (sin, cos) = self.rotation_sin_cos;
        let direction = map::normalized_direction(Vector3::new(
            cos * local.x - sin * local.y,
            sin * local.x + cos * local.y,
            local.z,
        ))?;
        Some(EnvironmentSample {
            direction,
            pdf: self.pdf_direction(direction)?,
        })
    }

    /// 世界方向用 Rz(-rotation) 查询同一分布；纯旋转不改变立体角密度。
    /// 有限非零方向会归一化，NaN、Infinity 和零向量返回 None。
    pub fn pdf_direction(&self, direction: Vector3<f64>) -> Option<f64> {
        if !self.is_valid() {
            return None;
        }
        let direction = map::normalized_direction(direction)?;
        let Some(map) = &self.map else {
            return Some(map::UNIFORM_SPHERE_PDF);
        };
        let (sin, cos) = self.rotation_sin_cos;
        map.pdf_direction(Vector3::new(
            cos * direction.x + sin * direction.y,
            -sin * direction.x + cos * direction.y,
            direction.z,
        ))
    }

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

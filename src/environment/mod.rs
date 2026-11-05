//! 背景与漫反射照明共用的线性环境辐亮度；优先查询 HDR 贴图，否则使用渐变。

pub mod map;

pub use map::EnvironmentMap;

use crate::config::scene::EnvironmentConfig;
use crate::material::unit_vector;
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
    horizon_color: Vector3<f64>,
    zenith_color: Vector3<f64>,
    intensity: f64,
    map: Option<EnvironmentMap>,
    /// 构建时缓存正旋转角的 (sin, cos)，查询时使用其逆变换。
    rotation_sin_cos: (f64, f64),
}

impl TryFrom<&EnvironmentConfig> for Environment {
    type Error = Box<dyn Error + Send + Sync>;

    /// 把环境配置构建为可反复查询的运行时环境，避免渲染期间反复读取 HDR 文件。
    ///
    /// 输入：EnvironmentConfig；输出：Environment，配置无效或 HDR 加载失败返回 Err。
    /// 缓存绕 Z 旋转的 sin/cos；有 map_path 时加载 HDR，否则使用地平线到天顶的渐变。
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
    /// 返回是否已加载 HDR 贴图；积分器据此选择环境与材质的混合方向采样。
    pub fn has_map(&self) -> bool {
        self.map.is_some()
    }

    /// 从环境分布抽取一个方向，供环境直接光积分使用。
    ///
    /// 输入：调用方的可变 RNG；输出：世界单位方向及其完整 PDF（sr⁻¹），数值奇异时返回 None。
    /// HDR 方向由局部空间按 Rz(+rotation) 旋到世界；渐变环境此接口采用均匀球面 p=1/(4π)。
    /// 环境亮度倍率不改变归一化后的采样 PDF。
    pub fn sample_direction(&self, rng: &mut dyn RngCore) -> Option<EnvironmentSample> {
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

    /// 查询环境采样生成某个世界方向的概率，供 MIS 比较采样策略。
    ///
    /// 输入：有限非零 direction（内部归一化）；输出：完整立体角密度，单位 sr⁻¹，无效时 None。
    /// HDR 先用 Rz(-rotation) 转回贴图方向；旋转不改变密度，渐变环境返回 1/(4π)。
    pub fn pdf_direction(&self, direction: Vector3<f64>) -> Option<f64> {
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

    pub fn horizon_color(&self) -> Vector3<f64> {
        self.horizon_color
    }
    pub fn zenith_color(&self) -> Vector3<f64> {
        self.zenith_color
    }
    pub fn intensity(&self) -> f64 {
        self.intensity
    }
    /// Validate an external intensity change without reloading the HDR map.
    pub fn set_intensity(&mut self, intensity: f64) -> Result<(), &'static str> {
        if !intensity.is_finite() || intensity < 0.0 {
            return Err("environment intensity must be finite and nonnegative");
        }
        self.intensity = intensity;
        Ok(())
    }
    pub fn set_gradient(
        &mut self,
        horizon: Vector3<f64>,
        zenith: Vector3<f64>,
    ) -> Result<(), &'static str> {
        if horizon
            .iter()
            .chain(zenith.iter())
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err("environment colors must be finite and nonnegative");
        }
        self.horizon_color = horizon;
        self.zenith_color = zenith;
        Ok(())
    }

    /// 沿给定方向读取环境的线性 RGB 辐亮度，供相机背景和环境照明共用。
    ///
    /// 输入：世界方向 direction；输出：保留 HDR 的 RGB，无效方向、参数或结果溢出时 None。
    /// HDR 查询 L=intensity·map(Rz(-rotation)d)；渐变用 t=clamp(normalize(d).z,0,1)，
    /// L=intensity·[(1-t)·horizon+t·zenith]，下半球保持地平线颜色；不加曝光或显示编码。
    pub fn radiance(&self, direction: Vector3<f64>) -> Option<Vector3<f64>> {
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

//! 可保存为 JSON 的材质参数；颜色使用线性 RGB，不包含散射采样或运行时状态。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaterialConfig {
    Lambertian { albedo: [f64; 3] },
    Dielectric { ior: f64 },
    Mirror { reflectance: [f64; 3] },
}

impl Default for MaterialConfig {
    fn default() -> Self {
        Self::Lambertian { albedo: [0.8; 3] }
    }
}

impl MaterialConfig {
    pub fn default_glass() -> Self {
        Self::Dielectric { ior: 1.52 }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Lambertian { albedo }
            | Self::Mirror {
                reflectance: albedo,
            } => {
                if albedo
                    .iter()
                    .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
                {
                    return Err("material.albedo 的三个线性 RGB 分量必须有限且位于 [0, 1]".into());
                }
            }
            Self::Dielectric { ior } => {
                if !ior.is_finite() || *ior <= 0.0 {
                    return Err("material.ior 必须有限且大于零".into());
                }
            }
        }
        Ok(())
    }
}

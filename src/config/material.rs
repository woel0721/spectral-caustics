//! 可保存为 JSON 的材质参数；颜色使用线性 RGB，不包含散射采样或运行时状态。

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaterialConfig {
    Lambertian {
        albedo: [f64; 3],
    },
    Dielectric {
        ior: f64,
        /// None 为 Constant IOR；光源/光谱相机的 dispersion 模式查询 Cauchy，RGB 相机仍使用 ior。
        #[serde(default)]
        dispersion: Option<crate::optics::dispersion::Cauchy>,
    },
    Mirror {
        reflectance: [f64; 3],
    },
    GgxMetal {
        eta: [f64; 3],
        k: [f64; 3],
        roughness: f64,
    },
}

impl Default for MaterialConfig {
    /// 返回线性 RGB 反射率 [0.8,0.8,0.8] 的漫反射配置，供省略材质的地面使用。
    fn default() -> Self {
        Self::Lambertian { albedo: [0.8; 3] }
    }
}

impl MaterialConfig {
    /// 返回常数 IOR=1.52、附带默认 Cauchy 模型的玻璃配置，供省略材质的玻璃几何使用。
    pub fn default_glass() -> Self {
        Self::Dielectric {
            ior: 1.52,
            dispersion: Some(crate::optics::dispersion::Cauchy::default()),
        }
    }

    /// 校验材质配置，供场景构建前报告参数错误。
    ///
    /// 输入：self；输出：有效时 Ok(())，颜色越界、非法折射率/色散系数或 GGX 参数无效时 Err。
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
                    let field = if matches!(self, Self::Lambertian { .. }) {
                        "albedo"
                    } else {
                        "reflectance"
                    };
                    return Err(format!(
                        "material.{field} 的三个线性 RGB 分量必须有限且位于 [0, 1]"
                    ));
                }
            }
            Self::GgxMetal { eta, k, roughness } => {
                crate::material::ggx_metal::GgxMetal::new((*eta).into(), (*k).into(), *roughness)?;
            }
            Self::Dielectric { ior, dispersion } => {
                if !ior.is_finite() || *ior <= 0.0 {
                    return Err("material.ior 必须有限且大于零".into());
                }
                if let Some(model) = dispersion {
                    model.validate()?;
                }
            }
        }
        Ok(())
    }
}

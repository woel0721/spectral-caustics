//! 可保存为 JSON 的场景描述；几何数据在运行时再转换为 Hittable 对象。

use super::{cameraconfig::CameraConfig, materialconfig::MaterialConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneConfig {
    pub camera: CameraConfig,
    pub background_linear_rgb: [f64; 3],
    pub objects: Vec<ObjectConfig>,
    pub light: ParallelLightConfig,
    pub receiver: GroundReceiverConfig,
}

/// 这里只描述物体类型和原始参数，不包含求交函数、法线缓存或 trait object。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObjectConfig {
    /// 与 Prism 当前默认顶点顺序一致，长度单位为米；面连接方式由运行时 Prism 定义。
    Prism {
        vertices_m: [[f64; 3]; 6],
        #[serde(default = "MaterialConfig::default_glass")]
        material: MaterialConfig,
    },
    Ground {
        height_m: f64,
        #[serde(default)]
        material: MaterialConfig,
    },
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
    /// 相对总通量；默认值为原有 0.5 亮度 × 16 平方米，不声明实际瓦特单位。
    pub total_flux: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundReceiverConfig {
    /// 地面接收矩形的世界 x/y 范围，单位为米，采用 [min, max) 半开区间。
    pub min_m: [f64; 2],
    pub max_m: [f64; 2],
}

impl Default for SceneConfig {
    fn default() -> Self {
        Self {
            camera: CameraConfig::default(),
            background_linear_rgb: [0.0; 3],
            objects: vec![
                ObjectConfig::Prism {
                    vertices_m: [
                        [-0.5, -1.0, 1.0],
                        [0.5, -1.0, 1.0],
                        [0.0, -1.0, 1.8],
                        [-0.5, 1.0, 1.0],
                        [0.5, 1.0, 1.0],
                        [0.0, 1.0, 1.8],
                    ],
                    material: MaterialConfig::default_glass(),
                },
                ObjectConfig::Ground {
                    height_m: 0.0,
                    material: MaterialConfig::default(),
                },
            ],
            light: ParallelLightConfig::default(),
            receiver: GroundReceiverConfig::default(),
        }
    }
}

impl Default for ParallelLightConfig {
    fn default() -> Self {
        Self {
            height_m: 3.0,
            direction: [0.0, 0.0, -1.0],
            aperture_min_m: [-2.0; 2],
            aperture_max_m: [2.0; 2],
            total_flux: 8.0,
        }
    }
}

impl Default for GroundReceiverConfig {
    fn default() -> Self {
        Self {
            min_m: [-2.0; 2],
            max_m: [2.0; 2],
        }
    }
}

impl SceneConfig {
    /// 校验描述中的数值；几何拓扑和实际求交对象的构建留给运行时 Scene。
    pub fn validate(&self) -> Result<(), String> {
        self.camera.validate()?;
        if self
            .background_linear_rgb
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err("scene.background_linear_rgb 必须有限且非负".into());
        }
        for (index, object) in self.objects.iter().enumerate() {
            match object {
                ObjectConfig::Prism {
                    vertices_m,
                    material,
                } => {
                    material
                        .validate()
                        .map_err(|message| format!("scene.objects[{index}].{message}"))?;
                    if vertices_m.iter().flatten().any(|v| !v.is_finite()) {
                        return Err("scene 三棱镜顶点必须为有限坐标".into());
                    }
                }
                ObjectConfig::Ground { height_m, material } => {
                    material
                        .validate()
                        .map_err(|message| format!("scene.objects[{index}].{message}"))?;
                    if !height_m.is_finite() {
                        return Err("scene 地面高度必须有限".into());
                    }
                }
            }
        }
        let direction_norm_squared: f64 = self.light.direction.iter().map(|v| v * v).sum();
        if !self.light.height_m.is_finite()
            || !direction_norm_squared.is_finite()
            || direction_norm_squared <= 0.0
            || !self.light.total_flux.is_finite()
            || self.light.total_flux < 0.0
        {
            return Err("scene.light 的高度须有限、方向须有限且非零、总通量须有限且非负".into());
        }
        validate_bounds(self.light.aperture_min_m, self.light.aperture_max_m)
            .map_err(|message| format!("scene.light 发射孔径：{message}"))?;
        validate_bounds(self.receiver.min_m, self.receiver.max_m)
            .map_err(|message| format!("scene.receiver 接收区域：{message}"))?;
        Ok(())
    }
}

fn validate_bounds(min: [f64; 2], max: [f64; 2]) -> Result<(), &'static str> {
    let spans = [max[0] - min[0], max[1] - min[1]];
    let area = spans[0] * spans[1];
    if min.iter().chain(max.iter()).any(|v| !v.is_finite())
        || spans.iter().any(|v| !v.is_finite() || *v <= 0.0)
        || !area.is_finite()
        || area <= 0.0
    {
        return Err("边界必须有限，且每轴跨度和面积须有限、为正");
    }
    Ok(())
}

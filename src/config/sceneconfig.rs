//! 可保存为 JSON 的场景描述；几何数据在运行时再转换为 Hittable 对象。

use super::{cameraconfig::CameraConfig, materialconfig::MaterialConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneConfig {
    pub camera: CameraConfig,
    pub background_linear_rgb: [f64; 3],
    /// 缺省/null 保留旧常量背景且关闭天空照明；显式配置时启用天空。
    #[serde(default)]
    pub environment: Option<EnvironmentConfig>,
    /// 每条相机路径在漫反射终点采样的环境方向数，默认 1。
    #[serde(default = "default_environment_samples")]
    pub environment_samples: usize,
    /// HDRI 多样本 MIS 权重；旧 JSON 缺省为 Balance。
    #[serde(default)]
    pub environment_mis_heuristic: EnvironmentMisHeuristic,
    /// 唯一接收地面的灰度反射率纹理；省略/null 保留纯色材质。
    #[serde(default)]
    pub ground_checkerboard: Option<CheckerboardConfig>,
    /// 地面矩形相对 receiver 各边向外扩展的米数，须为正。
    #[serde(default = "default_ground_margin_m")]
    pub ground_margin_m: f64,
    pub objects: Vec<ObjectConfig>,
    pub light: ParallelLightConfig,
    pub receiver: GroundReceiverConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentMisHeuristic {
    #[default]
    Balance,
    Power,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckerboardConfig {
    /// 世界 x/y 轴上每格的边长，单位米；原点固定在 (0,0)。
    pub cell_size_m: f64,
    /// 两种格子相对于地面材质 albedo 的乘数，范围 [0,1]。
    pub even_scale: f64,
    pub odd_scale: f64,
}

impl Default for CheckerboardConfig {
    fn default() -> Self {
        Self {
            cell_size_m: 0.5,
            even_scale: 1.0,
            odd_scale: 0.25,
        }
    }
}

impl CheckerboardConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.cell_size_m.is_finite() || self.cell_size_m <= 0.0 {
            return Err("scene.ground_checkerboard.cell_size_m 必须有限且大于零".into());
        }
        if [self.even_scale, self.odd_scale]
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err("scene.ground_checkerboard 的反射率乘数必须有限且位于 [0,1]".into());
        }
        Ok(())
    }
}

/// HDR 贴图或渐变天空的相对线性 RGB 参数；解析太阳仍由独立光源提供。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentConfig {
    /// 有路径时优先使用 HDR；相对路径相对于程序启动工作目录。
    #[serde(default)]
    pub map_path: Option<PathBuf>,
    /// 环境绕世界 +Z 轴旋转的角度，单位度。
    #[serde(default)]
    pub rotation_degrees: f64,
    /// 采样后在线性空间乘入，不施加曝光或显示裁剪。
    #[serde(default = "default_environment_intensity")]
    pub intensity: f64,
    /// 背景和照明共用的方向相关线性 RGB 辐亮度。
    pub horizon_linear_rgb: [f64; 3],
    pub zenith_linear_rgb: [f64; 3],
}

impl Default for EnvironmentConfig {
    fn default() -> Self {
        Self {
            map_path: None,
            rotation_degrees: 0.0,
            intensity: default_environment_intensity(),
            horizon_linear_rgb: [0.16, 0.20, 0.28],
            zenith_linear_rgb: [0.04, 0.08, 0.18],
        }
    }
}

impl EnvironmentConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.rotation_degrees.is_finite() {
            return Err("scene.environment.rotation_degrees 必须有限".into());
        }
        if !self.intensity.is_finite() || self.intensity < 0.0 {
            return Err("scene.environment.intensity 必须有限且非负".into());
        }
        if self
            .horizon_linear_rgb
            .iter()
            .chain(self.zenith_linear_rgb.iter())
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err("scene.environment 的天空辐亮度和背景颜色必须有限且非负".into());
        }
        Ok(())
    }
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
    /// 穿过水平发射窗口的相对通量 Phi；默认 8/16=0.5 为水平面辐照度。
    /// 阶段 14 太阳光束法向强度为 Phi/(窗口面积*|归一化 direction.z|)，无瓦特标定。
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
            background_linear_rgb: [0.08, 0.10, 0.14],
            environment: Some(EnvironmentConfig::default()),
            environment_samples: default_environment_samples(),
            environment_mis_heuristic: EnvironmentMisHeuristic::default(),
            ground_checkerboard: Some(CheckerboardConfig::default()),
            ground_margin_m: default_ground_margin_m(),
            objects: vec![
                ObjectConfig::Prism {
                    vertices_m: [
                        [-0.5, -1.0, 0.03],
                        [0.5, -1.0, 0.03],
                        [0.0, -1.0, 0.83],
                        [-0.5, 1.0, 0.03],
                        [0.5, 1.0, 0.03],
                        [0.0, 1.0, 0.83],
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
            direction: [0.45, 0.0, -1.0],
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
    /// 物理地面始终完整包住 receiver；两者都在世界 x/y 坐标中定义。
    pub fn ground_bounds(&self) -> Result<GroundReceiverConfig, String> {
        validate_bounds(self.receiver.min_m, self.receiver.max_m)
            .map_err(|message| format!("scene.receiver 接收区域：{message}"))?;
        if !self.ground_margin_m.is_finite() || self.ground_margin_m <= 0.0 {
            return Err("scene.ground_margin_m 必须有限且大于零".into());
        }
        let min_m = self
            .receiver
            .min_m
            .map(|value| value - self.ground_margin_m);
        let max_m = self
            .receiver
            .max_m
            .map(|value| value + self.ground_margin_m);
        validate_bounds(min_m, max_m).map_err(|message| format!("scene 地面范围：{message}"))?;
        if (0..2).any(|axis| {
            min_m[axis] >= self.receiver.min_m[axis] || max_m[axis] <= self.receiver.max_m[axis]
        }) {
            return Err("scene.ground_margin_m 太小，无法在 f64 精度下扩展 receiver".into());
        }
        Ok(GroundReceiverConfig { min_m, max_m })
    }

    /// 校验描述中的数值；几何拓扑和实际求交对象的构建留给运行时 Scene。
    pub fn validate(&self) -> Result<(), String> {
        self.camera.validate()?;
        if let Some(environment) = &self.environment {
            environment.validate()?;
        }
        if self.environment_samples == 0 {
            return Err("scene.environment_samples 必须大于零".into());
        }
        if let Some(checkerboard) = &self.ground_checkerboard {
            checkerboard.validate()?;
        }
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
        self.ground_bounds()?;
        Ok(())
    }
}

fn default_ground_margin_m() -> f64 {
    0.5
}

fn default_environment_samples() -> usize {
    1
}

fn default_environment_intensity() -> f64 {
    1.0
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

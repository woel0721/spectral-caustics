//! 可保存为 JSON 的场景描述；几何数据在运行时再转换为 Hittable 对象。

use super::camera::CameraConfig;
pub use super::light::DiskAreaLightConfig;
pub use super::light::LightConfig;
use super::material::MaterialConfig;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;

/// 防止误传 usize::MAX 等深度；不分配与深度成比例的路径存储。
pub(crate) const MAX_DIFFUSE_BOUNCES: usize = 1024;

/// 检查连续散射的额外反弹预算 count 是否在 0..=1024；有效返回 Ok(())，超限返回 Err，供配置和直接 API 共用。
pub(crate) fn validate_diffuse_bounces(count: usize) -> Result<(), String> {
    if count > MAX_DIFFUSE_BOUNCES {
        return Err(format!(
            "scene.camera_options.max_diffuse_bounces 不能超过 {MAX_DIFFUSE_BOUNCES}"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneConfig {
    pub camera: CameraConfig,
    pub background_linear_rgb: [f64; 3],
    /// 缺省/null 使用常量背景且关闭天空照明；显式配置时启用天空。
    #[serde(default)]
    pub environment: Option<EnvironmentConfig>,
    /// 单个有界均匀介质；省略/null 为真空，系数单位 m⁻¹。
    #[serde(default)]
    pub medium: Option<crate::medium::HomogeneousMediumConfig>,
    /// 每条相机路径在漫反射终点采样的环境方向数，默认 1。
    #[serde(default = "default_environment_samples")]
    pub environment_samples: usize,
    /// 圆盘 Direct NEE 的每顶点面积样本数；平行光不抽取这些样本。
    #[serde(default = "default_environment_samples")]
    pub light_samples: usize,
    /// HDRI 多样本 MIS 权重；省略字段时缺省为 Balance。
    #[serde(default)]
    pub environment_mis_heuristic: EnvironmentMisHeuristic,
    /// 相机额外 Lambertian 反弹数；缺省为 0，运行时上限见 MAX_DIFFUSE_BOUNCES。
    #[serde(default)]
    pub max_diffuse_bounces: usize,
    /// 唯一接收地面的灰度反射率纹理；省略/null 保留纯色材质。
    #[serde(default)]
    pub ground_checkerboard: Option<CheckerboardConfig>,
    /// 地面矩形相对 receiver 各边向外扩展的米数，须为正。
    #[serde(default = "default_ground_margin_m")]
    pub ground_margin_m: f64,
    /// Debug AOV 的首次命中目标（objects 的零基下标）；省略/null 自动选择三棱镜。
    pub objects: Vec<ObjectConfig>,
    pub light: LightConfig,
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
    /// 两种格子相对于地面材质 albedo 的乘数，范围 `[0,1]`。
    pub even_scale: f64,
    pub odd_scale: f64,
}

impl Default for CheckerboardConfig {
    /// 返回默认棋盘格配置：格子边长 0.5 m，偶/奇格反射率倍率分别为 1 和 0.25。
    fn default() -> Self {
        Self {
            cell_size_m: 0.5,
            even_scale: 1.0,
            odd_scale: 0.25,
        }
    }
}

impl CheckerboardConfig {
    /// 校验棋盘格参数，供程序化地面纹理构建。
    ///
    /// 输入：self；输出：有效时 Ok(())，边长非正/非法或反射率倍率不在 `[0,1]` 时 Err。
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
    /// 返回默认渐变环境配置：不加载 HDR，旋转 0°，强度 1，颜色见构造体。
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
    /// 校验环境颜色、强度和旋转角，供环境构建前使用。
    ///
    /// 输入：self；输出：有效时 Ok(())，非有限参数、负颜色或负强度时 Err；不在此读取 HDR 文件。
    pub fn validate(&self) -> Result<(), String> {
        if !self.rotation_degrees.is_finite() {
            return Err("scene.scene.environment.rotation_degrees 必须有限".into());
        }
        if !self.intensity.is_finite() || self.intensity < 0.0 {
            return Err("scene.scene.environment.intensity 必须有限且非负".into());
        }
        if self
            .horizon_linear_rgb
            .iter()
            .chain(self.zenith_linear_rgb.iter())
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err("scene.scene.environment 的天空辐亮度和背景颜色必须有限且非负".into());
        }
        Ok(())
    }
}

/// 这里只描述物体类型和原始参数，不包含求交函数、法线缓存或 trait object。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObjectConfig {
    /// 无倒角 XY 三角形沿世界 Z 轴挤出，两个端面均为水平三角形。
    VerticalPrism {
        cross_section_m: [[f64; 2]; 3],
        z_min_m: f64,
        z_max_m: f64,
        #[serde(default)]
        caustics: bool,
        #[serde(default = "MaterialConfig::default_glass")]
        material: MaterialConfig,
    },
    /// 与 Prism 当前默认顶点顺序一致，长度单位为米；面连接方式由运行时 Prism 定义。
    Prism {
        vertices_m: [[f64; 3]; 6],
        /// 旧三棱镜配置默认开启；显式 false 可将焦散目标切换到球体。
        #[serde(default)]
        caustics: bool,
        #[serde(default = "MaterialConfig::default_glass")]
        material: MaterialConfig,
    },
    /// 仅对三条 y 方向长棱做固定 8 段圆角；不进入现有三棱镜光谱焦散算法。
    BeveledPrism {
        /// 三角形的 (x,z) 截面，支持顺/逆时针输入。
        cross_section_m: [[f64; 2]; 3],
        y_min_m: f64,
        y_max_m: f64,
        /// 圆弧切点沿每条相邻边截取的长度；非圆角半径。
        bevel_size_m: f64,
        #[serde(default = "MaterialConfig::default_glass")]
        material: MaterialConfig,
    },
    /// 解析球面；理想 Dielectric 可显式启用光源端光谱焦散。
    Sphere {
        center_m: [f64; 3],
        radius_m: f64,
        #[serde(default)]
        caustics: bool,
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
    /// 光束法向辐照度为 Phi/(窗口面积*|归一化 direction.z|)，无瓦特标定。
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
    /// 返回内置场景配置，含默认相机、玻璃三棱镜、地面、平行光和环境。
    /// 环境样本数为 1、Balance MIS、额外漫反射预算为 0；展示 JSON 可以显式采用其他设置。
    fn default() -> Self {
        Self {
            camera: CameraConfig::default(),
            background_linear_rgb: [0.08, 0.10, 0.14],
            environment: Some(EnvironmentConfig::default()),
            medium: None,
            environment_samples: default_environment_samples(),
            light_samples: 1,
            environment_mis_heuristic: EnvironmentMisHeuristic::default(),
            max_diffuse_bounces: 0,
            ground_checkerboard: Some(CheckerboardConfig::default()),
            ground_margin_m: default_ground_margin_m(),
            objects: vec![
                ObjectConfig::Prism {
                    caustics: true,
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
            light: LightConfig::default(),
            receiver: GroundReceiverConfig::default(),
        }
    }
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

impl Default for GroundReceiverConfig {
    /// 返回内置地面接收范围：XY 两轴均为 [-2,2) m，供焦散图保存和查询。
    fn default() -> Self {
        Self {
            min_m: [-2.0; 2],
            max_m: [2.0; 2],
        }
    }
}

impl SceneConfig {
    /// 在焦散接收区域外增加边距，得到用于实际求交的有限地面范围。
    ///
    /// 输入：self.receiver 与正边距 m=ground_margin_m（米）；输出：GroundReceiverConfig，非法或扩展失效时 Err。
    ///
    /// 公式：ground.min=receiver.min-m，ground.max=receiver.max+m，使实体地面完整包住焦散查询范围。
    pub fn ground_bounds(&self) -> Result<GroundReceiverConfig, String> {
        validate_bounds(self.receiver.min_m, self.receiver.max_m)
            .map_err(|message| format!("scene.photon.receiver 接收区域：{message}"))?;
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

    /// 检查场景描述能否用于运行时构建，供 JSON 加载和 RenderJob::from_config 使用。
    ///
    /// 输入：self 中的相机、材质、几何参数、采样预算、光源和接收区域；输出：有效时 Ok(())，否则 Err。
    /// 检查焦散目标唯一性；部分几何直接试构建，六顶点 Prism 的凸性/绕序由运行时构建继续检查。
    pub fn validate(&self) -> Result<(), String> {
        self.camera.validate()?;
        if let Some(medium) = &self.medium {
            crate::medium::HomogeneousMedium::try_from(medium)?;
        }
        if let Some(environment) = &self.environment {
            environment.validate()?;
        }
        if self.environment_samples == 0 {
            return Err("scene.camera_options.environment_samples 必须大于零".into());
        }
        if self.light_samples == 0 {
            return Err("scene.camera_options.light_samples 必须大于零".into());
        }
        validate_diffuse_bounces(self.max_diffuse_bounces)?;
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
        let mut caustics_count = 0;
        for (index, object) in self.objects.iter().enumerate() {
            let selected_material = match object {
                ObjectConfig::Sphere {
                    caustics: true,
                    material,
                    ..
                }
                | ObjectConfig::VerticalPrism {
                    caustics: true,
                    material,
                    ..
                }
                | ObjectConfig::Prism {
                    caustics: true,
                    material,
                    ..
                } => {
                    if !matches!(material, MaterialConfig::Dielectric { .. }) {
                        return Err(format!(
                            "scene.objects[{index}].caustics 要求理想 Dielectric 材质"
                        ));
                    }
                    Some(material)
                }
                _ => None,
            };
            if selected_material.is_some_and(|m| matches!(m, MaterialConfig::Dielectric { .. })) {
                caustics_count += 1;
                if caustics_count > 1 {
                    return Err(
                        "当前只支持一个焦散物体，请仅为一个 Sphere/Prism/VerticalPrism 启用 caustics".into(),
                    );
                }
            }
            match object {
                ObjectConfig::VerticalPrism {
                    cross_section_m,
                    z_min_m,
                    z_max_m,
                    material,
                    ..
                } => {
                    material
                        .validate()
                        .map_err(|message| format!("scene.objects[{index}].{message}"))?;
                    if crate::geometry::vertical_prism::VerticalPrism::new(
                        cross_section_m.map(Into::into),
                        *z_min_m,
                        *z_max_m,
                    )
                    .is_none()
                    {
                        return Err(format!(
                            "scene.objects[{index}] 竖直三棱柱无效：需要有限非退化 XY 三角形和 z_min_m < z_max_m"
                        ));
                    }
                }
                ObjectConfig::Prism {
                    vertices_m,
                    material,
                    ..
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
                ObjectConfig::Sphere {
                    center_m,
                    radius_m,
                    material,
                    ..
                } => {
                    material
                        .validate()
                        .map_err(|message| format!("scene.objects[{index}].{message}"))?;
                    if crate::geometry::sphere::Sphere::new((*center_m).into(), *radius_m).is_none()
                    {
                        return Err(format!(
                            "scene.objects[{index}] 球心必须有限，半径必须有限且为正，平方及范围不能溢出或下溢"
                        ));
                    }
                }
                ObjectConfig::BeveledPrism {
                    cross_section_m,
                    y_min_m,
                    y_max_m,
                    bevel_size_m,
                    material,
                } => {
                    material
                        .validate()
                        .map_err(|message| format!("scene.objects[{index}].{message}"))?;
                    if crate::geometry::beveled_prism::BeveledPrism::new(
                        cross_section_m.map(Into::into),
                        *y_min_m,
                        *y_max_m,
                        *bevel_size_m,
                    )
                    .is_none()
                    {
                        return Err(format!(
                            "scene.objects[{index}] 倒角三棱镜无效：需要有限非退化截面、y_min_m < y_max_m、正 bevel_size_m 且小于每条边长的一半"
                        ));
                    }
                }
            }
        }
        self.light.validate()?;
        self.ground_bounds()?;
        Ok(())
    }
}

/// 为省略 ground_margin_m 的 JSON 返回 0.5 m，作为实体地面相对焦散接收区的外扩边距。
fn default_ground_margin_m() -> f64 {
    0.5
}

/// 为省略的环境/面积光采样配额返回 1，即每个着色顶点一个样本。
fn default_environment_samples() -> usize {
    1
}

/// 为省略环境 intensity 的 JSON 返回 1.0，表示保持原始环境亮度。
fn default_environment_intensity() -> f64 {
    1.0
}

/// 检查 XY 矩形是否适合发射窗口或焦散接收区，供两种配置共用。
///
/// 输入：min/max（米）；输出：有效时 Ok(())，边界非有限、任一跨度≤0 或面积无效时 Err。
///
/// 公式：跨度 s=max-min，面积 A=s_x·s_y。
pub(super) fn validate_bounds(min: [f64; 2], max: [f64; 2]) -> Result<(), &'static str> {
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

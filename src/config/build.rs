//! Scene 的默认构建、JSON 转换与几何/材质实例化。

use super::{
    RenderJob,
    job::{CameraOptions, PhotonOptions},
};
use crate::camera::Camera;
use crate::caustics::GroundReceiver;
use crate::config::scene::ObjectConfig;
use crate::config::scene::SceneConfig;
use crate::environment::Environment;
use crate::geometry::beveled_prism::BeveledPrism;
use crate::geometry::ground::Ground;
use crate::geometry::prism::Prism;
use crate::geometry::sphere::Sphere;
use crate::geometry::vertical_prism::VerticalPrism;
use crate::light::Light;
use crate::material;
use crate::material::checkerboard::Checkerboard;
use crate::scene::Scene;
use nalgebra::Vector2;
use nalgebra::Vector3;
use std::error::Error;

impl Default for RenderJob {
    /// 返回与 Scene::new 相同的空运行时场景；几何物体需后续添加。
    fn default() -> Self {
        Self::new()
    }
}

impl RenderJob {
    /// 创建使用默认共享参数的空场景，供示例和测试手工添加几何。
    ///
    /// 输入：无；输出：带相机、环境、光源和接收范围的 Scene，物体/材质表为空。
    pub fn new() -> Self {
        Self::empty_from_config(&SceneConfig::default()).expect("内置默认场景参数必须有效")
    }

    /// 从配置初始化场景的共享参数，供完整场景构建复用。
    ///
    /// 输入：由调用方负责校验的 SceneConfig；输出：尚无物体/材质的 Scene，相机/光源构建或 HDR 加载失败返回 Err。
    fn empty_from_config(config: &SceneConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Ok(Self {
            camera: Camera::from_config(&config.camera)?,
            scene: Scene {
                background_color: config.background_linear_rgb.into(),
                medium: config
                    .medium
                    .as_ref()
                    .map(crate::medium::HomogeneousMedium::try_from)
                    .transpose()?,
                environment: config
                    .environment
                    .as_ref()
                    .map(Environment::try_from)
                    .transpose()?,
                objects: Vec::new(),
                materials: Vec::new(),
                light: Light::try_from(&config.light)?,
            },
            camera_options: CameraOptions {
                environment_samples: config.environment_samples,
                light_samples: config.light_samples,
                environment_mis_heuristic: config.environment_mis_heuristic,
                max_diffuse_bounces: config.max_diffuse_bounces,
            },
            photon: PhotonOptions {
                receiver: GroundReceiver {
                    min_m: config.receiver.min_m.into(),
                    max_m: config.receiver.max_m.into(),
                },
                caustics_object_id: None,
                ground_id: None,
            },
            ground_checkerboard: config
                .ground_checkerboard
                .as_ref()
                .map(Checkerboard::try_from)
                .transpose()?,
            #[cfg(feature = "diagnostics")]
            diagnostics: Default::default(),
        })
    }

    /// 将场景配置构建为可求交、可照明的运行时场景。
    ///
    /// 输入：SceneConfig；输出：含相机、光源、几何、材质和编号绑定的 Scene，配置/几何/HDR 构建失败时 Err。
    /// 要求一个接收地面；可显式选择一个理想介质 Sphere/Prism/VerticalPrism 作为光源焦散目标。
    pub fn from_config(config: &SceneConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        crate::config::scene::validate_diffuse_bounces(config.max_diffuse_bounces)?;
        if config.environment_samples == 0 || config.light_samples == 0 {
            return Err("sampling counts must be positive".into());
        }
        if config
            .background_linear_rgb
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err("invalid background radiance".into());
        }
        let mut scene = Self::empty_from_config(config)?;
        let ground_bounds = config.ground_bounds()?;
        for object in &config.objects {
            match object {
                ObjectConfig::VerticalPrism {
                    cross_section_m,
                    z_min_m,
                    z_max_m,
                    material,
                    caustics,
                } => {
                    let prism =
                        VerticalPrism::new(cross_section_m.map(Vector2::from), *z_min_m, *z_max_m)
                            .ok_or("竖直三棱柱的 XY 截面或 Z 范围无效")?;
                    let id = scene
                        .scene
                        .add(Box::new(prism), material::from_config(material)?);
                    if *caustics {
                        scene.select_caustics_object(id)?;
                    }
                }
                ObjectConfig::Prism {
                    vertices_m,
                    material,
                    caustics,
                } => {
                    let prism = Prism::from_vertices(vertices_m.map(Vector3::from))
                        .ok_or("三棱镜顶点无效：需要按示例顺序描述非退化、朝外法线的凸三棱镜")?;
                    let id = scene
                        .scene
                        .add(Box::new(prism), material::from_config(material)?);
                    if *caustics {
                        scene.select_caustics_object(id)?;
                    }
                }
                ObjectConfig::Ground { height_m, material } => {
                    if scene.photon.ground_id.is_some() {
                        return Err("当前场景只支持一个接收地面".into());
                    }
                    if config
                        .light
                        .as_parallel()
                        .is_some_and(|light| *height_m >= light.height_m)
                    {
                        return Err("光源发射面必须高于接收地面".into());
                    }
                    scene.photon.ground_id = Some(
                        scene.scene.add(
                            Box::new(
                                Ground::with_bounds(
                                    *height_m,
                                    Vector2::from(ground_bounds.min_m),
                                    Vector2::from(ground_bounds.max_m),
                                )
                                .ok_or("地面高度或范围无效")?,
                            ),
                            material::from_config(material)?,
                        ),
                    );
                }
                ObjectConfig::Sphere {
                    center_m,
                    radius_m,
                    material,
                    caustics,
                } => {
                    let sphere = Sphere::new(Vector3::from(*center_m), *radius_m)
                        .ok_or("球体的球心或半径无效")?;
                    let id = scene
                        .scene
                        .add(Box::new(sphere), material::from_config(material)?);
                    if *caustics {
                        scene.select_caustics_object(id)?;
                    }
                }
                ObjectConfig::BeveledPrism {
                    cross_section_m,
                    y_min_m,
                    y_max_m,
                    bevel_size_m,
                    material,
                } => {
                    let prism = BeveledPrism::new(
                        cross_section_m.map(Vector2::from),
                        *y_min_m,
                        *y_max_m,
                        *bevel_size_m,
                    )
                    .ok_or("倒角三棱镜截面、y 范围或倒角长度无效")?;
                    scene
                        .scene
                        .add(Box::new(prism), material::from_config(material)?);
                }
            }
        }
        if scene.photon.ground_id.is_none() {
            return Err("场景必须包含一个 ground 接收地面".into());
        }
        Ok(scene)
    }
}

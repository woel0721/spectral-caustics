//! RenderJob 的默认构建、JSON 转换与几何/材质实例化。

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

pub(super) fn build_prism(vertices_m: [[f64; 3]; 6], index: usize) -> Result<Prism, String> {
    Prism::from_vertices(vertices_m.map(Vector3::from)).ok_or_else(|| {
        format!("scene.objects[{index}].vertices_m 须按示例顺序描述非退化、朝外法线的凸三棱镜")
    })
}

impl Default for RenderJob {
    /// 返回与 RenderJob::new 相同的空运行时任务；几何物体需后续添加。
    fn default() -> Self {
        Self::new()
    }
}

impl RenderJob {
    /// 创建使用默认共享参数的空运行时任务，供示例和测试手工添加几何。
    ///
    /// 输入：无；输出：带相机、环境、光源和接收范围的 RenderJob，物体/材质表为空。
    pub fn new() -> Self {
        Self::empty_from_config(&SceneConfig::default()).expect("内置默认场景参数必须有效")
    }

    /// 从配置初始化运行时任务的共享参数，供完整场景构建复用。
    ///
    /// 输入：由调用方负责校验的 SceneConfig；输出：尚无物体/材质的 RenderJob，相机/光源构建或 HDR 加载失败返回 Err。
    fn empty_from_config(config: &SceneConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let medium = config
            .medium
            .as_ref()
            .map(crate::medium::from_config)
            .transpose()?;
        Ok(Self {
            camera: Camera::from_config(&config.camera)?,
            scene: Scene {
                background_color: config.background_linear_rgb.into(),
                medium,
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
                max_volume_scatters: config.max_volume_scatters,
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
            diagnostic_target: None,
        })
    }

    /// 将场景配置构建为可求交、可照明的运行时渲染任务。
    ///
    /// 输入：SceneConfig；输出：含相机、光源、几何、材质和编号绑定的 RenderJob，配置/几何/HDR 构建失败时 Err。
    /// 要求一个接收地面；可显式选择一个理想介质 Sphere/Prism/VerticalPrism 作为光源焦散目标。
    pub fn from_config(config: &SceneConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        crate::config::scene::validate_diffuse_bounces(config.max_diffuse_bounces)?;
        crate::config::scene::validate_volume_scatters(config.max_volume_scatters)?;
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
        config.camera.validate()?;
        config.light.validate()?;
        if let Some(medium) = &config.medium {
            medium.validate()?;
        }
        if let Some(environment) = &config.environment {
            environment.validate()?;
        }
        if let Some(checkerboard) = &config.ground_checkerboard {
            checkerboard.validate()?;
        }
        let ground_bounds = config.ground_bounds()?;
        let mut ground_count = 0usize;
        let mut caustics_count = 0usize;
        let mut has_medium_interface = false;
        for (index, object) in config.objects.iter().enumerate() {
            if let Some((interface, mat)) = object.medium_interface() {
                let field = format!("scene.objects[{index}].medium_interface");
                if has_medium_interface {
                    return Err(format!("{field} 一个场景仅支持一个介质界面").into());
                }
                if !matches!(mat, super::material::MaterialConfig::Dielectric { .. }) {
                    return Err(format!("{field} 要求理想 Dielectric 材质").into());
                }
                interface
                    .validate()
                    .map_err(|error| format!("{field}.interior {error}"))?;
                has_medium_interface = true;
            }
            object
                .material()
                .validate()
                .map_err(|message| format!("scene.objects[{index}].{message}"))?;
            match object {
                ObjectConfig::Ground { height_m, .. } => {
                    if !height_m.is_finite() {
                        return Err("地面高度或范围无效".into());
                    }
                    ground_count += 1;
                    if ground_count > 1 {
                        return Err("当前场景只支持一个接收地面".into());
                    }
                    if config
                        .light
                        .as_parallel()
                        .is_some_and(|light| *height_m >= light.height_m)
                    {
                        return Err("光源发射面必须高于接收地面".into());
                    }
                }
                ObjectConfig::Sphere {
                    caustics: true,
                    material,
                    ..
                }
                | ObjectConfig::Prism {
                    caustics: true,
                    material,
                    ..
                }
                | ObjectConfig::VerticalPrism {
                    caustics: true,
                    material,
                    ..
                } => {
                    if !matches!(material, super::material::MaterialConfig::Dielectric { .. }) {
                        return Err("焦散物体必须是接收地面以外的理想 Dielectric".into());
                    }
                    caustics_count += 1;
                    if caustics_count > 1 {
                        return Err(
                            "当前只支持一个焦散物体，请仅为一个 Sphere/Prism/VerticalPrism 启用 caustics"
                                .into(),
                        );
                    }
                }
                _ => {}
            }
        }
        if ground_count == 0 {
            return Err("场景必须包含一个 ground 接收地面".into());
        }
        let mut scene = Self::empty_from_config(config)?;
        for (index, object) in config.objects.iter().enumerate() {
            let (id, caustics) = match object {
                ObjectConfig::VerticalPrism {
                    cross_section_m,
                    z_min_m,
                    z_max_m,
                    material,
                    caustics,
                    ..
                } => {
                    let prism =
                        VerticalPrism::new(cross_section_m.map(Vector2::from), *z_min_m, *z_max_m)
                            .ok_or("竖直三棱柱的 XY 截面或 Z 范围无效")?;
                    let id = scene
                        .scene
                        .add(Box::new(prism), material::from_config(material)?);
                    (id, *caustics)
                }
                ObjectConfig::Prism {
                    vertices_m,
                    material,
                    caustics,
                    ..
                } => {
                    let prism = build_prism(*vertices_m, index)?;
                    let id = scene
                        .scene
                        .add(Box::new(prism), material::from_config(material)?);
                    (id, *caustics)
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
                    let id = scene.scene.add(
                        Box::new(
                            Ground::with_bounds(
                                *height_m,
                                Vector2::from(ground_bounds.min_m),
                                Vector2::from(ground_bounds.max_m),
                            )
                            .ok_or("地面高度或范围无效")?,
                        ),
                        material::from_config(material)?,
                    );
                    scene.photon.ground_id = Some(id);
                    (id, false)
                }
                ObjectConfig::Sphere {
                    center_m,
                    radius_m,
                    material,
                    caustics,
                    ..
                } => {
                    let sphere = Sphere::new(Vector3::from(*center_m), *radius_m)
                        .ok_or("球体的球心或半径无效")?;
                    let id = scene
                        .scene
                        .add(Box::new(sphere), material::from_config(material)?);
                    (id, *caustics)
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
                    let id = scene
                        .scene
                        .add(Box::new(prism), material::from_config(material)?);
                    (id, false)
                }
            };
            if let Some((interface, _)) = object.medium_interface() {
                let field = format!("scene.objects[{index}].medium_interface");
                let interface = interface
                    .build()
                    .map_err(|error| format!("{field}.interior {error}"))?;
                scene
                    .scene
                    .bind_medium_interface(id, interface)
                    .map_err(|error| format!("{field} {error}"))?;
            }
            if caustics {
                scene.select_caustics_object(id)?;
            }
        }
        if scene.photon.ground_id.is_none() {
            return Err("场景必须包含一个 ground 接收地面".into());
        }
        Ok(scene)
    }
}

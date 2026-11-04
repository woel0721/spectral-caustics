//! Scene 的默认构建、JSON 转换与几何/材质实例化。

use super::{Checkerboard, Environment, GroundReceiver, Light, Scene};
use crate::{
    camera::Camera,
    config::sceneconfig::{ObjectConfig, SceneConfig},
    geometry::{
        beveled_prism::BeveledPrism, ground::Ground, prism::Prism, sphere::Sphere,
        vertical_prism::VerticalPrism,
    },
    material,
};
use nalgebra::{Vector2, Vector3};
use std::error::Error;

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    /// 创建空场景，使用默认配置中的相机、天空与备用常量背景。
    pub fn new() -> Self {
        Self::empty_from_config(&SceneConfig::default()).expect("内置默认场景参数必须有效")
    }

    /// 只初始化共享字段；调用者负责校验配置，几何/材质由 from_config 另行添加。
    fn empty_from_config(config: &SceneConfig) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            camera: Camera::from_config(&config.camera)?,
            background_color: Vector3::from(config.background_linear_rgb),
            environment: config
                .environment
                .as_ref()
                .map(Environment::try_from)
                .transpose()?,
            environment_samples: config.environment_samples,
            light_samples: config.light_samples,
            environment_mis_heuristic: config.environment_mis_heuristic,
            max_diffuse_bounces: config.max_diffuse_bounces,
            ground_checkerboard: config
                .ground_checkerboard
                .as_ref()
                .map(Checkerboard::try_from)
                .transpose()?,
            objects: Vec::new(),
            materials: Vec::new(),
            light: Light::try_from(&config.light)?,
            receiver: GroundReceiver {
                min_m: Vector2::from(config.receiver.min_m),
                max_m: Vector2::from(config.receiver.max_m),
            },
            prism_id: None,
            caustics_object_id: None,
            ground_id: None,
            debug_object_id: None,
        })
    }

    /// 支持一个地面及一个选中的理想介质 Sphere/Prism 焦散目标。
    pub fn from_config(config: &SceneConfig) -> Result<Self, Box<dyn Error>> {
        config.validate()?;
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
                    if scene.prism_id.is_some() {
                        return Err("当前渲染只支持一个三棱镜/竖直三棱柱".into());
                    }
                    let prism =
                        VerticalPrism::new(cross_section_m.map(Vector2::from), *z_min_m, *z_max_m)
                            .ok_or("竖直三棱柱的 XY 截面或 Z 范围无效")?;
                    let id =
                        scene.add_with_material(Box::new(prism), material::from_config(material)?);
                    scene.prism_id = Some(id);
                    if *caustics {
                        scene.select_caustics_object(id)?;
                    }
                }
                ObjectConfig::Prism {
                    vertices_m,
                    material,
                    caustics,
                } => {
                    if scene.prism_id.is_some() {
                        return Err("当前渲染只支持一个三棱镜".into());
                    }
                    let prism = Prism::from_vertices(vertices_m.map(Vector3::from))
                        .ok_or("三棱镜顶点无效：需要按示例顺序描述非退化、朝外法线的凸三棱镜")?;
                    scene.prism_id = Some(
                        scene.add_with_material(Box::new(prism), material::from_config(material)?),
                    );
                    // 非介质三棱镜保留原有的二值遮挡行为。
                    if *caustics && scene.supports_ideal_caustics(scene.prism_id.unwrap()) {
                        scene.select_caustics_object(scene.prism_id.unwrap())?;
                    }
                }
                ObjectConfig::Ground { height_m, material } => {
                    if scene.ground_id.is_some() {
                        return Err("当前场景只支持一个接收地面".into());
                    }
                    if config
                        .light
                        .as_parallel()
                        .is_some_and(|light| *height_m >= light.height_m)
                    {
                        return Err("光源发射面必须高于接收地面".into());
                    }
                    scene.ground_id = Some(
                        scene.add_with_material(
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
                    let id =
                        scene.add_with_material(Box::new(sphere), material::from_config(material)?);
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
                    scene.add_with_material(Box::new(prism), material::from_config(material)?);
                }
            }
        }
        if scene.ground_id.is_none() {
            return Err("场景必须包含一个 ground 接收地面".into());
        }
        scene.debug_object_id = config.debug_object_id.or(scene.prism_id);
        Ok(scene)
    }
}

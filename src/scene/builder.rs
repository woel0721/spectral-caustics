//! Scene 的默认构建、JSON 转换与几何/材质实例化。

use super::{Checkerboard, Environment, GroundReceiver, ParallelLight, Scene};
use crate::{
    camera::Camera,
    config::sceneconfig::{ObjectConfig, SceneConfig},
    geometry::{ground::Ground, prism::Prism, ray::Ray},
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
        let config = SceneConfig::default();
        Self {
            camera: Camera::from_config(&config.camera).expect("内置默认相机参数必须有效"),
            background_color: Vector3::from(config.background_linear_rgb),
            environment: config
                .environment
                .as_ref()
                .map(Environment::try_from)
                .transpose()
                .expect("内置默认环境参数必须有效"),
            environment_samples: config.environment_samples,
            environment_mis_heuristic: config.environment_mis_heuristic,
            max_diffuse_bounces: config.max_diffuse_bounces,
            ground_checkerboard: config
                .ground_checkerboard
                .as_ref()
                .map(Checkerboard::try_from)
                .transpose()
                .expect("内置默认棋盘格参数必须有效"),
            objects: Vec::new(),
            materials: Vec::new(),
            light: ParallelLight {
                height_m: config.light.height_m,
                direction: Ray::new(Vector3::zeros(), Vector3::from(config.light.direction))
                    .expect("内置默认光源方向必须有效")
                    .direction,
                aperture_min_m: Vector2::from(config.light.aperture_min_m),
                aperture_max_m: Vector2::from(config.light.aperture_max_m),
                total_flux: config.light.total_flux,
            },
            receiver: GroundReceiver {
                min_m: Vector2::from(config.receiver.min_m),
                max_m: Vector2::from(config.receiver.max_m),
            },
            prism_id: None,
            ground_id: None,
        }
    }

    /// 从纯数据描述建立真实物体和数学对象；当前支持一个地面、至多一个三棱镜。
    pub fn from_config(config: &SceneConfig) -> Result<Self, Box<dyn Error>> {
        config.validate()?;
        let mut scene = Self::new();
        scene.camera = Camera::from_config(&config.camera)?;
        scene.background_color = Vector3::from(config.background_linear_rgb);
        // 每次场景构建只加载一次 HDR；所有背景和照明查询共用内存中的像素。
        scene.environment = config
            .environment
            .as_ref()
            .map(Environment::try_from)
            .transpose()?;
        scene.environment_samples = config.environment_samples;
        scene.environment_mis_heuristic = config.environment_mis_heuristic;
        scene.max_diffuse_bounces = config.max_diffuse_bounces;
        scene.ground_checkerboard = config
            .ground_checkerboard
            .as_ref()
            .map(Checkerboard::try_from)
            .transpose()?;
        scene.light = ParallelLight {
            height_m: config.light.height_m,
            direction: Ray::new(Vector3::zeros(), Vector3::from(config.light.direction))
                .ok_or("光源方向无法归一化")?
                .direction,
            aperture_min_m: Vector2::from(config.light.aperture_min_m),
            aperture_max_m: Vector2::from(config.light.aperture_max_m),
            total_flux: config.light.total_flux,
        };
        scene.receiver = GroundReceiver {
            min_m: Vector2::from(config.receiver.min_m),
            max_m: Vector2::from(config.receiver.max_m),
        };
        let ground_bounds = config.ground_bounds()?;
        for object in &config.objects {
            match object {
                ObjectConfig::Prism {
                    vertices_m,
                    material,
                } => {
                    if scene.prism_id.is_some() {
                        return Err("当前渲染只支持一个三棱镜".into());
                    }
                    let prism = Prism::from_vertices(vertices_m.map(Vector3::from))
                        .ok_or("三棱镜顶点无效：需要按示例顺序描述非退化、朝外法线的凸三棱镜")?;
                    scene.prism_id = Some(
                        scene.add_with_material(Box::new(prism), material::from_config(material)?),
                    );
                }
                ObjectConfig::Ground { height_m, material } => {
                    if scene.ground_id.is_some() {
                        return Err("当前场景只支持一个接收地面".into());
                    }
                    if *height_m >= config.light.height_m {
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
            }
        }
        if scene.ground_id.is_none() {
            return Err("场景必须包含一个 ground 接收地面".into());
        }
        Ok(scene)
    }
}

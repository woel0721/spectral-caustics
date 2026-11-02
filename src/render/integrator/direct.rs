//! 解析太阳的照明约定、直接 BSDF 求值与二值阴影。

use super::{ground::GroundLighting, offset_ray_origin};
use crate::{
    geometry::{hit::Hit, ray::Ray},
    material::{BsdfValue, EvaluateMode, Material},
    render::utils::xyz_to_linear_srgb,
    scene::Scene,
};
use nalgebra::Vector3;
use std::{error::Error, mem::size_of};

/// 光谱绑定使用 D65 RGB；不绑定光源图的独立相机使用中性白光。
pub(super) fn surface_sun_lighting(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    (light_wi, light): (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    match ground {
        Some(sun) => Ok(
            direct_lighting(scene, hit, material, wo, sun.light_wi, 1.0)?.component_mul(
                &xyz_to_linear_srgb(sun.sun_irradiance_xyz).ok_or("太阳 XYZ 转换失败")?,
            ),
        ),
        None => direct_lighting(scene, hit, material, wo, light_wi, light),
    }
}

pub(super) fn visible_to_light(
    scene: &Scene,
    position: Vector3<f64>,
    normal: Vector3<f64>,
    wi: Vector3<f64>,
) -> Result<bool, Box<dyn Error>> {
    let origin = offset_ray_origin(position, normal, 1.0);
    let shadow_ray = Ray::new(origin, wi).ok_or("阴影光线起点或方向无效")?;
    Ok(scene.intersect(&shadow_ray).is_none())
}

/// 校验相机工作量、环境与光源，返回中性白光方向和相对强度。
pub(super) fn camera_lighting_setup(
    scene: &Scene,
    width: usize,
    height: usize,
) -> Result<(Vector3<f64>, f64), Box<dyn Error>> {
    crate::config::sceneconfig::validate_diffuse_bounces(scene.max_diffuse_bounces)?;
    if scene.environment_samples == 0 {
        return Err("环境方向样本数必须大于零".into());
    }
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(scene.environment_samples))
        .ok_or("环境方向样本总数溢出")?;
    if scene
        .environment
        .as_ref()
        .is_some_and(|environment| !environment.is_valid())
    {
        return Err("天空辐亮度和背景颜色必须有限且非负".into());
    }
    if width == 0 || height == 0 {
        return Err("相机光照图的宽高必须大于零".into());
    }
    u32::try_from(width)?;
    u32::try_from(height)?;
    width
        .checked_mul(height)
        .and_then(|count| count.checked_mul(size_of::<Vector3<f64>>()))
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or("相机光照图的像素缓冲大小溢出")?;
    let direction_length = scene.light.direction.norm();
    if !direction_length.is_finite() || direction_length == 0.0 {
        return Err("光源方向必须有限且非零".into());
    }
    let wi = -scene.light.direction / direction_length;
    let span = scene.light.aperture_max_m - scene.light.aperture_min_m;
    let area = span.x * span.y;
    if span.iter().any(|v| !v.is_finite() || *v <= 0.0)
        || !area.is_finite()
        || area <= 0.0
        || !scene.light.total_flux.is_finite()
        || scene.light.total_flux < 0.0
    {
        return Err("光源孔径面积须有限且为正，总通量须有限且非负".into());
    }
    let light = scene.light.total_flux / area;
    if !light.is_finite() {
        return Err("直接光照强度计算溢出".into());
    }
    Ok((wi, light))
}

pub(super) fn direct_lighting(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    wi: Vector3<f64>,
    light: f64,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let normal_length = hit.normal.norm();
    if !normal_length.is_finite() || normal_length == 0.0 {
        return Err("命中法线无效，无法计算直接光照".into());
    }
    let normal = hit.normal / normal_length;
    let BsdfValue::Rgb(f_r) = material.evaluate(wi, wo, normal, EvaluateMode::Rgb) else {
        return Err("材质 RGB 求值必须返回 RGB BSDF".into());
    };
    if f_r.iter().any(|value| !value.is_finite() || *value < 0.0) {
        return Err("直接光照 BSDF 必须有限且非负".into());
    }
    let cosine = normal.dot(&wi).clamp(0.0, 1.0);
    let direct_rgb = f_r * (light * cosine);
    if !direct_rgb.iter().all(|v| v.is_finite()) {
        return Err("直接光照颜色非法或计算溢出".into());
    }
    if direct_rgb != Vector3::zeros() && !visible_to_light(scene, hit.coords, normal, wi)? {
        return Ok(Vector3::zeros());
    }
    Ok(direct_rgb)
}

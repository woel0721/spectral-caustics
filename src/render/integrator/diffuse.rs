//! Lambertian 单步采样、后续顶点直接光与 Russian Roulette；无生产路径循环。

use super::{
    direct::surface_sun_lighting,
    environment::environment_lighting,
    ground::{GroundLighting, direct_ground_irradiance, lambertian_radiance},
    offset_ray_origin,
    types::{CameraPathState, DiffuseRay},
};
use crate::{
    geometry::{hit::Hit, ray::Ray},
    material::{BsdfValue, EvaluateMode, Material, MaterialSample, unit_vector},
    render::utils::xyz_to_linear_srgb,
    scene::Scene,
};
use nalgebra::Vector3;
use rand::Rng;
use std::error::Error;

/// 第三次漫反射后的顶点已着色，且还有预算时，RR 只控制之后的贡献。
/// 使用真实路径 RGB 吞吐量（含玻璃前缀）；存活补偿不缩放已累计辐亮度。
pub(super) fn russian_roulette(
    state: &mut CameraPathState,
    rng: &mut dyn rand::RngCore,
) -> Result<bool, Box<dyn Error>> {
    if state.diffuse_bounces < 3 || state.diffuse_bounces >= state.max_diffuse_bounces {
        return Ok(true);
    }
    if !state.throughput.iter().all(|v| v.is_finite() && *v >= 0.0) {
        return Err("Russian Roulette 吞吐量必须有限且非负".into());
    }
    let maximum = state.throughput.max();
    if maximum == 0.0 {
        return Ok(false);
    }
    let survival_probability = maximum.clamp(0.05, 0.95);
    if rng.random::<f64>() >= survival_probability {
        return Ok(false);
    }
    state.throughput /= survival_probability;
    if !state.throughput.iter().all(|v| v.is_finite()) {
        return Err("Russian Roulette 存活补偿计算溢出".into());
    }
    Ok(true)
}

/// 各深度共用一份着色逻辑；返回的 L 已含本顶点的 BSDF 和一次棋盘格调制。
pub(super) fn secondary_direct_lighting(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn rand::RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let direct = if let Some(g) = ground.filter(|g| hit.object_id == Some(g.object_id)) {
        let e = direct_ground_irradiance(scene, hit, g.light_wi, g.sun_irradiance_xyz)?;
        xyz_to_linear_srgb(lambertian_radiance(e, g.rho).ok_or("第二表面地面直射计算失败")?)
            .ok_or("第二表面地面 XYZ 转换失败")?
    } else {
        surface_sun_lighting(scene, hit, material, wo, light, ground)?
    };
    let environment = environment_lighting(scene, hit, material, wo, rng)?;
    let scale = scene
        .ground_reflectance_scale(hit)
        .ok_or("第二表面棋盘格坐标无效")?;
    Ok((direct + environment) * scale)
}

/// A 的 RGB 吞吐量为 f_r*cos/pdf，并且仅乘一次 A 的棋盘格反射率。
pub(super) fn sample_diffuse_ray(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    rng: &mut dyn rand::RngCore,
) -> Result<Option<DiffuseRay>, Box<dyn Error>> {
    if !material.is_lambertian() || material.is_delta() {
        return Ok(None);
    }
    let normal = unit_vector(hit.normal).ok_or("漫反射采样法线无效")?;
    let wo = unit_vector(wo).ok_or("漫反射采样出射方向无效")?;
    if normal.dot(&wo) <= 0.0 {
        return Ok(None);
    }
    let Some(sample) = material.sample(wo, normal, rng) else {
        return Ok(None);
    };
    let MaterialSample::Continuous { wi } = sample else {
        return Err("Lambertian 采样必须返回连续方向".into());
    };
    let wi = unit_vector(wi).ok_or("漫反射采样方向无效")?;
    let cosine = normal.dot(&wi).clamp(0.0, 1.0);
    if cosine == 0.0 {
        return Ok(None);
    }
    let pdf = material.pdf(wi, wo, normal);
    if !pdf.is_finite() || pdf <= 0.0 {
        return Err("漫反射采样 PDF 必须有限且为正".into());
    }
    let BsdfValue::Rgb(bsdf) = material.evaluate(wi, wo, normal, EvaluateMode::Rgb) else {
        return Err("漫反射 RGB 求值必须返回 RGB BSDF".into());
    };
    if bsdf.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err("漫反射 BSDF 必须有限且非负".into());
    }
    let scale = scene
        .ground_reflectance_scale(hit)
        .ok_or("漫反射棋盘格坐标无效")?;
    let throughput = bsdf * (cosine / pdf) * scale;
    if throughput.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err("漫反射吞吐量计算溢出".into());
    }
    if throughput == Vector3::zeros() {
        return Ok(None);
    }
    let origin = offset_ray_origin(hit.coords, normal, 1.0);
    let ray = Ray::new(origin, wi).ok_or("漫反射射线起点或方向无效")?;
    Ok(Some(DiffuseRay { ray, throughput }))
}

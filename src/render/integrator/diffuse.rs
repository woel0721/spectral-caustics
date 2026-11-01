//! 有界迭代 Lambertian 路径、RGB 吞吐量与后续顶点的直接光。

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
use std::error::Error;

/// 迭代 A→B→C…，只接收后续 Lambertian 顶点的太阳/环境直接光。
/// 不读背景、旧完整地面图或焦散；玻璃和不支持的材质终止漫反射链。
pub(super) fn trace_diffuse_path(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn rand::RngCore,
    state: &mut CameraPathState,
) -> Result<(), Box<dyn Error>> {
    let prefix_throughput = state.throughput;
    let Some(first) = sample_diffuse_ray(scene, hit, material, -state.ray.direction, rng)? else {
        return Ok(());
    };
    state.ray = first.ray;
    state.throughput = prefix_throughput.component_mul(&first.throughput);
    state.diffuse_bounces = 1;
    loop {
        if !state.throughput.iter().all(|v| v.is_finite() && *v >= 0.0) {
            return Err("漫反射路径吞吐量计算溢出".into());
        }
        let Some(next) = scene.intersect(&state.ray) else {
            break;
        };
        let Some(material) = next.material_id.and_then(|id| scene.material(id)) else {
            break;
        };
        if !material.is_lambertian() || material.is_delta() {
            break;
        }
        let normal = unit_vector(next.normal).ok_or("第二漫反射表面法线无效")?;
        let wo = -state.ray.direction;
        if normal.dot(&wo) <= 0.0 {
            break;
        }
        let direct = secondary_direct_lighting(scene, &next, material, wo, light, ground, rng)?;
        // 深度 1 严格保留 E1 的 (T_diffuse ⊙ L_next) * T_glass 分组。
        let contribution = if state.max_diffuse_bounces == 1 {
            first
                .throughput
                .component_mul(&direct)
                .component_mul(&prefix_throughput)
        } else {
            state.throughput.component_mul(&direct)
        };
        state.radiance.direct += contribution;
        if !state.radiance.direct.iter().all(|v| v.is_finite()) {
            return Err("漫反射间接光计算溢出".into());
        }
        if state.diffuse_bounces >= state.max_diffuse_bounces {
            break;
        }
        let Some(next_ray) = sample_diffuse_ray(scene, &next, material, wo, rng)? else {
            break;
        };
        state.throughput = state.throughput.component_mul(&next_ray.throughput);
        if state.throughput == Vector3::zeros() {
            break;
        }
        state.ray = next_ray.ray;
        state.diffuse_bounces += 1;
    }
    Ok(())
}

/// 各深度共用一份着色逻辑；返回的 L 已含本顶点的 BSDF 和一次棋盘格调制。
fn secondary_direct_lighting(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn rand::RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let direct = if let Some(g) = ground.filter(|g| hit.object_id == Some(g.object_id))
        && let Some(sun) = &g.sun
    {
        let e = direct_ground_irradiance(scene, hit, sun.wi, sun.irradiance_xyz)?;
        xyz_to_linear_srgb(lambertian_radiance(e, g.rho).ok_or("第二表面地面直射计算失败")?)
            .ok_or("第二表面地面 XYZ 转换失败")?
    } else {
        // 旧完整地面图无法分离焦散，E1 不读取它，改用现有解析 RGB 太阳。
        surface_sun_lighting(scene, hit, material, wo, light, ground)?
    };
    let environment = environment_lighting(scene, hit, material, wo, rng)?;
    let scale = scene
        .ground_reflectance_scale(hit)
        .ok_or("第二表面棋盘格坐标无效")?;
    Ok((direct + environment) * scale)
}

/// 原 E1 单连接测试入口，固定深度 1；生产只调用迭代路径。
#[cfg(test)]
pub(super) fn sample_one_diffuse_indirect(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn rand::RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let ray = Ray::new(hit.coords, -wo).ok_or("漫反射射线起点或方向无效")?;
    let mut state = CameraPathState::new(ray, 1);
    trace_diffuse_path(scene, hit, material, light, ground, rng, &mut state)?;
    Ok(state.radiance.direct)
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

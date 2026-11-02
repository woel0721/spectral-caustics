//! 通用 BSDF 单步采样、后续顶点直接光与 Russian Roulette；无生产路径循环。

use super::{
    direct::surface_sun_lighting,
    environment::environment_lighting,
    ground::{GroundLighting, direct_ground_irradiance, lambertian_radiance},
    offset_ray_origin,
    types::{CameraPathState, ScatterRay},
};
use crate::{
    geometry::{hit::Hit, ray::Ray},
    material::{Material, unit_vector},
    render::utils::xyz_to_linear_srgb,
    scene::Scene,
};
use nalgebra::Vector3;
use rand::Rng;
use std::error::Error;

/// 第三次连续散射后的顶点已着色，且还有预算时，RR 只控制之后的贡献。
/// 使用真实路径 RGB 吞吐量（含玻璃前缀）；存活补偿不缩放已累计辐亮度。
pub(super) fn russian_roulette(
    state: &mut CameraPathState,
    rng: &mut dyn rand::RngCore,
) -> Result<bool, Box<dyn Error>> {
    if state.non_delta_bounces < 3 || state.non_delta_bounces >= state.max_non_delta_bounces {
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

/// 所有事件统一使用材质返回的权重；不在积分器中再次计算 f*cos/pdf。
pub(super) fn sample_scatter_ray(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    rng: &mut dyn rand::RngCore,
) -> Result<Option<ScatterRay>, Box<dyn Error>> {
    let normal = unit_vector(hit.normal).ok_or("BSDF 采样法线无效")?;
    let Some(sample) = material.sample(wo, normal, rng) else {
        return Ok(None);
    };
    let wi = unit_vector(sample.wi).ok_or("BSDF 采样方向无效")?;
    if sample.event.is_delta() != sample.pdf.is_none()
        || sample.pdf.is_some_and(|p| !p.is_finite() || p <= 0.0)
        || !sample.weight.iter().all(|v| v.is_finite() && *v >= 0.0)
    {
        return Err("BSDF 采样权重、事件或立体角 PDF 无效".into());
    }
    if sample.event.is_delta() && !material.has_delta()
        || !sample.event.is_delta() && !material.has_non_delta()
    {
        return Err("BSDF 采样事件与声明的分量不一致".into());
    }
    // 当前连续材质均为单面反射；背面/切线方向是零贡献，不补采。
    if !sample.event.is_delta() && (normal.dot(&wi) <= 0.0 || normal.dot(&wo) <= 0.0) {
        return Ok(None);
    }
    let scale = scene
        .ground_reflectance_scale(hit)
        .ok_or("棋盘格坐标无效")?;
    let throughput = sample.weight * scale;
    if throughput == Vector3::zeros() && !sample.event.is_delta() {
        return Ok(None);
    }
    let side = if wi.dot(&normal) >= 0.0 { 1.0 } else { -1.0 };
    let ray = Ray::new(offset_ray_origin(hit.coords, normal, side), wi)
        .ok_or("BSDF 射线起点或方向无效")?;
    Ok(Some(ScatterRay {
        ray,
        throughput,
        event: sample.event,
    }))
}

//! E1 单次 Lambertian 连接、RGB 吞吐量与第二表面的直接光。

use super::{
    direct::surface_sun_lighting,
    environment::environment_lighting,
    ground::{GroundLighting, SunGroundLighting, ground_irradiance, lambertian_radiance},
    types::{DiffuseRay, GroundLightingComponent},
};
use crate::{
    geometry::{
        hit::Hit,
        ray::{INTERSECTION_EPSILON, Ray},
    },
    material::{BsdfValue, EvaluateMode, Material, MaterialSample, unit_vector},
    render::utils::xyz_to_linear_srgb,
    scene::Scene,
};
use nalgebra::Vector3;
use std::error::Error;

/// 仅采样 A→B；miss/delta/不支持的材质为零，不查询背景或 B 的焦散图。
pub(super) fn sample_one_diffuse_indirect(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn rand::RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let Some(DiffuseRay { ray, throughput }) = sample_diffuse_ray(scene, hit, material, wo, rng)?
    else {
        return Ok(Vector3::zeros());
    };
    let Some(next) = scene.intersect(&ray) else {
        // A 的 HDRI 已由 NEE/MIS 估计，不能再无权重添加逃逸方向的背景。
        return Ok(Vector3::zeros());
    };
    let Some(next_material) = next.material_id.and_then(|id| scene.material(id)) else {
        return Ok(Vector3::zeros());
    };
    if !next_material.is_lambertian() || next_material.is_delta() {
        return Ok(Vector3::zeros());
    }
    let normal = unit_vector(next.normal).ok_or("第二漫反射表面法线无效")?;
    let wo_next = -ray.direction;
    if normal.dot(&wo_next) <= 0.0 {
        return Ok(Vector3::zeros());
    }
    let direct = if let Some(g) = ground.filter(|g| next.object_id == Some(g.object_id))
        && let Some(sun) = &g.sun
    {
        // 复用地面直射分支，仅查询解析太阳；不读取焦散专用图。
        let direct_ground = GroundLighting {
            sun: Some(SunGroundLighting {
                component: GroundLightingComponent::Direct,
                ..*sun
            }),
            ..*g
        };
        let e = ground_irradiance(scene, &next, &direct_ground)?;
        xyz_to_linear_srgb(
            lambertian_radiance(e.direct_xyz, g.rho).ok_or("第二表面地面直射计算失败")?,
        )
        .ok_or("第二表面地面 XYZ 转换失败")?
    } else {
        // 旧完整地面图无法分离焦散，E1 不读取它，改用现有解析 RGB 太阳。
        surface_sun_lighting(scene, &next, next_material, wo_next, light, ground)?
    };
    let environment = environment_lighting(scene, &next, next_material, wo_next, rng)?;
    let scale = scene
        .ground_reflectance_scale(&next)
        .ok_or("第二表面棋盘格坐标无效")?;
    let indirect = throughput.component_mul(&((direct + environment) * scale));
    if !indirect.iter().all(|v| v.is_finite()) {
        return Err("漫反射间接光计算溢出".into());
    }
    Ok(indirect)
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
    let origin = hit.coords + normal * (10.0 * INTERSECTION_EPSILON);
    let ray = Ray::new(origin, wi).ok_or("漫反射射线起点或方向无效")?;
    Ok(Some(DiffuseRay { ray, throughput }))
}

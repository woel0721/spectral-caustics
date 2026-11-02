//! 通用 RGB 相机路径：按 BSDF 事件推进，局部 NEE 与 Delta 链逃逸分开计光。
use super::{
    diffuse::{russian_roulette, sample_scatter_ray, secondary_direct_lighting},
    direct::surface_sun_lighting,
    environment::environment_lighting,
    ground::{GroundLighting, ground_irradiance, lambertian_radiance},
    types::{CameraPathRngs, CameraPathState},
};
use crate::{
    geometry::hit::Hit,
    material::{Material, unit_vector},
    render::utils::xyz_to_linear_srgb,
    scene::Scene,
};
use nalgebra::Vector3;
use std::error::Error;

pub(super) const MAX_DELTA_EVENTS: usize = 8;

pub(super) fn trace_camera_path(
    scene: &Scene,
    state: &mut CameraPathState,
    rngs: &mut CameraPathRngs<'_>,
    light_wi: Vector3<f64>,
    light: f64,
    ground: Option<&GroundLighting<'_>>,
) -> Result<(), Box<dyn Error>> {
    crate::config::sceneconfig::validate_diffuse_bounces(scene.max_diffuse_bounces)?;
    loop {
        if !state.throughput.iter().all(|v| v.is_finite() && *v >= 0.0) {
            return Err("路径吞吐量必须有限且非负".into());
        }
        if state.throughput == Vector3::zeros() {
            break;
        }
        let Some(hit) = scene.intersect(&state.ray) else {
            if state.non_delta_bounces == 0 {
                let background = scene
                    .background_radiance(state.ray.direction)
                    .ok_or("天空或背景颜色无效")?;
                state.radiance.direct += state.throughput.component_mul(&background);
            } else if state.last_scatter.is_some_and(|event| event.is_delta())
                && let Some(environment) = &scene.environment
            {
                // 局部 NEE 的二值遮挡无法生成这条 Delta 链，完整 weight 已包含事件概率/RR。
                let li = environment
                    .radiance(state.ray.direction)
                    .ok_or("环境辐亮度无效")?;
                state.radiance.direct += state.throughput.component_mul(&li);
            }
            // 非 Delta miss 由该顶点的局部环境 NEE/MIS 估计，不能再次加背景。
            break;
        };
        let Some(material) = hit.material_id.and_then(|id| scene.material(id)) else {
            break;
        };
        let wo = -state.ray.direction;
        if material.has_non_delta() {
            let normal = unit_vector(hit.normal).ok_or("表面法线无效")?;
            if normal.dot(&wo) <= 0.0 && !material.has_delta() {
                break;
            }
            if state.has_non_delta_vertex || state.non_delta_bounces > 0 {
                let direct = secondary_direct_lighting(
                    scene,
                    &hit,
                    material,
                    wo,
                    (light_wi, light),
                    ground,
                    rngs.diffuse,
                )?;
                state.radiance.direct += state.throughput.component_mul(&direct);
            } else {
                let scale = scene
                    .ground_reflectance_scale(&hit)
                    .ok_or("地面棋盘格坐标无效")?;
                let env = environment_lighting(scene, &hit, material, wo, rngs.environment)?;
                let radiance = first_surface_lighting(
                    scene,
                    &hit,
                    material,
                    wo,
                    env,
                    (light_wi, light),
                    ground,
                )?;
                state.radiance.direct += state.throughput.component_mul(&radiance.direct) * scale;
                state.radiance.caustics +=
                    state.throughput.component_mul(&radiance.caustics) * scale;
            }
        }
        // 记录访问过的连续顶点，混合材质抽到 Delta 时也不能重读焦散图。
        state.has_non_delta_vertex |= material.has_non_delta();
        let continuous_budget = state.non_delta_bounces < state.max_non_delta_bounces;
        let delta_budget = state.delta_events() < MAX_DELTA_EVENTS;
        if (!continuous_budget || !material.has_non_delta())
            && (!delta_budget || !material.has_delta())
        {
            break;
        }
        // 以非 Delta 连接数量控制 RR，保留浅层/终点不抽样的约定。
        if material.has_non_delta() && continuous_budget && !russian_roulette(state, rngs.diffuse)?
        {
            break;
        }
        let rng = if state.non_delta_bounces == 0 && !state.has_non_delta_vertex {
            &mut *rngs.camera
        } else {
            &mut *rngs.diffuse
        };
        let Some(next) = sample_scatter_ray(scene, &hit, material, wo, rng)? else {
            break;
        };
        // 混合材质按完整概率采样；没有预算的分量终止，不能重新选择另一个分量。
        if next.event.is_delta() && !delta_budget || !next.event.is_delta() && !continuous_budget {
            break;
        }
        if !state.apply_scatter(next.event, next.throughput)? {
            break;
        }
        state.ray = next.ray;
    }
    if !state
        .radiance
        .direct
        .iter()
        .chain(state.radiance.caustics.iter())
        .all(|v| v.is_finite())
    {
        return Err("路径颜色非法或计算溢出".into());
    }
    Ok(())
}

/// 首次表面可接收焦散；后续顶点仅使用 secondary_direct_lighting。
fn first_surface_lighting(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    environment: Vector3<f64>,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
) -> Result<super::types::CameraPathComponents, Box<dyn Error>> {
    use super::types::CameraPathComponents;
    if let Some(ground) = ground.filter(|g| hit.object_id == Some(g.object_id)) {
        let normal = unit_vector(hit.normal).ok_or("地面法线无效")?;
        if normal.dot(&wo) <= 0.0 {
            return Ok(CameraPathComponents::default());
        }
        let e = ground_irradiance(scene, hit, ground)?;
        let direct =
            lambertian_radiance(e.direct_xyz, ground.rho).ok_or("地面直射辐亮度计算失败")?;
        let caustics =
            lambertian_radiance(e.caustics_xyz, ground.rho).ok_or("地面焦散辐亮度计算失败")?;
        return Ok(CameraPathComponents {
            direct: xyz_to_linear_srgb(direct).ok_or("地面 XYZ 辐亮度转换失败")? + environment,
            caustics: xyz_to_linear_srgb(caustics).ok_or("地面焦散 XYZ 辐亮度转换失败")?,
        });
    }
    Ok(CameraPathComponents {
        direct: surface_sun_lighting(scene, hit, material, wo, light, ground)? + environment,
        ..Default::default()
    })
}

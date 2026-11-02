//! 统一的相机路径迭代：玻璃前缀、有界 Lambertian 链与即时表面照明。

use super::{
    diffuse::{russian_roulette, sample_diffuse_ray, secondary_direct_lighting},
    direct::surface_sun_lighting,
    environment::environment_lighting,
    ground::{GroundLighting, ground_irradiance, lambertian_radiance},
    offset_ray_origin,
    types::{CameraPathRngs, CameraPathState},
};
use crate::{
    geometry::{hit::Hit, ray::Ray},
    material::{Material, MaterialSample, unit_vector},
    render::utils::xyz_to_linear_srgb,
    scene::Scene,
};
use nalgebra::Vector3;
use std::error::Error;

pub(super) const MAX_GLASS_BOUNCES: usize = 8;

/// 迭代推进相机路径，向 state 写入未曝光的 Direct/Caustics 线性 RGB 辐亮度。
///
/// 调用方为每条路径创建 CameraPathState；RNG 由像素采样循环持有，不能逐路径重设种子。
/// wo = -ray.direction，保留朝外几何法线；玻璃只乘 sample.weight。
/// 新起点沿采样方向所在侧的法线偏移 1e-5 米，第九个玻璃命中处截断。
/// 漫反射之前的 miss 查询背景；后续 miss 截断，避免重复计入环境。
/// 首次地面命中使用解析直射和焦散图，环境单独计入一次。
/// 首次 Lambertian 后迭代追踪有界漫反射链；后续仅累加太阳/环境直接光。
/// 漫反射后遇玻璃终止，不模拟透明 shadow ray。
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
        let after_diffuse = state.diffuse_bounces > 0;
        if after_diffuse && !state.throughput.iter().all(|v| v.is_finite() && *v >= 0.0) {
            return Err("漫反射路径吞吐量计算溢出".into());
        }
        let Some(hit) = scene.intersect(&state.ray) else {
            if !after_diffuse {
                state.radiance.direct += state.throughput.component_mul(
                    &scene
                        .background_radiance(state.ray.direction)
                        .ok_or("背景辐亮度无效")?,
                );
            }
            break;
        };
        let Some(material) = hit.material_id.and_then(|id| scene.material(id)) else {
            break;
        };
        let wo = -state.ray.direction;
        if after_diffuse {
            // 漫反射后的玻璃路径不在本估计器范围内。
            if !material.is_lambertian() || material.is_delta() {
                break;
            }
            let normal = unit_vector(hit.normal).ok_or("第二漫反射表面法线无效")?;
            if normal.dot(&wo) <= 0.0 {
                break;
            }
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
            if !state.radiance.direct.iter().all(|v| v.is_finite()) {
                return Err("漫反射间接光计算溢出".into());
            }
            if state.diffuse_bounces >= state.max_diffuse_bounces {
                break;
            }
            if !russian_roulette(state, rngs.diffuse)? {
                break;
            }
        } else {
            let reflectance_scale = if material.is_delta() {
                1.0
            } else {
                scene
                    .ground_reflectance_scale(&hit)
                    .ok_or("地面棋盘格坐标无效")?
            };
            let environment = environment_lighting(scene, &hit, material, wo, rngs.environment)?;
            if !material.is_delta() || ground.is_some_and(|g| hit.object_id == Some(g.object_id)) {
                let radiance = first_surface_lighting(
                    scene,
                    &hit,
                    material,
                    wo,
                    environment,
                    (light_wi, light),
                    ground,
                )?;
                state.radiance.direct +=
                    state.throughput.component_mul(&radiance.direct) * reflectance_scale;
                state.radiance.caustics +=
                    state.throughput.component_mul(&radiance.caustics) * reflectance_scale;
                if state.max_diffuse_bounces == 0 {
                    break;
                }
            } else {
                // 只有未发生漫反射的 delta 顶点才能推进玻璃事件。
                if state.glass_events == MAX_GLASS_BOUNCES {
                    break;
                }
                let normal_length = hit.normal.norm();
                if !normal_length.is_finite() || normal_length == 0.0 {
                    return Err("命中法线无效，无法采样玻璃路径".into());
                }
                let normal = hit.normal / normal_length;
                let Some(sample) = material.sample(wo, normal, rngs.camera) else {
                    break;
                };
                let MaterialSample::Glass(sample) = sample else {
                    return Err("delta 材质必须返回玻璃离散事件采样".into());
                };
                if !sample.probability.is_finite()
                    || sample.probability <= 0.0
                    || sample.probability > 1.0
                    || !sample.weight.is_finite()
                    || sample.weight < 0.0
                {
                    return Err("玻璃采样概率或权重无效".into());
                }
                state.throughput *= sample.weight;
                state.glass_events += 1;
                if !state.throughput.iter().all(|value| value.is_finite()) {
                    return Err("玻璃路径吞吐量计算溢出".into());
                }
                if state.throughput == Vector3::zeros() {
                    break;
                }
                let side = if sample.wi.dot(&normal) >= 0.0 {
                    1.0
                } else {
                    -1.0
                };
                let origin = offset_ray_origin(hit.coords, normal, side);
                state.ray = Ray::new(origin, sample.wi).ok_or("玻璃路径射线起点或方向无效")?;
                continue;
            }
        }
        // 所有 Lambertian 顶点共用采样、吞吐量和事件推进。
        let Some(next) = sample_diffuse_ray(scene, &hit, material, wo, rngs.diffuse)? else {
            break;
        };
        state.throughput = state.throughput.component_mul(&next.throughput);
        if state.throughput == Vector3::zeros() {
            break;
        }
        state.ray = next.ray;
        state.diffuse_bounces += 1;
    }
    if !state
        .radiance
        .direct
        .iter()
        .chain(state.radiance.caustics.iter())
        .all(|value| value.is_finite())
    {
        return Err("玻璃路径颜色非法或计算溢出".into());
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

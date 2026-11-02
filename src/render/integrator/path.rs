//! 统一的相机路径迭代：玻璃前缀、有界 Lambertian 链与兼容着色顺序。

use super::{
    diffuse::{russian_roulette, sample_diffuse_ray, secondary_direct_lighting},
    direct::surface_sun_lighting,
    environment::environment_lighting,
    ground::{GroundLighting, ground_irradiance, lambertian_radiance},
    offset_ray_origin,
    types::{CameraPathRngs, CameraPathState, GroundLightingComponent},
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

/// E2 在后续链结束后才计算并累加首次表面的原照明；只暂存一个顶点。
struct FirstSurface<'a> {
    hit: Hit,
    material: &'a dyn Material,
    wo: Vector3<f64>,
    environment: Vector3<f64>,
    reflectance_scale: f64,
    throughput: f64,
}

/// 迭代推进相机路径，向 state 写入未曝光的 Direct/Caustics 线性 RGB 辐亮度。
///
/// 调用方为每条路径创建 CameraPathState；RNG 由像素采样循环持有，不能逐路径重设种子。
/// wo = -ray.direction，保留朝外几何法线；玻璃只乘 sample.weight。
/// 新起点沿采样方向所在侧的法线偏移 1e-5 米，第九个玻璃命中处截断。
/// 漫反射之前的 miss 查询背景；后续 miss 截断，避免重复计入环境。
/// 阶段 13 绑定只读完整地面图；阶段 14 绑定将解析直射和焦散图相加。
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
    let mut first_surface = None;
    // 天空及可见背景归入 Direct；不改相机 RNG，三张调试图共享相同玻璃路径。
    let include_environment = ground
        .and_then(|g| g.sun.as_ref())
        .is_none_or(|sun| sun.component != GroundLightingComponent::Caustics);
    loop {
        let after_diffuse = state.diffuse_bounces > 0;
        if after_diffuse && !state.throughput.iter().all(|v| v.is_finite() && *v >= 0.0) {
            return Err("漫反射路径吞吐量计算溢出".into());
        }
        let Some(hit) = scene.intersect(&state.ray) else {
            if !after_diffuse && include_environment {
                state.radiance.direct += scene
                    .background_radiance(state.ray.direction)
                    .ok_or("背景辐亮度无效")?
                    * state.throughput.x;
            }
            break;
        };
        let Some(material) = hit.material_id.and_then(|id| scene.material(id)) else {
            break;
        };
        let wo = -state.ray.direction;
        if after_diffuse {
            // F1 仍截断 Diffuse → Glass；不得采样玻璃或消费 camera/environment 流。
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
            let contribution = if let Some((first, prefix)) = &state.e1_diffuse_weights {
                // 深度 1 保留 (T_diffuse ⊙ L_next) ⊙ T_glass 的原浮点分组。
                first.component_mul(&direct).component_mul(prefix)
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
            // 与 E2 一样，先采样首次环境，再推进整个漫反射链，最后计算首次直接光。
            let environment = if include_environment {
                environment_lighting(scene, &hit, material, wo, rngs.environment)?
            } else {
                Vector3::zeros()
            };
            if !material.is_delta() || ground.is_some_and(|g| hit.object_id == Some(g.object_id)) {
                first_surface = Some(FirstSurface {
                    hit,
                    material,
                    wo,
                    environment,
                    reflectance_scale,
                    throughput: state.throughput.x,
                });
                if state.max_diffuse_bounces == 0 || !include_environment {
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
        // 首次与后续 Lambertian 共用采样、吞吐量和事件推进；保留首步的零值检查时机。
        let Some(next) = sample_diffuse_ray(scene, &hit, material, wo, rngs.diffuse)? else {
            break;
        };
        if !after_diffuse && state.max_diffuse_bounces == 1 {
            state.e1_diffuse_weights = Some((next.throughput, state.throughput));
        }
        state.throughput = state.throughput.component_mul(&next.throughput);
        if after_diffuse && state.throughput == Vector3::zeros() {
            break;
        }
        state.ray = next.ray;
        state.diffuse_bounces += 1;
    }
    if let Some(first) = first_surface {
        accumulate_first_surface(scene, first, (light_wi, light), ground, state)?;
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

/// 首次地面仍可读原图/焦散图；整个后续链结束后才执行，保留错误和加法顺序。
fn accumulate_first_surface(
    scene: &Scene,
    first: FirstSurface<'_>,
    (light_wi, light): (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    state: &mut CameraPathState,
) -> Result<(), Box<dyn Error>> {
    let FirstSurface {
        hit,
        material,
        wo,
        environment,
        reflectance_scale,
        throughput,
    } = first;
    if let Some(ground) = ground
        && hit.object_id == Some(ground.object_id)
    {
        let length = hit.normal.norm();
        if !length.is_finite() || length == 0.0 {
            return Err("地面法线无效".into());
        }
        // 当前地面是单面 Lambertian，朝外法线指向已存储辐照度的那一侧。
        if (hit.normal / length).dot(&wo) <= 0.0 {
            return Ok(());
        }
        let e = ground_irradiance(scene, &hit, ground)?;
        let direct_xyz =
            lambertian_radiance(e.direct_xyz, ground.rho).ok_or("地面直射辐亮度计算失败")?;
        let caustics_xyz =
            lambertian_radiance(e.caustics_xyz, ground.rho).ok_or("地面焦散辐亮度计算失败")?;
        // 保留超出 sRGB 色域的线性值，最终输出时才裁剪；中性反射直接缩放 XYZ。
        let weight = reflectance_scale * throughput;
        state.radiance.direct +=
            (xyz_to_linear_srgb(direct_xyz).ok_or("地面 XYZ 辐亮度转换失败")? + environment)
                * weight;
        state.radiance.caustics +=
            xyz_to_linear_srgb(caustics_xyz).ok_or("地面焦散 XYZ 辐亮度转换失败")? * weight;
        return Ok(());
    }
    let direct = surface_sun_lighting(scene, &hit, material, wo, (light_wi, light), ground)?;
    state.radiance.direct += (direct + environment) * (reflectance_scale * throughput);
    Ok(())
}

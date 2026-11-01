//! 相机路径控制流、Fresnel 玻璃事件与八次玻璃传播上限。

use super::{
    diffuse::sample_one_diffuse_indirect,
    direct::surface_sun_lighting,
    environment::environment_lighting,
    ground::{GroundLighting, ground_irradiance, lambertian_radiance},
    types::{CameraPathComponents, CameraPathRngs, GroundLightingComponent},
};
use crate::{
    geometry::ray::{INTERSECTION_EPSILON, Ray},
    material::MaterialSample,
    render::utils::xyz_to_linear_srgb,
    scene::Scene,
};
use nalgebra::Vector3;
use std::error::Error;

pub(super) const MAX_GLASS_BOUNCES: usize = 8;

/// 追踪一条相机射线，返回未曝光的 Direct/Caustics 线性 RGB 辐亮度。
///
/// 每条路径重新初始化吞吐量与辐亮度；RNG 由像素采样循环持有，不能逐路径重设种子。
/// wo = -ray.direction，保留朝外几何法线；玻璃只乘 sample.weight。
/// 新起点沿采样方向所在侧的法线偏移 1e-5 米，第九个玻璃命中处截断。
/// 未命中按当前射线方向查询背景；非 delta 命中累加太阳与天空照明。
/// 阶段 13 绑定只读完整地面图；阶段 14 绑定将解析直射和焦散图相加。
/// E1 可额外连接一个 Lambertian，仅将第二表面的直接光照计入 Direct。
/// 不续追第二表面后的路径，不模拟透明 shadow ray。
pub(super) fn trace_camera_path(
    scene: &Scene,
    mut ray: Ray,
    rngs: &mut CameraPathRngs<'_>,
    light_wi: Vector3<f64>,
    light: f64,
    ground: Option<&GroundLighting<'_>>,
) -> Result<CameraPathComponents, Box<dyn Error>> {
    if scene.max_diffuse_bounces > 1 {
        return Err("E1 漫反射反弹数只能为 0 或 1".into());
    }
    let mut throughput = 1.0;
    let mut radiance = CameraPathComponents::default();
    // 天空及可见背景归入 Direct；不改相机 RNG，三张调试图共享相同玻璃路径。
    let include_environment = ground
        .and_then(|g| g.sun.as_ref())
        .is_none_or(|sun| sun.component != GroundLightingComponent::Caustics);
    for bounce in 0..=MAX_GLASS_BOUNCES {
        let Some(hit) = scene.intersect(&ray) else {
            if include_environment {
                radiance.direct += scene
                    .background_radiance(ray.direction)
                    .ok_or("背景辐亮度无效")?
                    * throughput;
            }
            break;
        };
        let Some(material) = hit.material_id.and_then(|id| scene.material(id)) else {
            break;
        };
        let wo = -ray.direction;
        let reflectance_scale = if material.is_delta() {
            1.0
        } else {
            scene
                .ground_reflectance_scale(&hit)
                .ok_or("地面棋盘格坐标无效")?
        };
        let environment = if include_environment {
            environment_lighting(scene, &hit, material, wo, rngs.environment)?
        } else {
            Vector3::zeros()
        };
        if scene.max_diffuse_bounces == 1 && include_environment {
            radiance.direct += sample_one_diffuse_indirect(
                scene,
                &hit,
                material,
                wo,
                (light_wi, light),
                ground,
                rngs.diffuse,
            )? * throughput;
        }
        if let Some(ground) = ground
            && hit.object_id == Some(ground.object_id)
        {
            let length = hit.normal.norm();
            if !length.is_finite() || length == 0.0 {
                return Err("地面法线无效".into());
            }
            // 当前地面是单面 Lambertian，朝外法线指向已存储辐照度的那一侧。
            if (hit.normal / length).dot(&wo) <= 0.0 {
                break;
            }
            let e = ground_irradiance(scene, &hit, ground)?;
            let direct_xyz =
                lambertian_radiance(e.direct_xyz, ground.rho).ok_or("地面直射辐亮度计算失败")?;
            let caustics_xyz =
                lambertian_radiance(e.caustics_xyz, ground.rho).ok_or("地面焦散辐亮度计算失败")?;
            // 保留超出 sRGB 色域的线性值，最终输出时才裁剪；中性反射直接缩放 XYZ。
            let weight = reflectance_scale * throughput;
            radiance.direct += (xyz_to_linear_srgb(direct_xyz).ok_or("地面 XYZ 辐亮度转换失败")?
                + environment)
                * weight;
            radiance.caustics +=
                xyz_to_linear_srgb(caustics_xyz).ok_or("地面焦散 XYZ 辐亮度转换失败")? * weight;
            break;
        }
        if !material.is_delta() {
            let direct =
                surface_sun_lighting(scene, &hit, material, wo, (light_wi, light), ground)?;
            radiance.direct += (direct + environment) * (reflectance_scale * throughput);
            break;
        }
        if bounce == MAX_GLASS_BOUNCES {
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
        throughput *= sample.weight;
        if !throughput.is_finite() {
            return Err("玻璃路径吞吐量计算溢出".into());
        }
        if throughput == 0.0 {
            break;
        }
        let side = if sample.wi.dot(&normal) >= 0.0 {
            1.0
        } else {
            -1.0
        };
        let origin = hit.coords + normal * (side * 10.0 * INTERSECTION_EPSILON);
        ray = Ray::new(origin, sample.wi).ok_or("玻璃路径射线起点或方向无效")?;
    }
    if !radiance
        .direct
        .iter()
        .chain(radiance.caustics.iter())
        .all(|value| value.is_finite())
    {
        return Err("玻璃路径颜色非法或计算溢出".into());
    }
    Ok(radiance)
}

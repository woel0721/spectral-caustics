// Frozen E1 estimator from e664004; test-only compatibility oracle.
use super::*;

pub(super) fn trace_camera_path_e1(
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
        let origin = offset_ray_origin(hit.coords, normal, side);
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
        let e = direct_ground_irradiance(scene, &next, sun.wi, sun.irradiance_xyz)?;
        xyz_to_linear_srgb(lambertian_radiance(e, g.rho).ok_or("第二表面地面直射计算失败")?)
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

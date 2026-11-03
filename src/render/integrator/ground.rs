//! 地面辐照度采样、材质/光源图绑定及 Direct/Caustics 查询。

use super::direct::visible_to_light;
use crate::{
    geometry::{hit::Hit, ray::PARALLEL_EPSILON},
    render::{
        caustics::{SpectralLighting, ground_to_pixel},
        framebuffer::Framebuffer,
        utils::bilinear_weights,
    },
    scene::{GroundReceiver, Scene},
};
use nalgebra::Vector3;
use std::{error::Error, f64::consts::PI};

#[derive(Default)]
pub(super) struct GroundIrradianceComponents {
    pub(super) direct_xyz: Vector3<f64>,
    pub(super) caustics_xyz: Vector3<f64>,
}

/// 一次相机渲染中只读的地面绑定；反射率由现有材质 evaluate 接口读取一次。
pub(super) struct GroundLighting<'a> {
    pub(super) irradiance: &'a Framebuffer,
    pub(super) receiver: &'a GroundReceiver,
    pub(super) object_id: usize,
    pub(super) rho: f64,
    pub(super) light_wi: Vector3<f64>,
    pub(super) sun_irradiance_xyz: Vector3<f64>,
}

/// 从未曝光的地面 XYZ 辐照度图读取世界交点的 E，使用像素中心约定和双线性插值。
///
/// 接收区域、尺寸应与生成该图的光源渲染一致；使用世界 x/y，图片 y 向下。
/// 有效区域为 [min, max)，区域外返回零；区域内边缘夹紧到最近像素中心。
/// 非法尺寸/接收区域/交点，或被读取的像素含负值、非有限值时返回 None。
/// 图中已除以像素面积，使用相同的相对能量标尺；不再除面积或施加曝光、sRGB。
pub fn sample_ground_irradiance(
    irradiance: &Framebuffer,
    receiver: &GroundReceiver,
    position: Vector3<f64>,
) -> Option<Vector3<f64>> {
    if !valid_ground_map_layout(irradiance, receiver)
        || !position.iter().all(|value| value.is_finite())
    {
        return None;
    }
    let Some((x, y)) = ground_to_pixel(position, irradiance.width, irradiance.height, receiver)
    else {
        return Some(Vector3::zeros());
    };
    let mut xyz = Vector3::zeros();
    for (px, py, weight) in bilinear_weights(x, y, irradiance.width, irradiance.height)? {
        if weight == 0.0 {
            continue;
        }
        let pixel = irradiance.get(px, py)?;
        if pixel.iter().any(|value| !value.is_finite() || *value < 0.0) {
            return None;
        }
        xyz += pixel * weight;
    }
    xyz.iter().all(|value| value.is_finite()).then_some(xyz)
}

/// 中性 Lambertian：L_o,XYZ = E_XYZ * rho / pi；rho 是 [0,1] 内的灰色反射率。
/// E 已对入射方向积分，不再乘入射余弦、光源强度或可见性。
/// 保留 HDR 线性值；非法输入或计算溢出返回 None。
pub fn lambertian_radiance(irradiance_xyz: Vector3<f64>, rho: f64) -> Option<Vector3<f64>> {
    if irradiance_xyz
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        || !rho.is_finite()
        || !(0.0..=1.0).contains(&rho)
    {
        return None;
    }
    let radiance = irradiance_xyz * (rho / PI);
    radiance
        .iter()
        .all(|value| value.is_finite())
        .then_some(radiance)
}

fn valid_ground_map_layout(irradiance: &Framebuffer, receiver: &GroundReceiver) -> bool {
    let span = receiver.max_m - receiver.min_m;
    irradiance.width > 0
        && irradiance.height > 0
        && irradiance.width.checked_mul(irradiance.height) == Some(irradiance.pixels.len())
        && receiver
            .min_m
            .iter()
            .chain(receiver.max_m.iter())
            .all(|value| value.is_finite())
        && span.iter().all(|value| value.is_finite() && *value > 0.0)
}

pub(super) fn bind_spectral_ground_lighting<'a>(
    scene: &Scene,
    lighting: &'a SpectralLighting,
) -> Result<GroundLighting<'a>, Box<dyn Error>> {
    if lighting.light_wi.iter().any(|v| !v.is_finite())
        || (lighting.light_wi.norm() - 1.0).abs() > 1e-12
        || lighting.light_wi.z <= PARALLEL_EPSILON
        || lighting
            .sun_irradiance_xyz
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
    {
        return Err("解析太阳的方向或 XYZ 辐照度无效".into());
    }
    let irradiance = &lighting.caustics;
    let receiver = &lighting.receiver;
    if !valid_ground_map_layout(irradiance, receiver)
        || irradiance
            .pixels
            .iter()
            .any(|pixel| pixel.iter().any(|v| !v.is_finite() || *v < 0.0))
    {
        return Err("地面辐照度图的尺寸、接收区域或 XYZ 数据无效".into());
    }
    let object_id = scene
        .ground_id
        .ok_or("场景缺少 ground_id，无法绑定地面辐照度图")?;
    let material = scene
        .objects
        .get(object_id)
        .and_then(|object| object.material_id())
        .and_then(|id| scene.material(id))
        .ok_or("地面编号或地面材质无效")?;
    let rho = material
        .irradiance_reflectance()
        .ok_or("辐照度贴图要求中性 Lambertian 地面")?;
    let bsdf = rho / PI;
    if bsdf
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0 || *v * PI > 1.0 + 1e-12)
        || (bsdf.x - bsdf.y).abs() > 1e-12
        || (bsdf.x - bsdf.z).abs() > 1e-12
    {
        return Err("当前地面辐照度贴图仅支持中性灰 Lambertian 反射率".into());
    }
    Ok(GroundLighting {
        irradiance,
        receiver,
        object_id,
        rho: (bsdf.x * PI).clamp(0.0, 1.0),
        light_wi: lighting.light_wi,
        sun_irradiance_xyz: lighting.sun_irradiance_xyz,
    })
}

pub(super) fn ground_irradiance(
    scene: &Scene,
    hit: &Hit,
    ground: &GroundLighting<'_>,
) -> Result<GroundIrradianceComponents, Box<dyn Error>> {
    Ok(GroundIrradianceComponents {
        direct_xyz: direct_ground_irradiance(
            scene,
            hit,
            ground.light_wi,
            ground.sun_irradiance_xyz,
        )?,
        // 焦散已包含折射路径可见性，不能用太阳 shadow ray 再次清零。
        caustics_xyz: if scene
            .caustics_object_id
            .or(scene.prism_id)
            .is_none_or(|id| scene.supports_ideal_caustics(id))
        {
            sample_ground_irradiance(ground.irradiance, ground.receiver, hit.coords)
                .ok_or("焦散辐照度采样失败")?
        } else {
            Vector3::zeros()
        },
    })
}

/// 只计算解析太阳的地面入射辐照度；不读取地面图，也不选择输出分量。
/// 调用者负责是否计入 Direct；材质 rho/pi 与棋盘格继续在着色处应用。
pub(super) fn direct_ground_irradiance(
    scene: &Scene,
    hit: &Hit,
    wi: Vector3<f64>,
    irradiance_xyz: Vector3<f64>,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let normal = hit.normal / hit.normal.norm();
    let cosine = normal.dot(&wi).clamp(0.0, 1.0);
    if cosine > 0.0
        && irradiance_xyz != Vector3::zeros()
        && visible_to_light(scene, hit.coords, normal, wi)?
    {
        Ok(irradiance_xyz * cosine)
    } else {
        Ok(Vector3::zeros())
    }
}

//! 地面辐照度采样、材质/光源图绑定及 Direct/Caustics 查询。

use crate::caustics::GroundReceiver;
use crate::caustics::SpectralLighting;
use crate::caustics::ground_to_pixel;
use crate::caustics::map::bilinear_weights;
use crate::config::RenderJob;
use crate::film::Framebuffer;
use crate::geometry::ray::PARALLEL_EPSILON;
use nalgebra::Vector3;
use std::error::Error;
use std::f64::consts::PI;

/// 一次 RGB 相机渲染中使用的已校验地面焦散图及光照参数。
pub struct GroundLighting<'a> {
    irradiance: &'a Framebuffer,
    receiver: &'a GroundReceiver,
    pub(super) object_id: usize,
    pub(super) rho: f64,
    pub(super) light_wi: Vector3<f64>,
    pub(super) sun_irradiance_rgb: Vector3<f64>,
}

impl GroundLighting<'_> {
    pub(super) fn sample_irradiance(&self, position: Vector3<f64>) -> Option<Vector3<f64>> {
        interpolate_ground_irradiance(self.irradiance, self.receiver, position, false)
    }
}

/// 按地面世界位置读取焦散图的 XYZ 辐照度，供 RGB 地面反射着色测试核对。
///
/// 输入：未曝光的 irradiance 图、生成它的 receiver 和 position（米）；图像尺寸/接收区须匹配。
///
/// 输出：E_XYZ=Σw_i E_i，范围 [min,max) 外返回零，非法布局/坐标/像素时 None。
/// 区域内边缘夹到最近像素中心；图已除像素面积，不再除面积、不施加曝光或 sRGB 编码。
#[cfg(test)]
pub(crate) fn sample_ground_irradiance(
    irradiance: &Framebuffer,
    receiver: &GroundReceiver,
    position: Vector3<f64>,
) -> Option<Vector3<f64>> {
    if !valid_ground_map_layout(irradiance, receiver) {
        return None;
    }
    interpolate_ground_irradiance(irradiance, receiver, position, true)
}

fn interpolate_ground_irradiance(
    irradiance: &Framebuffer,
    receiver: &GroundReceiver,
    position: Vector3<f64>,
    check_pixels: bool,
) -> Option<Vector3<f64>> {
    if !position.iter().all(|value| value.is_finite()) {
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
        if check_pixels && pixel.iter().any(|value| !value.is_finite() || *value < 0.0) {
            return None;
        }
        xyz += pixel * weight;
    }
    xyz.iter().all(|value| value.is_finite()).then_some(xyz)
}

/// 把地面收到的 XYZ 辐照度换算成朝相机反射的辐亮度，供中性漫反射地面测试核对。
///
/// 输入：非负有限 irradiance_xyz 和灰色反射率 ρ∈`[0,1]`。
///
/// 输出：L_o,XYZ=ρ·E_XYZ/π，非法输入或溢出返回 None，保留 HDR 线性值。
/// E 已对入射方向积分，不能再乘入射余弦、光源强度或可见性。
#[cfg(test)]
pub(crate) fn lambertian_radiance(irradiance_xyz: Vector3<f64>, rho: f64) -> Option<Vector3<f64>> {
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

/// 检查辐照度图与接收范围的布局，供地面绑定和采样使用。
///
/// 输入：Framebuffer/receiver；输出：宽高为正、像素数匹配、XY 边界、跨度和面积有效时 true。
fn valid_ground_map_layout(irradiance: &Framebuffer, receiver: &GroundReceiver) -> bool {
    irradiance.width > 0
        && irradiance.height > 0
        && irradiance.width.checked_mul(irradiance.height) == Some(irradiance.pixels.len())
        && receiver.checked_area().is_ok()
}

/// 校验 RGB 焦散图的接收地面绑定，返回 `(ground_id, neutral_rho)`。
pub(crate) fn validate_rgb_caustics_ground(
    scene: &RenderJob,
) -> Result<(usize, f64), Box<dyn Error + Send + Sync>> {
    let object_id = scene
        .photon
        .ground_id
        .ok_or("场景缺少 ground_id，无法绑定地面辐照度图")?;
    let material = scene
        .scene
        .objects
        .get(object_id)
        .map(|object| object.material_id())
        .map(|id| scene.scene.material(id))
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
    Ok((object_id, (bsdf.x * PI).clamp(0.0, 1.0)))
}

/// 校验并绑定光源 XYZ 焦散图，供一次 RGB 相机渲染只读复用。
///
/// 输入：scene 与 SpectralLighting；输出：含图像引用、接收范围、地面编号、反射率和太阳标尺的 GroundLighting。
/// 地面必须是中性灰 Lambertian，图像/太阳/编号不匹配或数值无效返回 Err。
pub(super) fn bind_spectral_ground_lighting<'a>(
    scene: &RenderJob,
    lighting: &'a SpectralLighting,
) -> Result<GroundLighting<'a>, Box<dyn Error + Send + Sync>> {
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
    let sun_irradiance_rgb =
        crate::color::xyz_to_linear_srgb(lighting.sun_irradiance_xyz).ok_or("sun XYZ overflow")?;
    let (object_id, rho) = validate_rgb_caustics_ground(scene)?;
    Ok(GroundLighting {
        irradiance,
        receiver,
        object_id,
        rho,
        light_wi: lighting.light_wi,
        sun_irradiance_rgb,
    })
}

//! 原有双界面正向光子追踪、Fresnel 通量及 Cauchy 折射率。

use super::Scene;
use crate::{
    geometry::{hit::Hit, ray::Ray},
    material::dielectric::fresnel_dielectric,
    optics::{iormode::IorMode, refract},
};
use nalgebra::Vector3;

/// Constant 忽略波长并返回 1.52；Dispersion 要求 Some(wavelength_nm)。
/// Cauchy 色散模型：n(λ) = A + B / λ_μm²，输入波长单位为 nm。
/// A = 1.5046、B = 0.0042 μm² 是 ROADMAP 中的演示参数，不代表特定真实玻璃。
/// 非有限、非正波长或计算溢出返回 None。
pub fn refractive_index(wavelength_nm: Option<f64>, mode: IorMode) -> Option<f64> {
    const DEFAULT_GLASS_IOR: f64 = 1.52;
    const A: f64 = 1.5046;
    const B: f64 = 0.0042;

    let wavelength_nm = match mode {
        IorMode::Constant => return Some(DEFAULT_GLASS_IOR),
        IorMode::Dispersion => wavelength_nm?,
    };
    if !wavelength_nm.is_finite() || wavelength_nm <= 0.0 {
        return None;
    }
    let wavelength_um = wavelength_nm / 1000.0;
    let index = A + B / wavelength_um.powi(2);
    index.is_finite().then_some(index)
}

/// 正向光子路径的落地结果；weight 是到达地面的通量比例，不是相机 radiance 权重。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotonLanding {
    pub position: Vector3<f64>,
    /// 直射为 1；双透射为 (1-F_enter)*(1-F_exit)，不含折射率平方因子。
    pub weight: f64,
}

/// 兼容的几何落点接口；能量累积应使用 take_photon_landing 获取透射权重。
pub fn take_photon(
    scene: &Scene,
    ray: Ray,
    prism_id: usize,
    ground_id: usize,
    glass_ior: impl FnOnce() -> Option<f64>,
) -> Option<Vector3<f64>> {
    take_photon_landing(scene, ray, prism_id, ground_id, glass_ior).map(|landing| landing.position)
}

/// 追踪从空气出发的单条光线，返回地面落点及确定双透射路径的通量权重。
/// glass_ior 是无参数闭包，例如 || refractive_index(Some(450.0), IorMode::Dispersion)。
/// 闭包只调用一次，返回的正有限折射率用于两个界面；也可传入 || Some(1.52)。
/// 只支持一个三棱镜的两次透射；闭包返回 None、折射率非法、全反射、其他遮挡或未命中地面时返回 None。
/// 被 Fresnel 反射的通量暂不追踪、不重新分配给透射路径。
pub fn take_photon_landing(
    scene: &Scene,
    ray: Ray,
    prism_id: usize,
    ground_id: usize,
    glass_ior: impl FnOnce() -> Option<f64>,
) -> Option<PhotonLanding> {
    let glass_ior = glass_ior()?;
    if !glass_ior.is_finite() || glass_ior <= 0.0 {
        return None;
    }
    if prism_id == ground_id || prism_id >= scene.objects.len() || ground_id >= scene.objects.len()
    {
        return None;
    }

    let first_hit = scene.intersect(&ray)?;
    take_photon_from_hit(scene, ray, first_hit, prism_id, ground_id, glass_ior)
}

/// 从已知首次交点继续追踪，供批量波长追踪复用同一份求交结果。
/// 调用方须校验物体编号和折射率，并传入同一 ray 在该场景中的最近交点。
pub(crate) fn take_photon_from_hit(
    scene: &Scene,
    ray: Ray,
    first_hit: Hit,
    prism_id: usize,
    ground_id: usize,
    glass_ior: f64,
) -> Option<PhotonLanding> {
    const AIR_IOR: f64 = 1.0;
    const EPSILON: f64 = 1e-5;

    // 没有经过玻璃，直接落地。
    if first_hit.object_id == Some(ground_id) {
        return Some(PhotonLanding {
            position: first_hit.coords,
            weight: 1.0,
        });
    }
    if first_hit.object_id != Some(prism_id) {
        return None;
    }

    // 空气 -> 玻璃：朝外法线朝向入射介质。
    let cos_enter = -ray.direction.dot(&first_hit.normal);
    let f_enter = fresnel_dielectric(cos_enter, AIR_IOR, glass_ior)?;
    let inside_direction = refract(ray.direction, first_hit.normal, AIR_IOR, glass_ior)?;
    let inside_ray = Ray::new(
        first_hit.coords - first_hit.normal * EPSILON,
        inside_direction,
    )?;

    let exit_hit = scene.intersect(&inside_ray)?;
    if exit_hit.object_id != Some(prism_id) {
        return None;
    }

    // 玻璃 -> 空气：法线取反，朝向玻璃内部；全反射由 ? 提前返回 None。
    // 入射方向来自玻璃内部，故 cos_exit = -d_inside·(-n_outward)。
    let cos_exit = inside_ray.direction.dot(&exit_hit.normal);
    let f_exit = fresnel_dielectric(cos_exit, glass_ior, AIR_IOR)?;
    let outside_direction = refract(inside_ray.direction, -exit_hit.normal, glass_ior, AIR_IOR)?;
    let outside_ray = Ray::new(
        exit_hit.coords + exit_hit.normal * EPSILON,
        outside_direction,
    )?;

    let ground_hit = scene.intersect(&outside_ray)?;
    if ground_hit.object_id == Some(ground_id) {
        Some(PhotonLanding {
            position: ground_hit.coords,
            weight: (1.0 - f_enter) * (1.0 - f_exit),
        })
    } else {
        None
    }
}

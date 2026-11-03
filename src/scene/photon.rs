//! 理想介质物体的双界面正向光子追踪与 Fresnel 通量。

use super::Scene;
use crate::{
    geometry::{hit::Hit, ray::Ray},
    material::dielectric::fresnel_dielectric,
    optics::{dispersion::Cauchy, iormode::IorMode, refract},
};
use nalgebra::Vector3;

/// Constant 忽略波长并返回 1.52；Dispersion 要求 Some(wavelength_nm)。
/// Cauchy 色散模型：n(λ) = A + B / λ_μm²，输入波长单位为 nm。
/// A = 1.5046、B = 0.0042 μm² 是 Cauchy::default() 的演示参数，不代表特定真实玻璃。
/// 保留无材质实验入口；实际渲染查询物体绑定的 Material::photon_ior。
/// 非有限、非正波长或计算溢出返回 None。
pub fn refractive_index(wavelength_nm: Option<f64>, mode: IorMode) -> Option<f64> {
    match mode {
        IorMode::Constant => Some(1.52),
        IorMode::Dispersion => Cauchy::default().ior(wavelength_nm?),
    }
}

/// 正向光子路径的落地结果；weight 是到达地面的通量比例，不是相机 radiance 权重。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotonLanding {
    pub position: Vector3<f64>,
    /// 直射为 1；双透射为 (1-F_enter)*(1-F_exit)，不含折射率平方因子。
    pub weight: f64,
}

/// 追踪从空气出发的单条光线，返回地面落点及确定双透射路径的通量权重。
/// glass_ior 是无参数闭包，例如 || refractive_index(Some(450.0), IorMode::Dispersion)。
/// 闭包只调用一次，返回的正有限折射率用于两个界面；也可传入 || Some(1.52)。
/// 支持一个闭合凸介质（Sphere/Prism）的两次透射；闭包返回 None、折射率非法、全反射、其他遮挡或未命中地面时返回 None。
/// 此显式 IOR 入口保留独立光学实验；渲染入口通过物体材质查询 IOR。
/// 被 Fresnel 反射的通量暂不追踪、不重新分配给透射路径。
pub fn take_photon_landing(
    scene: &Scene,
    ray: Ray,
    caustics_object_id: usize,
    ground_id: usize,
    glass_ior: impl FnOnce() -> Option<f64>,
) -> Option<PhotonLanding> {
    let glass_ior = glass_ior()?;
    if !glass_ior.is_finite() || glass_ior <= 0.0 {
        return None;
    }
    if caustics_object_id == ground_id
        || caustics_object_id >= scene.objects.len()
        || ground_id >= scene.objects.len()
    {
        return None;
    }

    let first_hit = scene.intersect(&ray)?;
    take_photon_from_hit(
        scene,
        ray,
        first_hit,
        caustics_object_id,
        ground_id,
        glass_ior,
    )
}

/// 从已知首次交点继续追踪，供批量波长追踪复用同一份求交结果。
/// 调用方须校验物体编号和折射率，并传入同一 ray 在该场景中的最近交点。
pub(crate) fn take_photon_from_hit(
    scene: &Scene,
    ray: Ray,
    first_hit: Hit,
    caustics_object_id: usize,
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
    if first_hit.object_id != Some(caustics_object_id) {
        return None;
    }

    // 显式绑定材质时，只有理想 Dielectric 能使用此双透射算法。
    // 无材质几何仍允许调用方显式提供折射率（独立光学实验 API）。
    if !scene.supports_ideal_caustics(caustics_object_id) {
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
    if exit_hit.object_id != Some(caustics_object_id) {
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

impl Scene {
    /// 手工构建场景时使用 add_with_material 返回的编号显式选取目标。
    /// JSON 从物体自身的 caustics 开关选择，不假定球体或地面位于固定下标。
    pub fn select_caustics_object(&mut self, object_id: usize) -> Result<(), String> {
        let material = self
            .objects
            .get(object_id)
            .and_then(|object| object.material_id())
            .and_then(|id| self.material(id))
            .ok_or("焦散物体必须绑定理想 Dielectric 材质")?;
        if material.ior().is_none() || self.ground_id == Some(object_id) {
            return Err("焦散物体必须是接收地面以外的理想 Dielectric".into());
        }
        if self.caustics_object_id.is_some_and(|id| id != object_id) {
            return Err("当前只支持一个焦散物体，请仅为一个 Sphere/Prism 启用 caustics".into());
        }
        self.caustics_object_id = Some(object_id);
        Ok(())
    }

    /// 追踪选中物体的直接双透射路径；折射率来自绑定材质，未选目标时无该路径。
    pub fn trace_caustics_photon(
        &self,
        ray: Ray,
        wavelength_nm: Option<f64>,
        mode: IorMode,
    ) -> Option<PhotonLanding> {
        let object_id = self.caustics_object_id?;
        take_photon_landing(self, ray, object_id, self.ground_id?, || {
            self.photon_ior(object_id, wavelength_nm, mode)
        })
    }

    pub(crate) fn photon_ior(
        &self,
        object_id: usize,
        wavelength_nm: Option<f64>,
        mode: IorMode,
    ) -> Option<f64> {
        let object = self.objects.get(object_id)?;
        match object.material_id() {
            Some(id) => self.material(id)?.photon_ior(wavelength_nm, mode),
            // 无材质几何仅供保留的显式光学实验入口使用。
            None => refractive_index(wavelength_nm, mode),
        }
        .filter(|index| index.is_finite() && *index > 0.0)
    }

    /// 无材质几何保留显式光学实验；绑定材质后必须声明理想介质能力。
    pub(crate) fn supports_ideal_caustics(&self, caustics_object_id: usize) -> bool {
        self.objects.get(caustics_object_id).is_some_and(|object| {
            object
                .material_id()
                .is_none_or(|id| self.material(id).is_some_and(|m| m.ior().is_some()))
        })
    }
}

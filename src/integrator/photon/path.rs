// 单个理想介质的有界通量路径，不分裂光子、不在界面存储或累积能量。

use super::PhotonLanding;
use crate::config::RenderJob;
use crate::geometry::hit::Hit;
use crate::geometry::ray::Ray;
use crate::medium::{Medium, checked_transmittance};
use crate::scene::{MediumRegion, Scene};
use nalgebra::Vector3;
use rand::RngCore;

#[derive(Clone, Copy)]
pub(super) struct PhotonSegment {
    pub ray: Ray,
    pub region: MediumRegion,
    pub distance: f64,
}

impl PhotonSegment {
    pub fn medium(self, scene: &Scene) -> Result<Option<&dyn Medium>, &'static str> {
        let medium = scene.active_medium(self.region)?;
        if medium.is_some() && (!self.distance.is_finite() || self.distance <= 0.0) {
            return Err("invalid photon segment distance");
        }
        Ok(medium)
    }

    pub fn transmittance(self, scene: &Scene, wavelength_nm: f64) -> Result<f64, &'static str> {
        self.medium(scene)?.map_or(Ok(1.0), |medium| {
            self.transmittance_in(medium, wavelength_nm)
        })
    }

    pub fn transmittance_in(
        self,
        medium: &dyn Medium,
        wavelength_nm: f64,
    ) -> Result<f64, &'static str> {
        checked_transmittance(medium, &self.ray, self.distance, wavelength_nm)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotonMedium {
    /// 当前在空气中，折射率为 1。
    Air,
    /// 当前路径所选唯一玻璃，IOR 在整条单波长路径上固定。
    Glass,
}

#[derive(Debug, Clone, Copy)]
pub struct PhotonPathState {
    /// 当前向前续追的射线，从光源、玻璃界面或体散射点出发。
    pub ray: Ray,
    /// 界面的折射率一侧，用于决定 IOR 与法线方向。
    pub medium: PhotonMedium,
    /// 当前射线所在的参与介质区域。
    pub medium_region: MediumRegion,
    /// 相对于发射光谱通量 Φλ 的无量纲系数，包含介质段和玻璃界面的权重。
    pub flux_weight: f64,
    /// 已发生的反射/透射总次数，用于事件上限截断。
    pub event_count: usize,
    /// 真实体散射次数，独立于玻璃事件。
    pub volume_scatters: usize,
    /// 其中反射的次数，包含全反射。
    #[cfg(any(feature = "diagnostics", test))]
    pub reflections: usize,
    /// 其中穿过界面的次数，每次透射切换 Air/Glass。
    #[cfg(any(feature = "diagnostics", test))]
    pub transmissions: usize,
}

impl PhotonPathState {
    /// 从已分类的起射点和发射段 Tr 初始化光子状态。
    pub(super) fn new(ray: Ray, region: MediumRegion, target: usize, tr: f64) -> Self {
        Self {
            ray,
            medium: if region == MediumRegion::Interior(target) {
                PhotonMedium::Glass
            } else {
                PhotonMedium::Air
            },
            medium_region: region,
            flux_weight: tr,
            event_count: 0,
            volume_scatters: 0,
            #[cfg(any(feature = "diagnostics", test))]
            reflections: 0,
            #[cfg(any(feature = "diagnostics", test))]
            transmissions: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PhotonTermination {
    /// 在空气中首次到达地面正面，坐标可用于沉积通量。
    Ground(Vector3<f64>),
    /// 没有下一交点，光子离开场景。
    Escaped,
    /// 命中选中玻璃和可接收地面之外的物体，通量被遮挡丢弃。
    Blocked,
    /// 路径需要再执行一次玻璃界面事件，但已达到界面预算。
    EventLimit,
}

#[derive(Debug, Clone, Copy)]
pub struct PhotonPathResult {
    /// 终止时的射线、介质、通量权重与事件统计。
    pub state: PhotonPathState,
    /// 停止的原因；只有 Ground 能通过 landing() 提取落地贡献。
    pub termination: PhotonTermination,
}

impl PhotonPathResult {
    /// 从追踪结果提取唯一可沉积的落地贡献。
    ///
    /// 输入：self；输出：Ground 终态对应的 PhotonLanding，其他终态为 None；逃逸、遮挡与截断通量不重分配。
    pub fn landing(&self) -> Option<PhotonLanding> {
        match self.termination {
            PhotonTermination::Ground(position) => Some(PhotonLanding {
                position,
                weight: self.state.flux_weight,
            }),
            _ => None,
        }
    }
}

pub(crate) struct PhotonTracer {
    pub object_id: usize,
    pub ground_id: usize,
    pub glass_ior: f64,
    pub max_events: usize,
}

impl PhotonTracer {
    pub fn new(
        scene: &RenderJob,
        object_id: usize,
        ground_id: usize,
        glass_ior: f64,
        max_events: usize,
    ) -> Result<Self, &'static str> {
        if object_id == ground_id
            || object_id >= scene.scene.objects.len()
            || ground_id >= scene.scene.objects.len()
            || !scene.supports_ideal_caustics(object_id)
            || !glass_ior.is_finite()
            || glass_ior <= 0.0
            || !(1..=1024).contains(&max_events)
        {
            return Err("光子路径要求有效的单一理想介质、地面、IOR 和 1..=1024 事件上限");
        }
        Ok(Self {
            object_id,
            ground_id,
            glass_ior,
            max_events,
        })
    }

    /// 对单条光子的起射点分类，再计算到最近命中的透射率。
    pub fn trace(
        &self,
        scene: &RenderJob,
        ray: Ray,
        first_hit: Option<Hit>,
        wavelength_nm: f64,
        rng: &mut dyn RngCore,
    ) -> Result<PhotonPathResult, &'static str> {
        let state = self.initial_state(scene, ray, first_hit, wavelength_nm)?;
        self.trace_state(scene, state, first_hit, rng, |segment| {
            segment.transmittance(&scene.scene, wavelength_nm)
        })
    }

    pub(super) fn initial_state(
        &self,
        scene: &RenderJob,
        ray: Ray,
        first_hit: Option<Hit>,
        wavelength_nm: f64,
    ) -> Result<PhotonPathState, &'static str> {
        let region = scene.scene.initial_medium_region(ray.origin, None)?;
        let tr = if let Some(first) = first_hit {
            PhotonSegment {
                ray,
                region,
                distance: first.distance,
            }
            .transmittance(&scene.scene, wavelength_nm)?
        } else {
            1.0
        };
        Ok(PhotonPathState::new(ray, region, self.object_id, tr))
    }

    /// Continue a volume-enabled path using the preweighted first segment.
    pub(super) fn trace_state_with_volume(
        &self,
        scene: &RenderJob,
        state: PhotonPathState,
        first_hit: Option<Hit>,
        wavelength_nm: f64,
        max_volume_scatters: usize,
        rng: &mut dyn RngCore,
    ) -> Result<PhotonPathResult, String> {
        self.trace_after_first_segment(scene, state, first_hit, rng, |state, distance, rng| {
            let Some(medium) = scene.scene.active_medium(state.medium_region)? else {
                return Ok(false);
            };
            if distance.is_some_and(|d| !d.is_finite() || d <= 0.0) {
                return Err("invalid photon segment distance".into());
            }
            let allow_scattering = state.volume_scatters < max_volume_scatters;
            let distance = distance.unwrap_or(f64::INFINITY);
            let sample = medium.sample_segment(
                &state.ray,
                distance,
                wavelength_nm,
                allow_scattering,
                rng,
            )?;
            sample.validate(&state.ray, distance, allow_scattering)?;
            state.flux_weight *= sample.weight;
            if !state.flux_weight.is_finite() {
                return Err("photon medium flux weight overflow".into());
            }
            let Some(position) = sample.position else {
                return Ok(false);
            };
            // HG is sampled from its own density, so its direction weight is 1.
            let direction = medium
                .phase(wavelength_nm)
                .sample(-state.ray.direction, rng)
                .wi;
            state.ray = Ray::new(position, direction).ok_or("invalid photon phase ray")?;
            state.volume_scatters += 1;
            Ok(true)
        })
    }

    /// Walk successive finite hit segments. The caller supplies the first
    /// segment's weight and either evaluates or records later transmittances.
    pub(super) fn trace_state(
        &self,
        scene: &RenderJob,
        state: PhotonPathState,
        hit: Option<Hit>,
        rng: &mut dyn RngCore,
        mut transmittance: impl FnMut(PhotonSegment) -> Result<f64, &'static str>,
    ) -> Result<PhotonPathResult, &'static str> {
        self.trace_after_first_segment(scene, state, hit, rng, |state, distance, _| {
            if let Some(distance) = distance {
                state.flux_weight *= transmittance(PhotonSegment {
                    ray: state.ray,
                    region: state.medium_region,
                    distance,
                })?;
            }
            Ok(false)
        })
    }

    /// The first segment is already weighted. Later segment transport returns
    /// true when a volume event has changed the ray and invalidated the next hit.
    fn trace_after_first_segment<E: From<&'static str>>(
        &self,
        scene: &RenderJob,
        mut state: PhotonPathState,
        mut hit: Option<Hit>,
        rng: &mut dyn RngCore,
        mut advance_segment: impl FnMut(
            &mut PhotonPathState,
            Option<f64>,
            &mut dyn RngCore,
        ) -> Result<bool, E>,
    ) -> Result<PhotonPathResult, E> {
        let termination = loop {
            let Some(current) = hit else {
                break PhotonTermination::Escaped;
            };
            if current.object_id == self.ground_id
                && state.medium == PhotonMedium::Air
                && current.is_front_face(state.ray.direction)
            {
                break PhotonTermination::Ground(current.coords);
            }
            if current.object_id != self.object_id {
                break PhotonTermination::Blocked;
            }
            // 允许恰好完成 max_events 次散射后的落地/逃逸，下一次散射才截断。
            if state.event_count == self.max_events {
                break PhotonTermination::EventLimit;
            }
            self.scatter(&scene.scene, &mut state, current, rng)?;
            hit = scene.scene.intersect(&state.ray);
            while advance_segment(&mut state, hit.map(|next| next.distance), rng)? {
                hit = scene.scene.intersect_from_volume_with_objects(
                    &state.ray,
                    &[self.object_id, self.ground_id],
                );
            }
        };
        Ok(PhotonPathResult { state, termination })
    }

    /// 在当前玻璃界面抽取一次反射或透射，并原地更新光子 Ray、介质、通量和计数。
    ///
    /// 输入：可变 state、对应交点 hit、独立事件 RNG；输出：成功为 Ok(())，介质/法线不匹配或计算失败为 Err。
    ///
    /// 公式：反射概率 p_r=F、透射概率 p_t=1-F，通量 W←W·C/p，C 分别为 F 或 1-F。
    /// 按 Fresnel 抽样时 C/p=1；光子传输不乘相机的 (η_i/η_t)²，也不能再乘一次 F/1-F。
    /// 全反射与 F=0 的确定事件不抽 RNG；从真实交点续追，由几何 ε 排除自交，避免法线偏移跨过尖棱。
    pub(super) fn scatter(
        &self,
        scene: &Scene,
        state: &mut PhotonPathState,
        hit: Hit,
        rng: &mut dyn RngCore,
    ) -> Result<(), &'static str> {
        let (normal, eta_i, eta_t, transmitted_medium) = match state.medium {
            PhotonMedium::Air => (hit.normal, 1.0, self.glass_ior, PhotonMedium::Glass),
            PhotonMedium::Glass => (-hit.normal, self.glass_ior, 1.0, PhotonMedium::Air),
        };
        let cos_i = -state.ray.direction.dot(&normal);
        // 背面或切线命中与当前折射率侧别不一致。
        if !cos_i.is_finite() || cos_i <= 0.0 {
            return Err("光子方向与当前介质/界面法线不一致");
        }
        let interface = crate::optics::interface::sample_dielectric(
            state.ray.direction,
            normal,
            eta_i,
            eta_t,
            rng,
        )
        .ok_or("invalid photon interface")?;
        let reflected = interface.event == crate::optics::interface::DielectricEvent::Reflection;
        let region = scene.region_after_event(
            state.medium_region,
            &hit,
            state.ray.direction,
            interface.event,
        )?;
        state.flux_weight *= interface.flux_weight();
        // Starting at the real hit avoids crossing the adjacent face at a prism corner.
        state.ray = Ray::new(hit.coords, interface.direction).ok_or("invalid photon ray")?;
        state.medium_region = region;
        state.event_count += 1;
        if reflected {
            #[cfg(any(feature = "diagnostics", test))]
            {
                state.reflections += 1;
            }
        } else {
            #[cfg(any(feature = "diagnostics", test))]
            {
                state.transmissions += 1;
            }
            state.medium = transmitted_medium;
        }
        Ok(())
    }
}

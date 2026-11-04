use crate::{
    geometry::ray::Ray,
    material::ScatterType,
    optics::emission::{WAVELENGTH_MAX_NM, WAVELENGTH_MIN_NM},
};

/// 独立于 RGB CameraPathState 的单波长标量 radiance 状态。
#[derive(Debug, Clone)]
pub struct SpectralPathState {
    pub ray: Ray,
    pub wavelength_nm: f64,
    pub throughput: f64,
    /// Direct：原有太阳、环境和背景的 Lλ，保持既有无焦散入口语义。
    pub radiance: f64,
    /// 仅首次连续地面顶点读取的焦散 Lλ，与 Direct 使用同一路径吞吐量。
    pub caustics_radiance: f64,
    pub last_scatter: Option<ScatterType>,
    pub path_depth: usize,
    pub non_delta_bounces: usize,
    pub max_non_delta_bounces: usize,
    pub has_non_delta_vertex: bool,
    /// 仅用于观察焦散：所有地面顶点忽略解析太阳，环境与 Caustics 继续累计。
    pub debug_disable_ground_sun: bool,
}

impl SpectralPathState {
    pub fn new(ray: Ray, wavelength_nm: f64, max_non_delta_bounces: usize) -> Result<Self, String> {
        let state = Self {
            ray,
            wavelength_nm,
            throughput: 1.0,
            radiance: 0.0,
            caustics_radiance: 0.0,
            last_scatter: None,
            path_depth: 0,
            non_delta_bounces: 0,
            max_non_delta_bounces,
            has_non_delta_vertex: false,
            debug_disable_ground_sun: false,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn delta_events(&self) -> usize {
        self.path_depth.saturating_sub(self.non_delta_bounces)
    }

    pub fn combined_radiance(&self) -> f64 {
        self.radiance + self.caustics_radiance
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        crate::config::sceneconfig::validate_diffuse_bounces(self.max_non_delta_bounces)?;
        if !(WAVELENGTH_MIN_NM..=WAVELENGTH_MAX_NM).contains(&self.wavelength_nm)
            || self
                .ray
                .origin
                .iter()
                .chain(self.ray.direction.iter())
                .any(|v| !v.is_finite())
            || !self.ray.direction.norm_squared().is_finite()
            || self.ray.direction.norm_squared() <= 0.0
            || !self.throughput.is_finite()
            || self.throughput < 0.0
            || !self.radiance.is_finite()
            || self.radiance < 0.0
            || !self.caustics_radiance.is_finite()
            || self.caustics_radiance < 0.0
            || !self.combined_radiance().is_finite()
            || self.non_delta_bounces > self.path_depth
            || self.non_delta_bounces > self.max_non_delta_bounces
            || self.path_depth.saturating_sub(self.non_delta_bounces)
                > super::super::path::MAX_DELTA_EVENTS
        {
            return Err("光谱路径波长、吞吐量、辐亮度或深度状态无效".into());
        }
        Ok(())
    }

    pub(super) fn scatter(&mut self, event: ScatterType, weight: f64) -> Result<bool, String> {
        if !weight.is_finite() || weight < 0.0 {
            return Err("光谱 BSDF 权重必须有限且非负".into());
        }
        self.throughput *= weight;
        if !self.throughput.is_finite() {
            return Err("光谱吞吐量计算溢出".into());
        }
        if event.is_delta() || self.throughput != 0.0 {
            self.path_depth += 1;
            self.last_scatter = Some(event);
            if !event.is_delta() {
                self.non_delta_bounces += 1;
            }
        }
        Ok(self.throughput != 0.0)
    }
}

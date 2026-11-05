//! Path state contains transport quantities and algorithm budgets only.
use super::transport::{CameraTransport, Rgb, Spectral};
use crate::{film::Framebuffer, geometry::ray::Ray, material::ScatterType};
use nalgebra::Vector3;
pub struct CameraLightingImages {
    pub direct: Framebuffer,
    pub caustics: Framebuffer,
    pub combined: Framebuffer,
    #[cfg(feature = "diagnostics")]
    pub path_debug: Option<crate::diagnostics::PathDebugBuffer>,
}
pub struct CameraPathComponents<V = Vector3<f64>> {
    pub direct: V,
    pub caustics: V,
}
impl<V: Default> Default for CameraPathComponents<V> {
    fn default() -> Self {
        Self {
            direct: V::default(),
            caustics: V::default(),
        }
    }
}
pub struct CameraPathRngs<'a> {
    pub camera: &'a mut dyn rand::RngCore,
    pub environment: &'a mut dyn rand::RngCore,
    pub diffuse: &'a mut dyn rand::RngCore,
}
pub struct ScatterRay<V = Vector3<f64>> {
    pub ray: Ray,
    pub throughput: V,
    pub event: ScatterType,
}
pub struct CameraPathState<T: CameraTransport = Rgb> {
    pub ray: Ray,
    pub transport: T,
    pub last_scatter: Option<ScatterType>,
    pub throughput: T::Value,
    pub radiance: CameraPathComponents<T::Value>,
    pub path_depth: usize,
    pub has_non_delta_vertex: bool,
    pub non_delta_bounces: usize,
    pub max_non_delta_bounces: usize,
    #[cfg(feature = "diagnostics")]
    pub debug_disable_ground_sun: bool,
}
impl CameraPathState<Rgb> {
    pub fn new(ray: Ray, max_non_delta_bounces: usize) -> Self {
        Self::with_transport(ray, max_non_delta_bounces, Rgb::default())
    }
}
impl CameraPathState<Spectral> {
    pub fn spectral(
        ray: Ray,
        wavelength_nm: f64,
        max_non_delta_bounces: usize,
    ) -> Result<Self, String> {
        let state = Self::with_transport(ray, max_non_delta_bounces, Spectral { wavelength_nm });
        state.validate()?;
        Ok(state)
    }
}
impl<T: CameraTransport> Clone for CameraPathState<T> {
    fn clone(&self) -> Self {
        Self {
            ray: self.ray,
            transport: self.transport,
            last_scatter: self.last_scatter,
            throughput: self.throughput,
            radiance: CameraPathComponents {
                direct: self.radiance.direct,
                caustics: self.radiance.caustics,
            },
            path_depth: self.path_depth,
            has_non_delta_vertex: self.has_non_delta_vertex,
            non_delta_bounces: self.non_delta_bounces,
            max_non_delta_bounces: self.max_non_delta_bounces,
            #[cfg(feature = "diagnostics")]
            debug_disable_ground_sun: self.debug_disable_ground_sun,
        }
    }
}
impl<T: CameraTransport> CameraPathState<T> {
    fn with_transport(ray: Ray, max_non_delta_bounces: usize, transport: T) -> Self {
        Self {
            ray,
            transport,
            last_scatter: None,
            throughput: transport.one(),
            radiance: CameraPathComponents::default(),
            path_depth: 0,
            has_non_delta_vertex: false,
            non_delta_bounces: 0,
            max_non_delta_bounces,
            #[cfg(feature = "diagnostics")]
            debug_disable_ground_sun: false,
        }
    }
    pub fn delta_events(&self) -> usize {
        self.path_depth - self.non_delta_bounces
    }
    pub fn combined_radiance(&self) -> T::Value {
        self.radiance.direct + self.radiance.caustics
    }
    pub fn validate(&self) -> Result<(), String> {
        if !self.transport.valid()
            || !self.transport.valid_weight(self.throughput)
            || !self.transport.finite(self.radiance.direct)
            || !self.transport.finite(self.radiance.caustics)
            || !self.transport.finite(self.combined_radiance())
            || self.non_delta_bounces > self.path_depth
            || self.non_delta_bounces > self.max_non_delta_bounces
            || self.max_non_delta_bounces > 1024
            || self.path_depth - self.non_delta_bounces > super::path::MAX_DELTA_EVENTS
            || self
                .ray
                .origin
                .iter()
                .chain(self.ray.direction.iter())
                .any(|v| !v.is_finite())
            || self.ray.direction.norm_squared() == 0.0
        {
            return Err("invalid initial path state".into());
        }
        Ok(())
    }
    pub fn apply_scatter(
        &mut self,
        event: ScatterType,
        weight: T::Value,
    ) -> Result<bool, &'static str> {
        if !self.transport.valid_weight(weight) {
            return Err("invalid BSDF weight");
        }
        self.throughput = self.transport.multiply(self.throughput, weight);
        if !self.transport.finite(self.throughput) {
            return Err("path throughput overflow");
        }
        let continues = self.throughput != T::Value::default();
        if event.is_delta() || continues {
            self.path_depth += 1;
            self.last_scatter = Some(event);
            self.non_delta_bounces += usize::from(!event.is_delta());
        }
        Ok(continues)
    }
}

//! A single bounded, achromatic medium. Coefficients are in m⁻¹.
//! The box is a spatial mask, including inside solids; it is not a material interface.

mod bounds;
mod config;
mod grid;
mod homogeneous;
pub mod phase;
mod types;

pub use config::{GridMediumConfig, HomogeneousMediumConfig, MediumConfig, from_config};
pub use grid::GridMedium;
pub use homogeneous::HomogeneousMedium;
pub use types::{MediumCoefficients, MediumSample};

use crate::geometry::ray::Ray;
use nalgebra::Vector3;
use phase::HenyeyGreenstein;
use rand::RngCore;

/// Achromatic camera medium, shared by RGB and spectral paths and Rayon workers.
/// Implementations return nonnegative finite local coefficients and a global
/// extinction bound covering all positions. Ray distances are in metres.
pub trait Medium: Send + Sync {
    fn density_at(&self, position: Vector3<f64>) -> f64;
    fn coefficients_at(&self, position: Vector3<f64>) -> MediumCoefficients;
    fn sigma_t_majorant(&self) -> f64;
    fn phase(&self) -> HenyeyGreenstein;
    fn transmittance_ray(&self, ray: &Ray, max_distance: f64) -> f64;

    /// Reject budgets requiring an unsupported collision sampler. The default
    /// permits all budgets; implementations with restricted sampling override it.
    fn validate_volume_sampling(&self, _max_volume_scatters: usize) -> Result<(), String> {
        Ok(())
    }

    /// Return a position on the input ray within the clipped segment and a finite,
    /// nonnegative compensated weight, or a segment weight without a collision.
    /// Respect allow_scattering and reject unsupported sampling;
    /// deterministic extinction must not consume RNG or use an incorrect distance PDF.
    fn sample_segment(
        &self,
        ray: &Ray,
        max_distance: f64,
        allow_scattering: bool,
        rng: &mut dyn RngCore,
    ) -> Result<MediumSample, String>;
}

#[cfg(test)]
mod tests;

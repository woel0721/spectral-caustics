//! Bounded participating media. Coefficients are in m⁻¹.
//! The box is a spatial mask, including inside solids; it is not a material interface.

mod bounds;
mod config;
mod grid;
mod homogeneous;
pub mod phase;
mod procedural;
mod spectral;
mod types;

pub use config::{
    GridMediumConfig, HgAnisotropy, HomogeneousMediumConfig, MediumCoefficient, MediumConfig,
    ProceduralDensityConfig, ProceduralMediumConfig, from_config,
};
pub use grid::GridMedium;
pub use homogeneous::HomogeneousMedium;
pub use types::{MediumCoefficients, MediumSample};

use crate::geometry::ray::Ray;
use nalgebra::Vector3;
use phase::HenyeyGreenstein;
use rand::RngCore;

/// Scene medium shared by camera and photon paths and Rayon workers.
/// Wavelength arguments are in nm (finite and within 380–780 nm).
/// Ray distances are in metres.
/// Camera transport uses only sample_segment, transmittance_ray and phase:
/// density traversal, majorants and null-collision loops stay inside the medium.
pub trait Medium: Send + Sync {
    /// True if the coefficients or phase function require a spectral camera.
    fn is_wavelength_dependent(&self) -> bool {
        false
    }
    /// True if the absorption or scattering coefficient depends on wavelength.
    /// Media with only a spectral phase function override this to return false.
    fn has_wavelength_dependent_coefficients(&self) -> bool {
        self.is_wavelength_dependent()
    }
    fn density_at(&self, position: Vector3<f64>) -> f64;
    fn coefficients_at(&self, position: Vector3<f64>, wavelength_nm: f64) -> MediumCoefficients;
    fn sigma_t_majorant(&self, wavelength_nm: f64) -> f64;
    fn phase(&self, wavelength_nm: f64) -> HenyeyGreenstein;
    /// Deterministic transmittance in [0, 1] from the ray origin up to max_distance.
    fn transmittance_ray(&self, ray: &Ray, max_distance: f64, wavelength_nm: f64) -> f64;

    /// Return a real collision on the input ray, strictly between the ray origin
    /// and any finite segment endpoint, or a segment weight without a collision.
    /// The compensated weight must be finite and nonnegative in either case.
    /// Return only real scattering events; null events do not escape this method.
    /// Respect allow_scattering;
    /// deterministic extinction must not consume RNG or use an incorrect distance PDF.
    fn sample_segment(
        &self,
        ray: &Ray,
        max_distance: f64,
        wavelength_nm: f64,
        allow_scattering: bool,
        rng: &mut dyn RngCore,
    ) -> Result<MediumSample, String>;
}

pub(crate) fn checked_transmittance(
    medium: &dyn Medium,
    ray: &Ray,
    max_distance: f64,
    wavelength_nm: f64,
) -> Result<f64, &'static str> {
    let tr = medium.transmittance_ray(ray, max_distance, wavelength_nm);
    if !(0.0..=1.0).contains(&tr) {
        return Err("invalid medium transmittance");
    }
    Ok(tr)
}

/// Used only for achromatic transport, where the query wavelength has no effect.
pub(crate) const ACHROMATIC_QUERY_NM: f64 = 550.0;
pub(crate) const SPECTRAL_CAMERA_REQUIRED: &str =
    "wavelength-dependent media require render.camera_mode = \"spectral\"";

#[cfg(test)]
mod tests;

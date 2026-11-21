//! Spatially uniform coefficients in a world-space AABB, with exponential free flight.
use super::{
    HomogeneousMediumConfig, Medium, MediumCoefficients, MediumSample,
    bounds::{box_interval, contains, valid_bounds},
    phase::{HenyeyGreenstein, SpectralHg},
    spectral::SpectralCoefficients,
};
use crate::geometry::ray::Ray;
use nalgebra::Vector3;
use rand::{Rng, RngCore};

#[derive(Debug, Clone)]
pub struct HomogeneousMedium {
    min: Vector3<f64>,
    max: Vector3<f64>,
    coefficients: SpectralCoefficients,
    phase: SpectralHg,
}

impl TryFrom<&HomogeneousMediumConfig> for HomogeneousMedium {
    type Error = String;
    fn try_from(c: &HomogeneousMediumConfig) -> Result<Self, String> {
        if !valid_bounds(c.min_m, c.max_m) {
            return Err("medium requires finite bounds min < max".into());
        }
        Ok(Self {
            min: c.min_m.into(),
            max: c.max_m.into(),
            coefficients: SpectralCoefficients::new(&c.sigma_a, &c.sigma_s)?,
            phase: SpectralHg::new(&c.g)?,
        })
    }
}

impl HomogeneousMedium {
    pub fn coefficients(&self, wavelength_nm: f64) -> MediumCoefficients {
        self.coefficients.at(wavelength_nm)
    }

    /// Beer–Lambert for a nonnegative length in metres; vacuum also supports infinity.
    pub fn transmittance(&self, length_m: f64, wavelength_nm: f64) -> f64 {
        beer_lambert(self.sigma_t_majorant(wavelength_nm), length_m)
    }

    /// Slab intersection clipped to [0, max_distance], with unit Ray directions.
    pub fn interval(&self, ray: &Ray, max_distance: f64) -> Option<(f64, f64)> {
        box_interval(self.min, self.max, ray, max_distance)
    }
}

fn beer_lambert(sigma_t: f64, length_m: f64) -> f64 {
    if sigma_t == 0.0 {
        1.0
    } else {
        (-sigma_t * length_m).exp()
    }
}

impl Medium for HomogeneousMedium {
    fn is_wavelength_dependent(&self) -> bool {
        self.has_wavelength_dependent_coefficients() || self.phase.is_wavelength_dependent()
    }

    fn has_wavelength_dependent_coefficients(&self) -> bool {
        self.coefficients.is_wavelength_dependent()
    }

    /// Unit density inside the closed box, vacuum outside (including invalid points).
    fn density_at(&self, position: Vector3<f64>) -> f64 {
        f64::from(contains(self.min, self.max, position))
    }
    fn coefficients_at(&self, position: Vector3<f64>, wavelength_nm: f64) -> MediumCoefficients {
        if contains(self.min, self.max, position) {
            self.coefficients(wavelength_nm)
        } else {
            MediumCoefficients::default()
        }
    }
    fn sigma_t_majorant(&self, wavelength_nm: f64) -> f64 {
        self.coefficients(wavelength_nm).sigma_t()
    }
    fn phase(&self, wavelength_nm: f64) -> HenyeyGreenstein {
        self.phase.at(wavelength_nm)
    }
    fn transmittance_ray(&self, ray: &Ray, max_distance: f64, wavelength_nm: f64) -> f64 {
        self.interval(ray, max_distance)
            .map_or(1.0, |(a, b)| self.transmittance(b - a, wavelength_nm))
    }

    /// While scattering is allowed, sample s=-ln(1-u)/σt. Implicit absorption gives
    /// σs Tr(s)/(σt Tr(s))=σs/σt; survival gives Tr(L)/Tr(L)=1.
    /// At the camera's volume budget, exclude further events using deterministic Tr.
    /// Pure absorption is evaluated analytically to avoid binary survival noise.
    fn sample_segment(
        &self,
        ray: &Ray,
        max_distance: f64,
        wavelength_nm: f64,
        allow_scattering: bool,
        rng: &mut dyn RngCore,
    ) -> Result<MediumSample, String> {
        let Some((enter, exit)) = self.interval(ray, max_distance) else {
            return Ok(MediumSample {
                position: None,
                weight: 1.0,
            });
        };
        let coefficients = self.coefficients(wavelength_nm);
        let sigma_t = coefficients.sigma_t();
        if !sigma_t.is_finite() {
            return Err("medium extinction coefficient must be finite".into());
        }
        if !allow_scattering || coefficients.sigma_s == 0.0 {
            return Ok(MediumSample {
                position: None,
                weight: beer_lambert(sigma_t, exit - enter),
            });
        }
        let distance = -(-rng.random::<f64>()).ln_1p() / sigma_t;
        Ok(if distance < exit - enter {
            MediumSample {
                position: Some(ray.at(enter + distance)),
                weight: coefficients.sigma_s / sigma_t,
            }
        } else {
            MediumSample {
                position: None,
                weight: 1.0,
            }
        })
    }
}

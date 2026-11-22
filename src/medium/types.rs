//! Local coefficients and compensated segment sampling results.
use crate::geometry::ray::Ray;
use nalgebra::Vector3;

/// Local absorption/scattering coefficients; extinction is their sum.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MediumCoefficients {
    pub sigma_a: f64,
    pub sigma_s: f64,
}

impl MediumCoefficients {
    pub fn sigma_t(self) -> f64 {
        self.sigma_a + self.sigma_s
    }

    pub(super) fn scaled_by_density(self, density: f64) -> Self {
        Self {
            sigma_a: density * self.sigma_a,
            sigma_s: density * self.sigma_s,
        }
    }
}

/// Distance sampling result. Weight includes Tr / survival PDF, or σs Tr / distance PDF.
#[derive(Debug, Clone, Copy)]
pub struct MediumSample {
    pub position: Option<Vector3<f64>>,
    pub weight: f64,
}

impl MediumSample {
    pub(crate) fn validate(
        self,
        ray: &Ray,
        max_distance: f64,
        allow_scattering: bool,
    ) -> Result<(), &'static str> {
        if !self.weight.is_finite() || self.weight < 0.0 {
            return Err("invalid medium sample weight");
        }
        if let Some(position) = self.position {
            if !allow_scattering || position.iter().any(|value| !value.is_finite()) {
                return Err("invalid medium sample position or scattering budget");
            }
            // A volume ray skips hits at t=0. A rounded endpoint can leave it on
            // an interface while the path still carries the incident region.
            if position == ray.origin
                || (max_distance.is_finite() && position == ray.at(max_distance))
            {
                return Err("medium collision rounds to a ray segment endpoint");
            }
        }
        Ok(())
    }
}

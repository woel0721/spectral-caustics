//! Constant coefficients in a world-space AABB, with exponential free-flight sampling.
use super::{
    HomogeneousMediumConfig, Medium, MediumCoefficients, MediumSample,
    bounds::{box_interval, contains, valid_bounds},
    phase::HenyeyGreenstein,
};
use crate::geometry::ray::Ray;
use nalgebra::Vector3;
use rand::{Rng, RngCore};

#[derive(Debug, Clone, Copy)]
pub struct HomogeneousMedium {
    min: Vector3<f64>,
    max: Vector3<f64>,
    sigma_a: f64,
    sigma_s: f64,
    sigma_t: f64,
    phase: HenyeyGreenstein,
}

impl TryFrom<&HomogeneousMediumConfig> for HomogeneousMedium {
    type Error = String;
    fn try_from(c: &HomogeneousMediumConfig) -> Result<Self, String> {
        let sigma_t = c.sigma_a + c.sigma_s;
        if [c.sigma_a, c.sigma_s, sigma_t]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
            || !valid_bounds(c.min_m, c.max_m)
        {
            return Err(
                "medium requires finite bounds min < max and nonnegative finite coefficients"
                    .into(),
            );
        }
        Ok(Self {
            min: c.min_m.into(),
            max: c.max_m.into(),
            sigma_a: c.sigma_a,
            sigma_s: c.sigma_s,
            sigma_t,
            phase: HenyeyGreenstein::new(c.g)?,
        })
    }
}

impl HomogeneousMedium {
    pub fn sigma_a(&self) -> f64 {
        self.sigma_a
    }
    pub fn sigma_s(&self) -> f64 {
        self.sigma_s
    }
    pub fn sigma_t(&self) -> f64 {
        self.sigma_t
    }

    /// Beer–Lambert for a nonnegative length in metres; vacuum also supports infinity.
    pub fn transmittance(&self, length_m: f64) -> f64 {
        if self.sigma_t == 0.0 {
            1.0
        } else {
            (-self.sigma_t * length_m).exp()
        }
    }

    /// Slab intersection clipped to [0, max_distance], with unit Ray directions.
    pub fn interval(&self, ray: &Ray, max_distance: f64) -> Option<(f64, f64)> {
        box_interval(self.min, self.max, ray, max_distance)
    }
}

impl Medium for HomogeneousMedium {
    /// Unit density inside the closed box, vacuum outside (including invalid points).
    fn density_at(&self, position: Vector3<f64>) -> f64 {
        f64::from(contains(self.min, self.max, position))
    }
    fn coefficients_at(&self, position: Vector3<f64>) -> MediumCoefficients {
        let density = self.density_at(position);
        MediumCoefficients {
            sigma_a: density * self.sigma_a,
            sigma_s: density * self.sigma_s,
        }
    }
    fn sigma_t_majorant(&self) -> f64 {
        self.sigma_t
    }
    fn phase(&self) -> HenyeyGreenstein {
        self.phase
    }
    fn transmittance_ray(&self, ray: &Ray, max_distance: f64) -> f64 {
        self.interval(ray, max_distance)
            .map_or(1.0, |(a, b)| self.transmittance(b - a))
    }

    /// While scattering is allowed, sample s=-ln(1-u)/σt. Implicit absorption gives
    /// σs Tr(s)/(σt Tr(s))=σs/σt; survival gives Tr(L)/Tr(L)=1.
    /// At the camera's volume budget, exclude further events using deterministic Tr.
    /// Pure absorption is evaluated analytically to avoid binary survival noise.
    fn sample_segment(
        &self,
        ray: &Ray,
        max_distance: f64,
        allow_scattering: bool,
        rng: &mut dyn RngCore,
    ) -> Result<MediumSample, String> {
        let Some((enter, exit)) = self.interval(ray, max_distance) else {
            return Ok(MediumSample {
                position: None,
                weight: 1.0,
            });
        };
        if !allow_scattering || self.sigma_s == 0.0 {
            return Ok(MediumSample {
                position: None,
                weight: self.transmittance(exit - enter),
            });
        }
        let distance = -(-rng.random::<f64>()).ln_1p() / self.sigma_t;
        Ok(if distance < exit - enter {
            MediumSample {
                position: Some(ray.at(enter + distance)),
                weight: self.sigma_s / self.sigma_t,
            }
        } else {
            MediumSample {
                position: None,
                weight: 1.0,
            }
        })
    }
}

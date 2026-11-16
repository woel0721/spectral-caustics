//! A single bounded, achromatic homogeneous medium. Coefficients are in m⁻¹.
//! The box is a spatial mask, including inside solids; it is not a material interface.

pub mod phase;
use crate::geometry::ray::Ray;
use nalgebra::Vector3;
use phase::HenyeyGreenstein;
use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomogeneousMediumConfig {
    pub min_m: [f64; 3],
    pub max_m: [f64; 3],
    pub sigma_a: f64,
    pub sigma_s: f64,
    pub g: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct HomogeneousMedium {
    min: Vector3<f64>,
    max: Vector3<f64>,
    sigma_a: f64,
    sigma_s: f64,
    sigma_t: f64,
    phase: HenyeyGreenstein,
}

/// Distance sampling result. Weight includes Tr / survival PDF, or σs Tr / distance PDF.
#[derive(Debug, Clone, Copy)]
pub struct MediumSample {
    pub position: Option<Vector3<f64>>,
    pub weight: f64,
}

impl TryFrom<&HomogeneousMediumConfig> for HomogeneousMedium {
    type Error = String;
    fn try_from(c: &HomogeneousMediumConfig) -> Result<Self, String> {
        let sigma_t = c.sigma_a + c.sigma_s;
        if [c.sigma_a, c.sigma_s, sigma_t]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
            || (0..3).any(|i| {
                !c.min_m[i].is_finite()
                    || !c.max_m[i].is_finite()
                    || c.min_m[i] >= c.max_m[i]
                    || !(c.max_m[i] - c.min_m[i]).is_finite()
            })
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
    pub fn phase(&self) -> HenyeyGreenstein {
        self.phase
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
        let (mut enter, mut exit) = (0.0_f64, max_distance);
        for axis in 0..3 {
            let d = ray.direction[axis];
            let o = ray.origin[axis];
            if d == 0.0 {
                if o < self.min[axis] || o > self.max[axis] {
                    return None;
                }
            } else {
                let a = (self.min[axis] - o) / d;
                let b = (self.max[axis] - o) / d;
                enter = enter.max(a.min(b));
                exit = exit.min(a.max(b));
            }
            if exit <= enter {
                return None;
            }
        }
        Some((enter, exit))
    }

    pub fn transmittance_ray(&self, ray: &Ray, max_distance: f64) -> f64 {
        self.interval(ray, max_distance)
            .map_or(1.0, |(a, b)| self.transmittance(b - a))
    }

    /// While scattering is allowed, sample s=-ln(1-u)/σt. Implicit absorption gives
    /// σs Tr(s)/(σt Tr(s))=σs/σt; survival gives Tr(L)/Tr(L)=1.
    /// At the camera's volume budget, exclude further events using deterministic Tr.
    /// Pure absorption is evaluated analytically to avoid binary survival noise.
    pub fn sample_segment(
        &self,
        ray: &Ray,
        max_distance: f64,
        allow_scattering: bool,
        rng: &mut dyn RngCore,
    ) -> MediumSample {
        let Some((enter, exit)) = self.interval(ray, max_distance) else {
            return MediumSample {
                position: None,
                weight: 1.0,
            };
        };
        if !allow_scattering || self.sigma_s == 0.0 {
            return MediumSample {
                position: None,
                weight: self.transmittance(exit - enter),
            };
        }
        let distance = -(-rng.random::<f64>()).ln_1p() / self.sigma_t;
        if distance < exit - enter {
            MediumSample {
                position: Some(ray.at(enter + distance)),
                weight: self.sigma_s / self.sigma_t,
            }
        } else {
            MediumSample {
                position: None,
                weight: 1.0,
            }
        }
    }
}

#[cfg(test)]
mod tests;

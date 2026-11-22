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
        let Some(position) = self.position else {
            return Ok(());
        };
        if !allow_scattering || position.iter().any(|value| !value.is_finite()) {
            return Err("invalid medium sample position or scattering budget");
        }
        if max_distance.is_nan() || max_distance <= 0.0 {
            return Err("invalid medium sample ray segment");
        }
        let endpoint = max_distance.is_finite().then(|| ray.at(max_distance));
        // A volume ray skips hits at t=0. A rounded endpoint can leave it on
        // an interface while the path still carries the incident region.
        if position == ray.origin || endpoint.is_some_and(|end| position == end) {
            return Err("medium collision rounds to a ray segment endpoint");
        }
        let direction_norm_sq = ray.direction.norm_squared();
        if !direction_norm_sq.is_finite() || direction_norm_sq == 0.0 {
            return Err("invalid medium sample ray segment");
        }
        let delta = position - ray.origin;
        if delta.iter().any(|value| !value.is_finite()) {
            return Err("medium collision lies outside the ray segment");
        }
        let t = delta.dot(&ray.direction) / direction_norm_sq;
        if !t.is_finite() || t <= 0.0 {
            return Err("medium collision lies outside the ray segment");
        }
        // Compare against the rounded finite endpoint when representable so a valid
        // sample near max_distance at large origin coordinates is not falsely rejected.
        let before_endpoint = match endpoint {
            Some(end) if end.iter().all(|value| value.is_finite()) => {
                let remaining = (end - position).dot(&ray.direction);
                remaining.is_finite() && remaining > 0.0
            }
            _ => t < max_distance,
        };
        if !before_endpoint {
            return Err("medium collision lies outside the ray segment");
        }
        // Measure perpendicular residual against the 3D coordinate scale: rounding
        // of p = fl(o + t*d) on a large axis projects into t and smaller axes.
        let residual = delta - ray.direction * t;
        let max_residual = residual.iter().fold(0.0_f64, |acc, v| acc.max(v.abs()));
        let scale = ray
            .origin
            .iter()
            .chain(position.iter())
            .fold(t, |acc, v| acc.max(v.abs()));
        let tolerance = (64.0 * f64::EPSILON * scale).max(8.0 * f64::from_bits(1));
        if !max_residual.is_finite() || max_residual > tolerance {
            return Err("medium collision lies off the ray");
        }
        Ok(())
    }
}

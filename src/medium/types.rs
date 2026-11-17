//! Local coefficients and compensated camera segment sampling results.
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
}

/// Distance sampling result. Weight includes Tr / survival PDF, or σs Tr / distance PDF.
#[derive(Debug, Clone, Copy)]
pub struct MediumSample {
    pub position: Option<Vector3<f64>>,
    pub weight: f64,
}

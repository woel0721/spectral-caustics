//! HG uses PBRT's convention: wo and wi both point away from the interaction.
use crate::material::to_world;
use nalgebra::Vector3;
use rand::{Rng, RngCore};
use std::f64::consts::{PI, TAU};

#[derive(Debug, Clone, Copy)]
pub struct HenyeyGreenstein {
    g: f64,
}
#[derive(Debug, Clone, Copy)]
pub struct PhaseSample {
    pub wi: Vector3<f64>,
    pub p: f64,
    pub pdf: f64,
}
impl HenyeyGreenstein {
    pub fn new(g: f64) -> Result<Self, String> {
        if !g.is_finite() || g.abs() >= 1.0 {
            return Err("HG g must be finite and strictly between -1 and 1".into());
        }
        Ok(Self { g })
    }
    pub fn g(self) -> f64 {
        self.g
    }
    /// Unit vectors required. Positive g peaks at wi=-wo (forward scattering).
    pub fn eval(self, wo: Vector3<f64>, wi: Vector3<f64>) -> f64 {
        let cosine = wo.dot(&wi).clamp(-1.0, 1.0);
        let g = self.g;
        // Equivalent to 1+g²+2g cosθ, without cancellation near the peak.
        let denom = if g >= 0.0 {
            (1.0 - g).powi(2) + 2.0 * g * (1.0 + cosine)
        } else {
            (1.0 + g).powi(2) - 2.0 * g * (1.0 - cosine)
        };
        (1.0 - g * g) / (4.0 * PI * denom * denom.sqrt())
    }
    pub fn pdf(self, wo: Vector3<f64>, wi: Vector3<f64>) -> f64 {
        self.eval(wo, wi)
    }
    pub fn sample(self, wo: Vector3<f64>, rng: &mut dyn RngCore) -> PhaseSample {
        let g = self.g.abs();
        let x = 2.0 * rng.random::<f64>() - 1.0;
        // Exact inverse CDF including g=0. The ratio stays in [0,1], avoiding
        // cancellation at u=0 when |g| approaches 1. Negative g mirrors the lobe.
        let ratio = (1.0 - g) / (1.0 + g * x);
        let cosine =
            (1.0 - 0.5 * ratio * ratio * (1.0 - x) * (2.0 + g * (1.0 + x))).clamp(-1.0, 1.0);
        let cosine = if self.g < 0.0 { -cosine } else { cosine };
        let sine = (1.0 - cosine * cosine).max(0.0).sqrt();
        let phi = TAU * rng.random::<f64>();
        let wi = to_world(
            Vector3::new(sine * phi.cos(), sine * phi.sin(), cosine),
            -wo,
        )
        .expect("phase requires a unit wo");
        let pdf = self.pdf(wo, wi);
        PhaseSample { wi, p: pdf, pdf }
    }
}

//! HG uses PBRT's convention: wo and wi both point away from the interaction.
use super::{
    HgAnisotropy,
    spectral::{constant_curve_value, interpolate_curve, validate_wavelength_nodes},
};
use crate::material::to_world;
use nalgebra::Vector3;
use rand::{Rng, RngCore};
use std::f64::consts::{PI, TAU};

const INVALID_G: &str = "HG g must be finite and strictly between -1 and 1";

fn valid_g(g: f64) -> bool {
    g.is_finite() && g.abs() < 1.0
}

/// Validated anisotropy shared by the built-in media.
#[derive(Debug, Clone)]
pub(super) struct SpectralHg(HgAnisotropy);

impl SpectralHg {
    pub(super) fn new(g: &HgAnisotropy) -> Result<Self, String> {
        match g {
            HgAnisotropy::Constant(value) => {
                HenyeyGreenstein::new(*value)?;
            }
            HgAnisotropy::Curve { samples } => {
                validate_wavelength_nodes(samples).map_err(|e| format!("HG g {e}"))?;
                if samples.iter().any(|&[_, value]| !valid_g(value)) {
                    return Err(INVALID_G.into());
                }
                if let Some(value) = constant_curve_value(samples) {
                    return Ok(Self(HgAnisotropy::Constant(value)));
                }
            }
        }
        Ok(Self(g.clone()))
    }

    pub(super) fn is_wavelength_dependent(&self) -> bool {
        matches!(self.0, HgAnisotropy::Curve { .. })
    }

    pub(super) fn at(&self, wavelength_nm: f64) -> HenyeyGreenstein {
        let g = match &self.0 {
            HgAnisotropy::Constant(g) => *g,
            HgAnisotropy::Curve { samples } => interpolate_curve(samples, wavelength_nm),
        };
        HenyeyGreenstein { g }
    }
}

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
        if !valid_g(g) {
            return Err(INVALID_G.into());
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

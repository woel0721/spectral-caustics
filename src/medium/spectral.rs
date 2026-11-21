use super::{MediumCoefficient, MediumCoefficients};
use crate::spectrum::emission::{WAVELENGTH_MAX_NM, WAVELENGTH_MIN_NM};

/// Validated absorption and scattering spectra shared by the built-in media.
#[derive(Debug, Clone)]
pub(super) struct SpectralCoefficients {
    absorption: MediumCoefficient,
    scattering: MediumCoefficient,
}

impl SpectralCoefficients {
    pub(super) fn new(a: &MediumCoefficient, s: &MediumCoefficient) -> Result<Self, String> {
        let coefficients = Self {
            absorption: a.validated().map_err(|e| format!("sigma_a {e}"))?,
            scattering: s.validated().map_err(|e| format!("sigma_s {e}"))?,
        };
        if !coefficients.has_finite_extinction(1.0) {
            return Err("medium extinction coefficient exceeds the finite range".into());
        }
        Ok(coefficients)
    }

    pub(super) fn is_wavelength_dependent(&self) -> bool {
        !self.absorption.nodes().is_empty() || !self.scattering.nodes().is_empty()
    }

    pub(super) fn at(&self, wavelength_nm: f64) -> MediumCoefficients {
        MediumCoefficients {
            sigma_a: self.absorption.at(wavelength_nm),
            sigma_s: self.scattering.at(wavelength_nm),
        }
    }

    pub(super) fn has_finite_extinction(&self, density: f64) -> bool {
        // The exact density-scaled sum peaks at a knot from either curve.
        // Leave rounding headroom for both interpolations, density scaling and
        // their sum between knots. Constant coefficients use identical
        // arithmetic at every wavelength and can use the full finite range.
        let limit = if self.is_wavelength_dependent() {
            f64::MAX * (1.0 - 16.0 * f64::EPSILON)
        } else {
            f64::MAX
        };
        let nodes = self
            .absorption
            .nodes()
            .iter()
            .chain(self.scattering.nodes());
        std::iter::once(WAVELENGTH_MIN_NM)
            .chain(nodes.map(|sample| sample[0]))
            .all(|nm| self.at(nm).scaled_by_density(density).sigma_t() <= limit)
    }
}

impl MediumCoefficient {
    fn validated(&self) -> Result<Self, &'static str> {
        let valid_coefficient = |v: f64| v.is_finite() && v >= 0.0;
        match self {
            Self::Constant(value) if valid_coefficient(*value) => Ok(Self::Constant(*value)),
            Self::Constant(_) => Err("must be nonnegative and finite"),
            Self::Curve { samples } => {
                validate_wavelength_nodes(samples)?;
                if samples.iter().any(|&[_, value]| !valid_coefficient(value)) {
                    return Err("must be nonnegative and finite");
                }
                if let Some(value) = constant_curve_value(samples) {
                    return Ok(Self::Constant(value));
                }
                Ok(self.clone())
            }
        }
    }

    fn nodes(&self) -> &[[f64; 2]] {
        match self {
            Self::Constant(_) => &[],
            Self::Curve { samples } => samples,
        }
    }

    /// Evaluate a validated curve.
    pub(super) fn at(&self, wavelength_nm: f64) -> f64 {
        match self {
            Self::Constant(value) => *value,
            Self::Curve { samples } => interpolate_curve(samples, wavelength_nm),
        }
    }
}

pub(super) fn validate_wavelength_nodes(samples: &[[f64; 2]]) -> Result<(), &'static str> {
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        return Err("requires samples covering 380–780 nm");
    };
    if samples.iter().any(|&[nm, _]| !nm.is_finite() || nm <= 0.0)
        || samples.windows(2).any(|pair| pair[0][0] >= pair[1][0])
    {
        return Err("requires strictly increasing positive finite wavelengths");
    }
    if first[0] > WAVELENGTH_MIN_NM || last[0] < WAVELENGTH_MAX_NM {
        return Err("requires samples covering 380–780 nm");
    }
    Ok(())
}

pub(super) fn constant_curve_value(samples: &[[f64; 2]]) -> Option<f64> {
    let value = samples.first()?[1];
    samples
        .iter()
        .all(|sample| sample[1] == value)
        .then_some(value)
}

/// Interpolate validated finite values with a finite difference between adjacent knots.
pub(super) fn interpolate_curve(samples: &[[f64; 2]], wavelength_nm: f64) -> f64 {
    let i = samples.partition_point(|sample| sample[0] < wavelength_nm);
    if i == 0 {
        return samples[0][1];
    }
    if i == samples.len() {
        return samples[i - 1][1];
    }
    let [hi_nm, hi] = samples[i];
    if wavelength_nm == hi_nm {
        return hi;
    }
    let [lo_nm, lo] = samples[i - 1];
    let t = (wavelength_nm - lo_nm) / (hi_nm - lo_nm);
    // Keep the rounded result inside the interval, including for g near ±1.
    (hi - lo).mul_add(t, lo).clamp(lo.min(hi), lo.max(hi))
}

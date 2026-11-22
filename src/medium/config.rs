//! Serializable medium descriptions and construction of runtime trait objects.
use super::{
    GridMedium, HomogeneousMedium, Medium,
    bounds::valid_bounds,
    grid::{validate_densities, validate_grid_layout},
    phase::SpectralHg,
    spectral::SpectralCoefficients,
};
use serde::{Deserialize, Serialize};

/// The variants have disjoint field sets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MediumConfig {
    Homogeneous(HomogeneousMediumConfig),
    Grid(GridMediumConfig),
    Procedural(ProceduralMediumConfig),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomogeneousMediumConfig {
    pub min_m: [f64; 3],
    pub max_m: [f64; 3],
    pub sigma_a: MediumCoefficient,
    pub sigma_s: MediumCoefficient,
    pub g: HgAnisotropy,
}

/// A constant coefficient or piecewise linear samples in [nm, m⁻¹].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum MediumCoefficient {
    Constant(f64),
    Curve { samples: Vec<[f64; 2]> },
}

impl From<f64> for MediumCoefficient {
    fn from(value: f64) -> Self {
        Self::Constant(value)
    }
}

/// Constant HG anisotropy or piecewise linear samples in [nm, dimensionless g].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum HgAnisotropy {
    Constant(f64),
    Curve { samples: Vec<[f64; 2]> },
}

impl From<f64> for HgAnisotropy {
    fn from(value: f64) -> Self {
        Self::Constant(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridMediumConfig {
    pub min_m: [f64; 3],
    pub max_m: [f64; 3],
    /// [nx, ny, nz], all positive. Densities are dimensionless.
    pub resolution: [usize; 3],
    /// x varies fastest: index = x + nx * (y + ny * z).
    pub densities: Vec<f64>,
    pub sigma_a_base: MediumCoefficient,
    pub sigma_s_base: MediumCoefficient,
    pub g: HgAnisotropy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProceduralMediumConfig {
    pub min_m: [f64; 3],
    pub max_m: [f64; 3],
    pub resolution: [usize; 3],
    pub procedural: ProceduralDensityConfig,
    pub sigma_a_base: MediumCoefficient,
    pub sigma_s_base: MediumCoefficient,
    pub g: HgAnisotropy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProceduralDensityConfig {
    /// Shape seed, independent of the render sampling seed.
    pub seed: u64,
}

impl MediumCoefficient {
    pub(crate) fn is_wavelength_dependent(&self) -> bool {
        matches!(
            self,
            Self::Curve { samples } if super::spectral::constant_curve_value(samples).is_none()
        )
    }
}

impl HgAnisotropy {
    pub(crate) fn is_wavelength_dependent(&self) -> bool {
        matches!(
            self,
            Self::Curve { samples } if super::spectral::constant_curve_value(samples).is_none()
        )
    }
}

impl MediumConfig {
    /// Validate medium configuration without cloning density arrays or building Majorant caches.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Homogeneous(c) => {
                if !valid_bounds(c.min_m, c.max_m) {
                    return Err("medium requires finite bounds min < max".into());
                }
                SpectralCoefficients::new(&c.sigma_a, &c.sigma_s)?;
                SpectralHg::new(&c.g)?;
                Ok(())
            }
            Self::Grid(c) => {
                let base = SpectralCoefficients::new(&c.sigma_a_base, &c.sigma_s_base)?;
                SpectralHg::new(&c.g)?;
                let count = validate_grid_layout(c.min_m, c.max_m, c.resolution)?;
                validate_densities(&c.densities, count, &base)?;
                Ok(())
            }
            Self::Procedural(c) => {
                SpectralCoefficients::new(&c.sigma_a_base, &c.sigma_s_base)?;
                SpectralHg::new(&c.g)?;
                validate_grid_layout(c.min_m, c.max_m, c.resolution)?;
                Ok(())
            }
        }
    }

    pub(crate) fn is_wavelength_dependent(&self) -> bool {
        match self {
            Self::Homogeneous(c) => {
                c.sigma_a.is_wavelength_dependent()
                    || c.sigma_s.is_wavelength_dependent()
                    || c.g.is_wavelength_dependent()
            }
            Self::Grid(c) => {
                c.sigma_a_base.is_wavelength_dependent()
                    || c.sigma_s_base.is_wavelength_dependent()
                    || c.g.is_wavelength_dependent()
            }
            Self::Procedural(c) => {
                c.sigma_a_base.is_wavelength_dependent()
                    || c.sigma_s_base.is_wavelength_dependent()
                    || c.g.is_wavelength_dependent()
            }
        }
    }
}

/// Build a runtime medium from serializable data, matching material::from_config.
pub fn from_config(config: &MediumConfig) -> Result<Box<dyn Medium>, String> {
    match config {
        MediumConfig::Homogeneous(c) => Ok(Box::new(HomogeneousMedium::try_from(c)?)),
        MediumConfig::Grid(c) => Ok(Box::new(GridMedium::try_from(c)?)),
        MediumConfig::Procedural(c) => Ok(Box::new(GridMedium::try_from(c)?)),
    }
}

//! Serializable medium descriptions and construction of runtime trait objects.
use super::{GridMedium, HomogeneousMedium, Medium};
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
    pub g: f64,
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
    pub g: f64,
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
    pub g: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProceduralDensityConfig {
    /// Shape seed, independent of the render sampling seed.
    pub seed: u64,
}

/// Build a runtime medium from serializable data, matching material::from_config.
pub fn from_config(config: &MediumConfig) -> Result<Box<dyn Medium>, String> {
    match config {
        MediumConfig::Homogeneous(c) => Ok(Box::new(HomogeneousMedium::try_from(c)?)),
        MediumConfig::Grid(c) => Ok(Box::new(GridMedium::try_from(c)?)),
        MediumConfig::Procedural(c) => Ok(Box::new(GridMedium::try_from(c)?)),
    }
}

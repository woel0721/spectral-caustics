//! Serializable medium descriptions and construction of runtime trait objects.
use super::{GridMedium, HomogeneousMedium, Medium};
use serde::{Deserialize, Serialize};

/// The two field sets are disjoint; the existing homogeneous JSON is unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MediumConfig {
    Homogeneous(HomogeneousMediumConfig),
    Grid(GridMediumConfig),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomogeneousMediumConfig {
    pub min_m: [f64; 3],
    pub max_m: [f64; 3],
    pub sigma_a: f64,
    pub sigma_s: f64,
    pub g: f64,
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
    pub sigma_a_base: f64,
    pub sigma_s_base: f64,
    pub g: f64,
}

/// Build a runtime medium from serializable data, matching material::from_config.
pub fn from_config(config: &MediumConfig) -> Result<Box<dyn Medium>, String> {
    match config {
        MediumConfig::Homogeneous(c) => Ok(Box::new(HomogeneousMedium::try_from(c)?)),
        MediumConfig::Grid(c) => Ok(Box::new(GridMedium::try_from(c)?)),
    }
}

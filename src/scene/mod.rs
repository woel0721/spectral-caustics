//! Physical scene data and intersection. Integrators borrow it without owning algorithms.
mod lighting;
mod medium_interface;
mod objects;
use crate::{environment::Environment, light::Light, material::Material, medium::Medium};
pub use medium_interface::{MediumInterface, MediumRegion};
use nalgebra::Vector3;
pub use objects::SceneObject;
pub struct Scene {
    pub background_color: Vector3<f64>,
    pub environment: Option<Environment>,
    pub medium: Option<Box<dyn Medium>>,
    pub objects: Vec<SceneObject>,
    pub(crate) materials: Vec<Box<dyn Material>>,
    pub light: Light,
}
#[cfg(test)]
mod tests;

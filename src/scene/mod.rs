//! Physical scene data and intersection. Integrators borrow it without owning algorithms.
mod lighting;
mod objects;
use crate::{environment::Environment, light::Light, material::Material};
use nalgebra::Vector3;
pub use objects::SceneObject;
pub struct Scene {
    pub background_color: Vector3<f64>,
    pub environment: Option<Environment>,
    pub objects: Vec<SceneObject>,
    pub(crate) materials: Vec<Box<dyn Material>>,
    pub light: Light,
}
#[cfg(test)]
mod tests;

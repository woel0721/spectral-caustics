//! Geometry reports a surface; Scene adds the two mandatory table bindings.
use nalgebra::Vector3;
#[derive(Debug, Clone, Copy)]
pub struct GeometryHit {
    pub coords: Vector3<f64>,
    pub normal: Vector3<f64>,
    pub distance: f64,
}
#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub coords: Vector3<f64>,
    pub normal: Vector3<f64>,
    pub distance: f64,
    pub object_id: usize,
    pub material_id: usize,
}

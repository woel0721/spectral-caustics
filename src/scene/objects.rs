use super::Scene;
use crate::{
    geometry::{hit::Hit, object::Hittable, ray::Ray},
    material::Material,
};
pub struct SceneObject {
    pub geometry: Box<dyn Hittable>,
    material_id: usize,
}
impl SceneObject {
    pub fn material_id(&self) -> usize {
        self.material_id
    }
}
impl Scene {
    /// Both bindings are established before an object becomes intersectable.
    pub fn add(&mut self, geometry: Box<dyn Hittable>, material: Box<dyn Material>) -> usize {
        let object_id = self.objects.len();
        let material_id = self.materials.len();
        self.materials.push(material);
        self.objects.push(SceneObject {
            geometry,
            material_id,
        });
        object_id
    }
    pub fn material(&self, id: usize) -> &dyn Material {
        &*self.materials[id]
    }
    pub fn intersect(&self, ray: &Ray) -> Option<Hit> {
        self.objects
            .iter()
            .enumerate()
            .filter_map(|(id, o)| {
                o.geometry.intersect(ray).map(|h| Hit {
                    coords: h.coords,
                    normal: h.normal,
                    distance: h.distance,
                    object_id: id,
                    material_id: o.material_id,
                })
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}

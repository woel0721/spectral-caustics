use super::{MediumInterface, Scene};
use crate::{
    geometry::{hit::Hit, object::Hittable, ray::Ray},
    material::Material,
};
pub struct SceneObject {
    pub geometry: Box<dyn Hittable>,
    material_id: usize,
    pub(super) medium_interface: Option<MediumInterface>,
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
            medium_interface: None,
        });
        object_id
    }
    pub fn material(&self, id: usize) -> &dyn Material {
        &*self.materials[id]
    }
    pub fn intersect(&self, ray: &Ray) -> Option<Hit> {
        self.intersect_objects(ray, false)
    }

    pub(crate) fn intersect_from_volume(&self, ray: &Ray) -> Option<Hit> {
        self.intersect_objects(ray, true)
    }

    fn intersect_objects(&self, ray: &Ray, from_volume: bool) -> Option<Hit> {
        self.objects
            .iter()
            .enumerate()
            .filter_map(|(id, o)| {
                let hit = if from_volume && o.medium_interface.is_some() {
                    o.geometry.intersect_from_volume(ray)
                } else {
                    o.geometry.intersect(ray)
                };
                hit.map(|h| Hit {
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

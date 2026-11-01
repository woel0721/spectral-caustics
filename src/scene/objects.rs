//! 场景物体/材质绑定、编号查询与最近交点。

use super::Scene;
use crate::{
    geometry::{hit::Hit, object::Hittable, ray::Ray},
    material::Material,
};

/// 几何与场景材质编号的绑定；几何本身不依赖任何材质类型。
pub struct SceneObject {
    pub geometry: Box<dyn Hittable>,
    material_id: Option<usize>,
}

impl SceneObject {
    pub fn material_id(&self) -> Option<usize> {
        self.material_id
    }
}

impl Scene {
    /// 将物体的所有权交给场景，返回用于 Hit.object_id 的集合下标。
    pub fn add(&mut self, object: Box<dyn Hittable>) -> usize {
        let id = self.objects.len();
        self.objects.push(SceneObject {
            geometry: object,
            material_id: None,
        });
        id
    }

    /// 添加带材质的物体；接受任意实现 Material 的具体类型。
    pub fn add_with_material(
        &mut self,
        object: Box<dyn Hittable>,
        material: Box<dyn Material>,
    ) -> usize {
        let object_id = self.add(object);
        let material_id = self.materials.len();
        self.materials.push(material);
        self.objects[object_id].material_id = Some(material_id);
        object_id
    }

    /// 通过 Hit.material_id 查询配置构建的运行时材质；非法编号返回 None。
    pub fn material(&self, material_id: usize) -> Option<&dyn Material> {
        self.materials.get(material_id).map(Box::as_ref)
    }

    /// 返回最近命中，并填写 object_id 与可选的 material_id。
    pub fn intersect(&self, ray: &Ray) -> Option<Hit> {
        self.objects
            .iter()
            .enumerate()
            .filter_map(|(id, object)| {
                object.geometry.intersect(ray).map(|mut hit| {
                    hit.object_id = Some(id);
                    hit.material_id = object.material_id;
                    hit
                })
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}

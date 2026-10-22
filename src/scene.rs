//! 场景参数与物体集合；intersect 查询所有物体中的最近交点。

use crate::hit::Hit;
use crate::object::Hittable;
use crate::ray::Ray;
use nalgebra::Vector3;

pub struct Scene {
    /// 输出图像的宽、高，单位为像素。
    pub width: usize,
    pub height: usize,
    /// 垂直视场角，单位为度；当前只保存参数。
    pub fov: f64,
    /// 线性 RGB 背景色；后续写入 XYZ Framebuffer 前需要转换颜色空间。
    pub background_color: Vector3<f64>,
    /// Box 让大小不同的物体可以放入同一个 Vec；dyn Hittable 提供统一接口。
    pub objects: Vec<Box<dyn Hittable>>,
}

impl Scene {
    /// 创建空场景，默认视场角为 90 度、背景为黑色。
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            fov: 90.0,
            background_color: Vector3::zeros(),
            objects: Vec::new(),
        }
    }

    /// 将物体的所有权交给场景，返回用于 Hit.object_id 的集合下标。
    pub fn add(&mut self, object: Box<dyn Hittable>) -> usize {
        let id = self.objects.len();
        self.objects.push(object);
        id
    }

    /// 返回最近的命中记录，并将场景中的物体下标写入 object_id。
    pub fn intersect(&self, ray: &Ray) -> Option<Hit> {
        self.objects
            .iter()
            .enumerate()
            .filter_map(|(id, object)| {
                object.intersect(ray).map(|mut hit| {
                    hit.object_id = Some(id);
                    hit
                })
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ground::Ground, prism::Prism, ray::Ray};

    #[test]
    fn added_objects_keep_their_own_intersection_behavior() {
        let mut scene = Scene::new(512, 512);
        scene.add(Box::new(Prism::new().unwrap()));
        scene.add(Box::new(Ground::new(0.0).unwrap()));
        let ray = Ray::new(
            Vector3::new(0.25, 0.2, 3.0),
            Vector3::new(0.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();

        // 这里只验证同一集合中的不同物体仍调用各自的求交算法。
        // 场景级的遍历、最近命中选择由 Scene::intersect 负责。
        let prism_hit = scene.objects[0].intersect(&ray).unwrap();
        let ground_hit = scene.objects[1].intersect(&ray).unwrap();
        assert!((prism_hit.coords - Vector3::new(0.25, 0.2, 1.4)).norm() < 1e-12);
        assert!((ground_hit.coords - Vector3::new(0.25, 0.2, 0.0)).norm() < 1e-12);
    }

    #[test]
    fn scene_returns_nearest_hit_and_object_id_independently_of_order() {
        assert!(
            Scene::new(8, 4)
                .intersect(
                    &Ray::new(
                        Vector3::new(0.0, 0.0, 3.0),
                        Vector3::new(0.0, 0.0, -1.0),
                        0.0
                    )
                    .unwrap()
                )
                .is_none()
        );
        for ground_first in [false, true] {
            let mut scene = Scene::new(8, 4);
            let (prism_id, ground_id) = if ground_first {
                let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
                (scene.add(Box::new(Prism::new().unwrap())), ground_id)
            } else {
                let prism_id = scene.add(Box::new(Prism::new().unwrap()));
                (prism_id, scene.add(Box::new(Ground::new(0.0).unwrap())))
            };
            let ray = Ray::new(
                Vector3::new(0.25, 0.2, 3.0),
                Vector3::new(0.0, 0.0, -1.0),
                0.0,
            )
            .unwrap();
            let hit = scene.intersect(&ray).unwrap();
            assert_eq!(hit.object_id, Some(prism_id));
            assert!((hit.distance - 1.6).abs() < 1e-12);
            assert!((hit.coords - Vector3::new(0.25, 0.2, 1.4)).norm() < 1e-12);
            let ray = Ray::new(
                Vector3::new(1.0, 0.2, 3.0),
                Vector3::new(0.0, 0.0, -1.0),
                0.0,
            )
            .unwrap();
            let hit = scene.intersect(&ray).unwrap();
            assert_eq!(hit.object_id, Some(ground_id));
            assert_eq!(hit.distance, 3.0);
        }
    }
}

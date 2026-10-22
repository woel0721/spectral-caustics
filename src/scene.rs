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

/// Snell 折射：direction、normal 应为单位向量，normal 朝向入射介质。
/// eta_i 是入射介质折射率，eta_t 是出射介质折射率；全反射或非法数值返回 None。
pub fn refract(
    direction: Vector3<f64>,
    normal: Vector3<f64>,
    eta_i: f64,
    eta_t: f64,
) -> Option<Vector3<f64>> {
    if !direction.iter().chain(normal.iter()).all(|v| v.is_finite())
        || !eta_i.is_finite()
        || eta_i <= 0.0
        || !eta_t.is_finite()
        || eta_t <= 0.0
    {
        return None;
    }
    let cosi = -normal.dot(&direction);
    let eta = eta_i / eta_t;
    let k = 1.0 - eta * eta * (1.0 - cosi * cosi);
    if !k.is_finite() || k < 0.0 {
        None
    } else {
        let refracted = eta * direction + (eta * cosi - k.sqrt()) * normal;
        refracted.iter().all(|v| v.is_finite()).then_some(refracted)
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

    #[test]
    fn normal_incidence_is_unchanged_and_total_internal_reflection_returns_none() {
        let direction = Vector3::new(0.0, 0.0, -1.0);
        let normal = Vector3::new(0.0, 0.0, 1.0);
        for (eta_i, eta_t) in [(1.0, 1.52), (1.52, 1.0)] {
            assert!((refract(direction, normal, eta_i, eta_t).unwrap() - direction).norm() < 1e-12);
        }
        let angle = 50.0_f64.to_radians();
        let direction = Vector3::new(angle.sin(), 0.0, -angle.cos());
        assert!(refract(direction, normal, 1.52, 1.0).is_none());
    }

    #[test]
    fn refraction_rejects_non_finite_values_and_nonpositive_indices() {
        let direction = Vector3::new(0.0, 0.0, -1.0);
        let normal = Vector3::new(0.0, 0.0, 1.0);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
            assert!(refract(direction, normal, value, 1.0).is_none());
            assert!(refract(direction, normal, 1.0, value).is_none());
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for axis in 0..3 {
                let mut invalid = direction;
                invalid[axis] = value;
                assert!(refract(invalid, normal, 1.0, 1.52).is_none());
                let mut invalid = normal;
                invalid[axis] = value;
                assert!(refract(direction, invalid, 1.0, 1.52).is_none());
            }
        }
    }

    #[test]
    fn single_ray_refracts_through_two_prism_surfaces_and_reaches_ground() {
        let prism = Prism::new().unwrap();
        let ray = Ray::new(
            Vector3::new(0.25, 0.2, 3.0),
            Vector3::new(0.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();
        let entry = prism.intersect(&ray).unwrap();
        assert!((entry.coords - Vector3::new(0.25, 0.2, 1.4)).norm() < 1e-12);
        assert!(ray.direction.dot(&entry.normal) < 0.0);
        let inside_direction = refract(ray.direction, entry.normal, 1.0, 1.52).unwrap();
        // 已知入射角对应 cos(theta_i)=1/sqrt(1.6²+1)，独立检查 Snell 定律。
        let sin_incident = 1.6 / (1.6_f64 * 1.6 + 1.0).sqrt();
        let sin_inside = (1.0 - inside_direction.dot(&entry.normal).powi(2)).sqrt();
        assert!((sin_incident - 1.52 * sin_inside).abs() < 1e-12);
        assert!((inside_direction.norm() - 1.0).abs() < 1e-12);
        let inside_ray =
            Ray::new(entry.coords - entry.normal * 1e-5, inside_direction, 0.0).unwrap();
        let exit = prism.intersect(&inside_ray).unwrap();
        assert!((exit.coords.z - 1.0).abs() < 1e-12);
        assert_eq!(exit.normal, Vector3::new(0.0, 0.0, -1.0));
        assert!(inside_ray.direction.dot(&exit.normal) > 0.0);
        let outside_direction = refract(inside_ray.direction, -exit.normal, 1.52, 1.0).unwrap();
        assert!((outside_direction.norm() - 1.0).abs() < 1e-12);
        assert!((outside_direction.x - 1.52 * inside_ray.direction.x).abs() < 1e-12);
        let outside_ray =
            Ray::new(exit.coords + exit.normal * 1e-5, outside_direction, 0.0).unwrap();
        assert!(prism.intersect(&outside_ray).is_none());
        let ground_hit = Ground::new(0.0).unwrap().intersect(&outside_ray).unwrap();
        assert!((ground_hit.coords.z).abs() < 1e-12);
        assert!((ground_hit.coords.y - 0.2).abs() < 1e-12);
        // 不加玻璃时落在 x=0.25；经过两次折射后向 -x 偏移。
        assert!((-0.8..-0.7).contains(&ground_hit.coords.x));
    }
}

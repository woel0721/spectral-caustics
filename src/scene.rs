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
    if cosi < 0.0 {
        return None;
    }
    let eta = eta_i / eta_t;
    let k = 1.0 - eta * eta * (1.0 - cosi * cosi);
    if !k.is_finite() || k < 0.0 {
        None
    } else {
        let refracted = eta * direction + (eta * cosi - k.sqrt()) * normal;
        refracted.iter().all(|v| v.is_finite()).then_some(refracted)
    }
}

/// 追踪从空气出发的单条光线，返回最终地面落点。
/// 只支持一个三棱镜的两次透射；全反射、其他遮挡或未命中地面时返回 None。
pub fn take_photon(
    scene: &Scene,
    ray: Ray,
    prism_id: usize,
    ground_id: usize,
) -> Option<Vector3<f64>> {
    const AIR_IOR: f64 = 1.0;
    const GLASS_IOR: f64 = 1.52;
    const EPSILON: f64 = 1e-5;

    if prism_id == ground_id || prism_id >= scene.objects.len() || ground_id >= scene.objects.len()
    {
        return None;
    }

    let first_hit = scene.intersect(&ray)?;
    // 没有经过玻璃，直接落地。
    if first_hit.object_id == Some(ground_id) {
        return Some(first_hit.coords);
    }
    if first_hit.object_id != Some(prism_id) {
        return None;
    }

    // 空气 -> 玻璃：朝外法线朝向入射介质。
    let inside_direction = refract(ray.direction, first_hit.normal, AIR_IOR, GLASS_IOR)?;
    let inside_ray = Ray::new(
        first_hit.coords - first_hit.normal * EPSILON,
        inside_direction,
        ray.time,
    )?;

    let exit_hit = scene.intersect(&inside_ray)?;
    if exit_hit.object_id != Some(prism_id) {
        return None;
    }

    // 玻璃 -> 空气：法线取反，朝向玻璃内部；全反射由 ? 提前返回 None。
    let outside_direction = refract(inside_ray.direction, -exit_hit.normal, GLASS_IOR, AIR_IOR)?;
    let outside_ray = Ray::new(
        exit_hit.coords + exit_hit.normal * EPSILON,
        outside_direction,
        ray.time,
    )?;

    let ground_hit = scene.intersect(&outside_ray)?;
    if ground_hit.object_id == Some(ground_id) {
        Some(ground_hit.coords)
    } else {
        None
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

    fn photon_scene() -> (Scene, usize, usize) {
        let mut scene = Scene::new(16, 8);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        (scene, prism_id, ground_id)
    }

    #[test]
    fn photon_returns_direct_ground_hit_or_none_when_ray_misses() {
        let (scene, prism_id, ground_id) = photon_scene();
        let ray = Ray::new(
            Vector3::new(1.0, 0.2, 3.0),
            Vector3::new(0.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();
        assert_eq!(
            take_photon(&scene, ray, prism_id, ground_id),
            Some(Vector3::new(1.0, 0.2, 0.0))
        );
        let ray = Ray::new(
            Vector3::new(1.0, 0.2, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            0.0,
        )
        .unwrap();
        assert!(take_photon(&scene, ray, prism_id, ground_id).is_none());
    }

    #[test]
    fn photon_reaches_expected_refracted_landing_point_in_either_object_order() {
        for ground_first in [false, true] {
            let mut scene = Scene::new(16, 8);
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
                2.0,
            )
            .unwrap();
            let position = take_photon(&scene, ray, prism_id, ground_id).unwrap();
            // 与独立单光线示例的已知落点比较；直接落地时 x 应为 0.25。
            assert!((position - Vector3::new(-0.719593104236608, 0.2, 0.0)).norm() < 1e-12);
        }
    }

    #[test]
    fn photon_stops_at_other_objects_before_inside_or_after_glass() {
        // 三个高度分别遮挡入射段、玻璃内部段和出射后的空气段。
        for height in [2.5, 1.2, 0.5] {
            let (mut scene, prism_id, ground_id) = photon_scene();
            scene.add(Box::new(Ground::new(height).unwrap()));
            let ray = Ray::new(
                Vector3::new(0.25, 0.2, 3.0),
                Vector3::new(0.0, 0.0, -1.0),
                0.0,
            )
            .unwrap();
            assert!(take_photon(&scene, ray, prism_id, ground_id).is_none());
        }
    }

    #[test]
    fn photon_terminates_on_total_internal_reflection_at_prism_bottom() {
        let (scene, prism_id, ground_id) = photon_scene();
        // 垂直于右斜面入射，进入后方向不变；与底面法线夹角约 58°，超过临界角。
        let ray = Ray::new(
            Vector3::new(1.05, 0.2, 1.9),
            Vector3::new(-1.6, 0.0, -1.0),
            0.0,
        )
        .unwrap();
        assert_eq!(scene.intersect(&ray).unwrap().object_id, Some(prism_id));
        assert!(take_photon(&scene, ray, prism_id, ground_id).is_none());
    }

    #[test]
    fn photon_rejects_invalid_or_identical_object_ids() {
        let (scene, prism_id, ground_id) = photon_scene();
        let ray = Ray::new(
            Vector3::new(1.0, 0.2, 3.0),
            Vector3::new(0.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();
        assert!(take_photon(&scene, ray, usize::MAX, ground_id).is_none());
        assert!(take_photon(&scene, ray, prism_id, usize::MAX).is_none());
        assert!(take_photon(&scene, ray, ground_id, ground_id).is_none());
    }

    #[test]
    fn refraction_handles_near_normal_near_critical_and_reversed_normals() {
        let normal = Vector3::new(0.0, 0.0, 1.0);
        let angle = 1e-9_f64;
        let direction = Vector3::new(angle.sin(), 0.0, -angle.cos());
        let refracted = refract(direction, normal, 1.0, 1.52).unwrap();
        assert!((refracted.x - direction.x / 1.52).abs() < 1e-15);
        assert!((refracted.norm() - 1.0).abs() < 1e-12);
        assert!(refract(direction, -normal, 1.0, 1.52).is_none());

        let critical = (1.0_f64 / 1.52).asin();
        for (offset, transmitted) in [(-1e-8, true), (1e-8, false)] {
            let angle = critical + offset;
            let direction = Vector3::new(angle.sin(), 0.0, -angle.cos());
            let result = refract(direction, normal, 1.52, 1.0);
            assert_eq!(result.is_some(), transmitted);
            if let Some(direction) = result {
                assert!(direction.iter().all(|value| value.is_finite()));
                assert!((direction.norm() - 1.0).abs() < 1e-12);
            }
        }
    }
}

//! 水平地面 z = height；可选择有限矩形边界，几何求交通过 Hittable 返回记录。

use super::hit::GeometryHit as Hit;
use super::object::Hittable;
use super::ray::INTERSECTION_EPSILON;
use super::ray::PARALLEL_EPSILON;
use super::ray::Ray;
use nalgebra::Vector2;
use nalgebra::Vector3;

#[derive(Debug, Clone, Copy)]
pub struct Ground {
    /// 地面所在水平面的世界 Z 坐标，单位为米。
    height: f64,
    bounds: Option<(Vector2<f64>, Vector2<f64>)>,
}

impl Ground {
    /// 创建水平无限地面，供独立几何实验使用。
    ///
    /// 输入：有限高度 height（米）；输出：平面 z=height，非有限高度返回 None。
    /// 配置场景使用 with_bounds 创建有限地面。
    pub fn new(height: f64) -> Option<Self> {
        if !height.is_finite() {
            return None;
        }
        Some(Self {
            height,
            bounds: None,
        })
    }

    /// 创建具有矩形边界的水平地面，供相机求交和光子接收使用。
    ///
    /// 输入：高度 height 及 XY 边界 min/max（米），要求每轴 min<max。
    ///
    /// 输出：Ground，范围 [min.x,max.x]×[min.y,max.y] 包含边缘；非法高度、跨度或面积返回 None。
    pub fn with_bounds(height: f64, min: Vector2<f64>, max: Vector2<f64>) -> Option<Self> {
        let span = max - min;
        let area = span.x * span.y;
        if min.iter().chain(max.iter()).any(|value| !value.is_finite())
            || span.iter().any(|value| !value.is_finite() || *value <= 0.0)
            || !area.is_finite()
            || area <= 0.0
        {
            return None;
        }
        let mut ground = Self::new(height)?;
        ground.bounds = Some((min, max));
        Some(ground)
    }
}

impl Hittable for Ground {
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        self.intersect_after(ray, INTERSECTION_EPSILON)
    }

    fn intersect_from_volume(&self, ray: &Ray) -> Option<Hit> {
        self.intersect_after(ray, 0.0)
    }
}

impl Ground {
    /// 求光线与水平地面的交点，并检查它是否位于有限边界内。
    ///
    /// 输入：Ray；输出：正向 Hit，平行、越界、距离过近或数值无效返回 None。
    ///
    /// 公式：t=(height-origin.z)/direction.z，P=origin+t·direction。
    /// 支持双面求交，返回的几何法线固定为世界 +Z。
    fn intersect_after(&self, ray: &Ray, min_distance: f64) -> Option<Hit> {
        if !ray.origin.iter().all(|v| v.is_finite()) || !ray.direction.iter().all(|v| v.is_finite())
        {
            return None;
        }

        // 水平地面的单位法线是 (0, 0, 1)，因此投影就是 direction.z。
        if ray.direction.z.abs() <= PARALLEL_EPSILON {
            return None;
        }

        // origin.z + distance * direction.z = height。
        let distance = (self.height - ray.origin.z) / ray.direction.z;
        if !distance.is_finite() || distance <= min_distance {
            return None;
        }

        let coords = ray.at(distance);
        if !coords.iter().all(|v| v.is_finite()) {
            return None;
        }
        if let Some((min, max)) = self.bounds
            && (coords.x < min.x || coords.x > max.x || coords.y < min.y || coords.y > max.y)
        {
            return None;
        }

        // Hit 只保存数据；计算、正向命中检查和法线约定由 Ground 负责。
        Some(Hit {
            coords,
            normal: Vector3::new(0.0, 0.0, 1.0),
            distance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn downward_ray(origin: Vector3<f64>) -> Ray {
        Ray::new(origin, Vector3::new(0.0, 0.0, -5.0)).unwrap()
    }

    #[test]
    fn finite_ground_includes_edges_and_rejects_points_outside_all_four_sides() {
        let ground =
            Ground::with_bounds(0.0, Vector2::new(-2.5, -1.5), Vector2::new(3.0, 2.5)).unwrap();
        for (x, y, inside) in [
            (0.0, 0.0, true),
            (-2.5, -1.5, true),
            (3.0, 2.5, true),
            (-2.5, 2.5, true),
            (3.0, -1.5, true),
            (-2.500001, 0.0, false),
            (3.000001, 0.0, false),
            (0.0, -1.500001, false),
            (0.0, 2.500001, false),
        ] {
            for height in [-2.0_f64, 2.0] {
                let ray = Ray::new(Vector3::new(x, y, height), Vector3::z() * -height).unwrap();
                let hit = ground.intersect(&ray);
                assert_eq!(hit.is_some(), inside, "({x},{y}), height={height}");
                if let Some(hit) = hit {
                    assert_eq!(hit.normal, Vector3::z());
                    assert_eq!(hit.distance, 2.0);
                }
            }
        }
    }

    #[test]
    fn finite_ground_clips_the_hit_position_instead_of_the_ray_origin() {
        let ground = Ground::with_bounds(0.0, Vector2::repeat(-1.0), Vector2::repeat(1.0)).unwrap();
        let from_outside =
            Ray::new(Vector3::new(2.0, 0.0, 2.0), Vector3::new(-1.0, 0.0, -1.0)).unwrap();
        assert!(ground.intersect(&from_outside).unwrap().coords.norm() < 1e-12);
        let towards_outside =
            Ray::new(Vector3::new(0.0, 0.0, 2.0), Vector3::new(1.0, 0.0, -1.0)).unwrap();
        assert!(ground.intersect(&towards_outside).is_none());
    }

    #[test]
    fn finite_ground_rejects_invalid_bounds_and_overflow() {
        for (min, max) in [
            (Vector2::zeros(), Vector2::zeros()),
            (Vector2::repeat(1.0), Vector2::repeat(-1.0)),
            (Vector2::repeat(f64::NAN), Vector2::repeat(1.0)),
            (Vector2::zeros(), Vector2::repeat(f64::INFINITY)),
            (Vector2::repeat(-f64::MAX), Vector2::repeat(f64::MAX)),
            (Vector2::zeros(), Vector2::repeat(f64::MAX)),
        ] {
            assert!(Ground::with_bounds(0.0, min, max).is_none());
        }
        assert!(Ground::with_bounds(f64::NAN, Vector2::zeros(), Vector2::repeat(1.0)).is_none());
    }

    #[test]
    fn vertical_ray_returns_expected_hit_and_upward_normal() {
        let ground = Ground::new(0.0).unwrap();
        let ray = downward_ray(Vector3::new(0.0, 0.0, 2.0));
        let hit = ground.intersect(&ray).unwrap();

        assert_eq!(hit.distance, 2.0);
        assert_eq!(hit.coords, Vector3::zeros());
        assert_eq!(hit.normal, Vector3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn oblique_ray_hits_configured_height_at_expected_position() {
        let ground = Ground::new(1.0).unwrap();
        let ray = Ray::new(Vector3::new(1.0, 2.0, 3.0), Vector3::new(1.0, 0.0, -1.0)).unwrap();
        let hit = ground.intersect(&ray).unwrap();

        assert!((hit.distance - 2.0 * 2.0_f64.sqrt()).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(3.0, 2.0, 1.0)).norm() < 1e-12);
    }

    #[test]
    fn upward_ray_from_below_preserves_geometric_normal() {
        let ground = Ground::new(-1.0).unwrap();
        let ray = Ray::new(Vector3::new(0.0, 0.0, -2.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let hit = ground.intersect(&ray).unwrap();

        assert_eq!(hit.distance, 1.0);
        assert_eq!(hit.coords, Vector3::new(0.0, 0.0, -1.0));
        assert_eq!(hit.normal, Vector3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn parallel_nearly_parallel_and_coplanar_rays_miss() {
        let ground = Ground::new(0.0).unwrap();
        for (height, direction_z) in [
            (1.0, 0.0),
            (1.0, -PARALLEL_EPSILON / 2.0),
            (1.0, PARALLEL_EPSILON / 2.0),
            (0.0, 0.0),
        ] {
            let ray = Ray::new(
                Vector3::new(0.0, 0.0, height),
                Vector3::new(1.0, 0.0, direction_z),
            )
            .unwrap();
            assert!(ground.intersect(&ray).is_none());
        }
    }

    #[test]
    fn intersections_behind_origin_are_rejected() {
        let ground = Ground::new(0.0).unwrap();
        for (height, direction_z) in [(1.0, 1.0), (-1.0, -1.0)] {
            let ray = Ray::new(
                Vector3::new(0.0, 0.0, height),
                Vector3::new(0.0, 0.0, direction_z),
            )
            .unwrap();
            assert!(ground.intersect(&ray).is_none());
        }
    }

    #[test]
    fn self_intersections_are_rejected() {
        let ground = Ground::new(0.0).unwrap();
        for height in [0.0, INTERSECTION_EPSILON / 2.0, INTERSECTION_EPSILON] {
            assert!(
                ground
                    .intersect(&downward_ray(Vector3::new(0.0, 0.0, height)))
                    .is_none()
            );
        }
        assert!(
            ground
                .intersect(&downward_ray(Vector3::new(
                    0.0,
                    0.0,
                    2.0 * INTERSECTION_EPSILON,
                )))
                .is_some()
        );
    }

    #[test]
    fn non_finite_heights_are_rejected() {
        for height in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Ground::new(height).is_none());
        }
    }

    #[test]
    fn non_finite_ray_fields_and_zero_direction_are_rejected() {
        let ground = Ground::new(0.0).unwrap();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for axis in 0..3 {
                let mut ray = downward_ray(Vector3::new(0.0, 0.0, 2.0));
                ray.origin[axis] = value;
                assert!(ground.intersect(&ray).is_none());
                ray = downward_ray(Vector3::new(0.0, 0.0, 2.0));
                ray.direction[axis] = value;
                assert!(ground.intersect(&ray).is_none());
            }
        }
        let mut ray = downward_ray(Vector3::new(0.0, 0.0, 2.0));
        ray.direction = Vector3::zeros();
        assert!(ground.intersect(&ray).is_none());
    }

    #[test]
    fn overflowing_distance_is_rejected() {
        let ground = Ground::new(f64::MAX).unwrap();
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, -f64::MAX),
            Vector3::new(0.0, 0.0, 1.0),
        )
        .unwrap();
        assert!(ground.intersect(&ray).is_none());

        let ground = Ground::new(0.0).unwrap();
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, f64::MAX),
            Vector3::new(1.0, 0.0, -1e-10),
        )
        .unwrap();
        assert!(ground.intersect(&ray).is_none());
    }

    #[test]
    fn overflowing_hit_position_is_rejected() {
        let ground = Ground::new(0.0).unwrap();
        let ray = Ray::new(
            Vector3::new(f64::MAX, 0.0, f64::MAX / 2.0),
            Vector3::new(1.0, 0.0, -1.0),
        )
        .unwrap();
        assert!(ground.intersect(&ray).is_none());
    }
}

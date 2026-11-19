//! 由球心和半径（米）定义的解析球面；几何法线始终朝外。

use super::hit::GeometryHit as Hit;
use super::object::{ClosedVolume, Hittable, PointLocation};
use super::ray::INTERSECTION_EPSILON;
use super::ray::Ray;
use nalgebra::Vector3;

#[derive(Debug, Clone, Copy)]
pub struct Sphere {
    center: Vector3<f64>,
    radius: f64,
}

impl Sphere {
    /// 按球心和半径创建解析球面，供光线直接求交。
    ///
    /// 输入：有限球心 center（米）和正半径 radius（米）。
    ///
    /// 输出：Sphere；半径、半径平方或坐标范围无法有效表示时返回 None。
    pub fn new(center: Vector3<f64>, radius: f64) -> Option<Self> {
        let radius_squared = radius * radius;
        if !radius.is_finite()
            || radius <= 0.0
            || !radius_squared.is_finite()
            || radius_squared == 0.0
            || center
                .iter()
                .any(|v| !v.is_finite() || !(v - radius).is_finite() || !(v + radius).is_finite())
        {
            return None;
        }
        Some(Self { center, radius })
    }
}

impl ClosedVolume for Sphere {
    fn classify_point(&self, point: Vector3<f64>) -> Result<PointLocation, &'static str> {
        if !point.iter().all(|x| x.is_finite()) {
            return Err("non-finite launch point");
        }
        let d = point - self.center;
        let distance = d.x.hypot(d.y).hypot(d.z);
        // Roundoff band in metres, scaled to the radius rather than the ray's self-hit cutoff.
        let tolerance = 8.0 * f64::EPSILON * self.radius;
        Ok(if (distance - self.radius).abs() <= tolerance {
            PointLocation::OnBoundary
        } else if distance < self.radius {
            PointLocation::Inside
        } else {
            PointLocation::Outside
        })
    }
}

impl Hittable for Sphere {
    fn closed_volume(&self) -> Option<&dyn ClosedVolume> {
        Some(self)
    }

    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        self.intersect_after(ray, INTERSECTION_EPSILON)
    }

    fn intersect_from_volume(&self, ray: &Ray) -> Option<Hit> {
        self.intersect_after(ray, 0.0)
    }
}

impl Sphere {
    /// 求光线与球面最近的正向交点；从球内出发时返回出射交点。
    ///
    /// 输入：Ray；输出：最近的 Hit，未命中或数值无效返回 None；法线始终朝球外。
    ///
    /// 公式：令 q=o-center、a=d·d、b=q·d，则 t=(-b±√D)/a，D=a·radius²-‖q×d‖²。
    /// 依次尝试较小、较大根，只接受 t>min_distance；n=normalize(P-center)。
    fn intersect_after(&self, ray: &Ray, min_distance: f64) -> Option<Hit> {
        if ray
            .origin
            .iter()
            .chain(ray.direction.iter())
            .any(|v| !v.is_finite())
        {
            return None;
        }
        let offset = ray.origin - self.center;
        let a = ray.direction.norm_squared();
        let half_b = offset.dot(&ray.direction);
        // 等价于 half_b² - a*c，但避免远处轴向光线中两个大数相减丢失半径。
        let discriminant =
            a * self.radius * self.radius - offset.cross(&ray.direction).norm_squared();
        if !a.is_finite()
            || a <= 0.0
            || !half_b.is_finite()
            || !discriminant.is_finite()
            || discriminant < 0.0
        {
            return None;
        }
        let midpoint = -half_b / a;
        let half_chord = discriminant.sqrt() / a;
        let roots = [midpoint - half_chord, midpoint + half_chord];
        let distance = roots
            .into_iter()
            .find(|t| t.is_finite() && *t > min_distance)?;
        let coords = ray.at(distance);
        let outward = coords - self.center;
        let length = outward.norm();
        if !coords.iter().all(|v| v.is_finite()) || !length.is_finite() || length == 0.0 {
            return None;
        }
        Some(Hit {
            coords,
            normal: outward / length,
            distance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_entry_and_inside_exit_keep_outward_unit_normals() {
        let center = Vector3::new(2.0, -3.0, 4.0);
        let sphere = Sphere::new(center, 2.0).unwrap();
        for axis in [
            Vector3::x(),
            Vector3::y(),
            Vector3::z(),
            Vector3::new(1.0, 2.0, 3.0).normalize(),
        ] {
            for (origin, direction, distance, entering) in [
                (center + axis * 5.0, -axis, 3.0, true),
                (center, axis, 2.0, false),
                (center + axis * 0.5, axis, 1.5, false),
            ] {
                let ray = Ray::new(origin, direction).unwrap();
                let hit = sphere.intersect(&ray).unwrap();
                assert!((hit.distance - distance).abs() < 1e-12);
                assert!((hit.coords - (center + axis * 2.0)).norm() < 1e-12);
                assert!((hit.normal - axis).norm() < 1e-12);
                assert!((hit.normal.norm() - 1.0).abs() < 1e-12);
                assert_eq!(hit.normal.dot(&ray.direction) < 0.0, entering);
            }
        }
    }

    #[test]
    fn tangent_miss_away_surface_and_epsilon_select_the_nearest_valid_root() {
        let sphere = Sphere::new(Vector3::zeros(), 1.0).unwrap();
        for (origin, direction, expected) in [
            (Vector3::new(-2.0, 1.0, 0.0), Vector3::x(), Some(2.0)),
            (Vector3::new(-2.0, 1.001, 0.0), Vector3::x(), None),
            (Vector3::x() * 2.0, Vector3::x(), None),
            (Vector3::x(), Vector3::x(), None),
            (Vector3::x(), -Vector3::x(), Some(2.0)),
            (
                Vector3::x() * (1.0 + INTERSECTION_EPSILON / 2.0),
                -Vector3::x(),
                Some(2.0 + INTERSECTION_EPSILON / 2.0),
            ),
            (Vector3::x() * 1e8, -Vector3::x(), Some(1e8 - 1.0)),
        ] {
            let hit = sphere.intersect(&Ray::new(origin, direction).unwrap());
            match expected {
                Some(distance) => assert!((hit.unwrap().distance - distance).abs() < 1e-8),
                None => assert!(hit.is_none()),
            }
        }
    }

    #[test]
    fn invalid_geometry_rays_and_overflow_are_rejected() {
        for radius in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MAX, 1e-200] {
            assert!(Sphere::new(Vector3::zeros(), radius).is_none());
        }
        let sphere = Sphere::new(Vector3::zeros(), 1.0).unwrap();
        let ray = Ray::new(Vector3::x() * 2.0, -Vector3::x()).unwrap();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Sphere::new(Vector3::repeat(value), 1.0).is_none());
            let mut bad = ray;
            bad.origin.y = value;
            assert!(sphere.intersect(&bad).is_none());
            bad = ray;
            bad.direction.x = value;
            assert!(sphere.intersect(&bad).is_none());
        }
        let mut bad = ray;
        bad.direction = Vector3::zeros();
        assert!(sphere.intersect(&bad).is_none());
        bad = ray;
        bad.origin = Vector3::repeat(f64::MAX);
        assert!(sphere.intersect(&bad).is_none());
    }
}

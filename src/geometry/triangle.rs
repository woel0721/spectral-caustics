//! 单个三角形与双面的 Möller–Trumbore 求交。

use super::hit::GeometryHit as Hit;
use super::object::Hittable;
use super::ray::INTERSECTION_EPSILON;
use super::ray::PARALLEL_EPSILON;
use super::ray::Ray;
use nalgebra::Vector3;

#[derive(Debug, Clone, Copy)]
pub struct Triangle {
    /// 按顶点顺序存储；字段保持私有，使顶点与缓存的边、法线始终一致。
    vertices: [Vector3<f64>; 3],
    /// edge1 = v1 - v0，edge2 = v2 - v0。
    edge1: Vector3<f64>,
    edge2: Vector3<f64>,
    /// 单位几何法线，方向由 edge1 × edge2 决定，不随入射方向翻转。
    normal: Vector3<f64>,
}

impl Triangle {
    /// Signed distance to this face plane, with its dot-product roundoff scale.
    pub(super) fn plane_distance(&self, point: Vector3<f64>) -> Result<(f64, f64), &'static str> {
        let offset = point - self.vertices[0];
        let distance = self.normal.dot(&offset);
        let tolerance = 8.0 * f64::EPSILON * self.normal.abs().dot(&offset.abs());
        if !distance.is_finite() || !tolerance.is_finite() {
            return Err("non-finite face-plane distance");
        }
        Ok((distance, tolerance))
    }

    /// 用三个世界坐标顶点创建三角形，并缓存边与单位法线，减少重复求交的计算。
    ///
    /// 输入：按绕序给出的 v0、v1、v2（米）。
    ///
    /// 输出：Triangle；顶点非有限、三点共线或计算溢出时返回 None。
    ///
    /// 公式：e1=v1-v0，e2=v2-v0，n=(e1×e2)/‖e1×e2‖；顶点绕序决定法线方向。
    pub fn new(v0: Vector3<f64>, v1: Vector3<f64>, v2: Vector3<f64>) -> Option<Self> {
        let vertices = [v0, v1, v2];
        if !vertices.iter().all(|v| v.iter().all(|c| c.is_finite())) {
            return None;
        }

        let edge1 = v1 - v0;
        let edge2 = v2 - v0;
        let cross = edge1.cross(&edge2);
        let normal_length = cross.norm();
        if !normal_length.is_finite() || normal_length == 0.0 {
            return None;
        }

        Some(Self {
            vertices,
            edge1,
            edge2,
            normal: cross / normal_length,
        })
    }
}

impl Hittable for Triangle {
    /// 求光线与三角形的最近正向交点，两面都能命中，供封闭玻璃寻找入射面和出射面。
    ///
    /// 输入：世界空间 Ray；输出：坐标、距离和原始几何法线组成的 Hit，未命中或数值无效返回 None。
    ///
    /// 公式：解 o+t·d=v0+u·e1+v·e2；仅接受 u≥0、v≥0、u+v≤1 和 t>ε。
    /// 使用 Möller–Trumbore 的叉积与点积求 t/u/v；法线不随光线方向翻转。
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        if !ray.origin.iter().all(|c| c.is_finite()) || !ray.direction.iter().all(|c| c.is_finite())
        {
            return None;
        }

        // 用单位法线检测平行，阈值不随三角形面积变化。
        // 取绝对值以保留双面求交，玻璃内部的光线也能找到出射面。
        let facing = ray.direction.dot(&self.normal);
        if !facing.is_finite() || facing.abs() <= PARALLEL_EPSILON {
            return None;
        }

        // 解 origin + t * direction = v0 + u * edge1 + v * edge2。
        let p = ray.direction.cross(&self.edge2);
        let det = self.edge1.dot(&p);
        if !det.is_finite() || det == 0.0 {
            return None;
        }

        let offset = ray.origin - self.vertices[0];
        let u = offset.dot(&p) / det;
        if !u.is_finite() || !(0.0..=1.0).contains(&u) {
            return None;
        }

        let q = offset.cross(&self.edge1);
        let v = ray.direction.dot(&q) / det;
        if !v.is_finite() || v < 0.0 || u + v > 1.0 {
            return None;
        }

        let distance = self.edge2.dot(&q) / det;
        if !distance.is_finite() || distance <= INTERSECTION_EPSILON {
            return None;
        }
        let coords = ray.at(distance);
        if !coords.iter().all(|c| c.is_finite()) {
            return None;
        }

        // Hit 只记录结果；几何计算与合法性检查都在求交算法中完成。
        // 物体/材质编号由场景查询补充，这里只填写几何结果。
        Some(Hit {
            coords,
            normal: self.normal,
            distance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_triangle() -> Triangle {
        Triangle::new(
            Vector3::zeros(),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap()
    }

    fn downward_ray(x: f64, y: f64, z: f64) -> Ray {
        Ray::new(Vector3::new(x, y, z), Vector3::new(0.0, 0.0, -1.0)).unwrap()
    }

    #[test]
    fn front_hit_has_expected_distance_position_and_normal() {
        let ray = downward_ray(0.25, 0.25, 2.0);
        let hit = unit_triangle().intersect(&ray).unwrap();

        assert!((hit.distance - 2.0).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(0.25, 0.25, 0.0)).norm() < 1e-12);
        assert!((hit.normal - Vector3::new(0.0, 0.0, 1.0)).norm() < 1e-12);
    }

    #[test]
    fn back_hit_preserves_geometric_normal() {
        let ray = Ray::new(Vector3::new(0.25, 0.25, -2.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let hit = unit_triangle().intersect(&ray).unwrap();

        assert!((hit.distance - 2.0).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(0.25, 0.25, 0.0)).norm() < 1e-12);
        assert_eq!(hit.normal, Vector3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn reversing_vertex_order_reverses_normal_without_losing_hit() {
        let triangle = Triangle::new(
            Vector3::zeros(),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let hit = triangle.intersect(&downward_ray(0.25, 0.25, 2.0)).unwrap();

        assert!((hit.distance - 2.0).abs() < 1e-12);
        assert_eq!(hit.normal, Vector3::new(0.0, 0.0, -1.0));
    }

    #[test]
    fn oblique_ray_hits_expected_point() {
        let ray = Ray::new(Vector3::new(-0.75, 0.25, 1.0), Vector3::new(1.0, 0.0, -1.0)).unwrap();
        let hit = unit_triangle().intersect(&ray).unwrap();

        assert!((hit.distance - 2.0_f64.sqrt()).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(0.25, 0.25, 0.0)).norm() < 1e-12);
    }

    #[test]
    fn points_outside_each_edge_miss() {
        let triangle = unit_triangle();
        for (x, y) in [(-0.1, 0.25), (0.25, -0.1), (0.75, 0.75), (1.1, 0.0)] {
            assert!(triangle.intersect(&downward_ray(x, y, 2.0)).is_none());
        }
    }

    #[test]
    fn vertices_and_edges_are_included() {
        let triangle = unit_triangle();
        for (x, y) in [
            (0.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (0.5, 0.0),
            (0.0, 0.5),
            (0.5, 0.5),
        ] {
            let hit = triangle.intersect(&downward_ray(x, y, 2.0)).unwrap();
            assert!((hit.coords - Vector3::new(x, y, 0.0)).norm() < 1e-12);
        }
    }

    #[test]
    fn parallel_nearly_parallel_and_coplanar_rays_miss() {
        let triangle = unit_triangle();
        for (z, direction_z) in [(1.0, 0.0), (1.0, -1e-13), (0.0, 0.0)] {
            let ray = Ray::new(
                Vector3::new(0.25, 0.25, z),
                Vector3::new(1.0, 0.0, direction_z),
            )
            .unwrap();
            assert!(triangle.intersect(&ray).is_none());
        }
    }

    #[test]
    fn intersection_behind_ray_origin_is_rejected() {
        let ray = Ray::new(Vector3::new(0.25, 0.25, 2.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();

        assert!(unit_triangle().intersect(&ray).is_none());
    }

    #[test]
    fn self_intersections_are_rejected() {
        let triangle = unit_triangle();
        for z in [0.0, INTERSECTION_EPSILON / 2.0, INTERSECTION_EPSILON] {
            assert!(triangle.intersect(&downward_ray(0.25, 0.25, z)).is_none());
        }
        assert!(
            triangle
                .intersect(&downward_ray(0.25, 0.25, 2.0 * INTERSECTION_EPSILON))
                .is_some()
        );
    }

    #[test]
    fn duplicate_and_collinear_vertices_are_rejected() {
        assert!(Triangle::new(Vector3::zeros(), Vector3::zeros(), Vector3::zeros()).is_none());
        assert!(
            Triangle::new(
                Vector3::zeros(),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(2.0, 0.0, 0.0),
            )
            .is_none()
        );
    }

    #[test]
    fn non_finite_vertices_are_rejected() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for vertex_index in 0..3 {
                for axis in 0..3 {
                    let mut vertices = unit_triangle().vertices;
                    vertices[vertex_index][axis] = value;
                    assert!(Triangle::new(vertices[0], vertices[1], vertices[2]).is_none());
                }
            }
        }
    }

    #[test]
    fn overflow_during_triangle_construction_is_rejected() {
        assert!(
            Triangle::new(
                Vector3::new(-f64::MAX, 0.0, 0.0),
                Vector3::new(f64::MAX, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            )
            .is_none()
        );
    }

    #[test]
    fn non_finite_and_zero_ray_directions_are_rejected() {
        let triangle = unit_triangle();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for axis in 0..3 {
                let mut ray = downward_ray(0.25, 0.25, 2.0);
                ray.origin[axis] = value;
                assert!(triangle.intersect(&ray).is_none());
                ray = downward_ray(0.25, 0.25, 2.0);
                ray.direction[axis] = value;
                assert!(triangle.intersect(&ray).is_none());
            }
        }
        let mut ray = downward_ray(0.25, 0.25, 2.0);
        ray.direction = Vector3::zeros();
        assert!(triangle.intersect(&ray).is_none());
    }

    #[test]
    fn small_triangle_is_not_rejected_by_an_absolute_area_threshold() {
        let size = 1e-8;
        let triangle = Triangle::new(
            Vector3::zeros(),
            Vector3::new(size, 0.0, 0.0),
            Vector3::new(0.0, size, 0.0),
        )
        .unwrap();
        let ray = downward_ray(0.25 * size, 0.25 * size, 1.0);
        let hit = triangle.intersect(&ray).unwrap();

        assert!((hit.distance - 1.0).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(0.25 * size, 0.25 * size, 0.0)).norm() < 1e-20);
    }
}

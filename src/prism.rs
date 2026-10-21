//! 固定三角截面沿 y 轴拉伸的封闭三棱镜，共 8 个三角形。

use crate::{hit::Hit, object::Hittable, ray::Ray, triangle::Triangle};
use nalgebra::Vector3;

#[derive(Debug)]
pub struct Prism {
    /// 三角形与缓存法线由 Triangle 管理，外部不能单独修改顶点。
    faces: Vec<Triangle>,
}

impl Prism {
    /// 沿用固定的 6 个顶点，长度单位为米；整个三棱镜位于 z = 0 地面上方。
    /// 与 Triangle::new 保持一致，任何面构造失败时返回 None。
    pub fn new() -> Option<Self> {
        // 前端三角形位于 y = -1，后端位于 y = 1。
        let vertices = [
            Vector3::new(-0.5, -1.0, 1.0), // 0
            Vector3::new(0.5, -1.0, 1.0),  // 1
            Vector3::new(0.0, -1.0, 1.8),  // 2
            Vector3::new(-0.5, 1.0, 1.0),  // 3
            Vector3::new(0.5, 1.0, 1.0),   // 4
            Vector3::new(0.0, 1.0, 1.8),   // 5
        ];

        // 两个端面各 1 个三角形，三个矩形侧面各拆成 2 个三角形。
        // 顺序保证 (vb - va) × (vc - va) 朝向三棱镜外部。
        let indices = [
            [0, 1, 2], // 前端，法线 -y
            [3, 5, 4], // 后端，法线 +y
            [0, 3, 4], // 底面，法线 -z
            [0, 4, 1],
            [1, 4, 5], // 右斜面，法线朝 +x、+z
            [1, 5, 2],
            [2, 5, 3], // 左斜面，法线朝 -x、+z
            [2, 3, 0],
        ];

        // collect 将多个 Option<Triangle> 合并成 Option<Vec<Triangle>>。
        // 任意一个面失败，? 就让整个构造函数返回 None。
        let faces = indices
            .iter()
            .map(|&[a, b, c]| Triangle::new(vertices[a], vertices[b], vertices[c]))
            .collect::<Option<Vec<_>>>()?;

        Some(Self { faces })
    }
}

impl Hittable for Prism {
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        // Triangle 已检查正向距离、epsilon 和非有限值。
        // 必须检查所有面再比较距离，索引靠前的面不一定离光线起点最近。
        self.faces
            .iter()
            .filter_map(|face| face.intersect(ray))
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_eight_faces_are_closed_with_outward_unit_normals() {
        let prism = Prism::new().unwrap();
        assert_eq!(prism.faces.len(), 8);
        let center = Vector3::new(0.0, 0.0, 1.2);
        // 每个三角形内选一点，从内部射向它；预期法线由对应平面独立给出。
        let cases = [
            (Vector3::new(0.0, -1.0, 1.2), Vector3::new(0.0, -1.0, 0.0)),
            (Vector3::new(0.0, 1.0, 1.2), Vector3::new(0.0, 1.0, 0.0)),
            (Vector3::new(-0.2, 0.6, 1.0), Vector3::new(0.0, 0.0, -1.0)),
            (Vector3::new(0.2, -0.6, 1.0), Vector3::new(0.0, 0.0, -1.0)),
            (Vector3::new(0.375, 0.6, 1.2), Vector3::new(1.6, 0.0, 1.0)),
            (Vector3::new(0.125, -0.6, 1.6), Vector3::new(1.6, 0.0, 1.0)),
            (Vector3::new(-0.125, 0.6, 1.6), Vector3::new(-1.6, 0.0, 1.0)),
            (
                Vector3::new(-0.375, -0.6, 1.2),
                Vector3::new(-1.6, 0.0, 1.0),
            ),
        ];

        for (point, normal) in cases {
            let ray = Ray::new(center, point - center, 0.0).unwrap();
            let hit = prism.intersect(&ray).unwrap();

            assert!((hit.coords - point).norm() < 1e-12);
            assert!((hit.distance - (point - center).norm()).abs() < 1e-12);
            assert!((hit.normal - normal.normalize()).norm() < 1e-12);
            assert!(hit.normal.dot(&ray.direction) > 0.0);
        }
    }

    #[test]
    fn nearest_surface_is_selected_instead_of_first_face() {
        let ray = Ray::new(
            Vector3::new(0.0, 3.0, 1.2),
            Vector3::new(0.0, -1.0, 0.0),
            0.0,
        )
        .unwrap();
        let hit = Prism::new().unwrap().intersect(&ray).unwrap();

        // 索引中的前端面更早被遍历，但后端面才是距离为 2 米的入口。
        assert!((hit.distance - 2.0).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(0.0, 1.0, 1.2)).norm() < 1e-12);
        assert_eq!(hit.normal, Vector3::new(0.0, 1.0, 0.0));
        assert_eq!(hit.object_id, None);
        assert_eq!(hit.material_id, None);
    }

    #[test]
    fn downward_ray_hits_sloping_face_before_bottom() {
        let ray = Ray::new(
            Vector3::new(0.25, 0.2, 3.0),
            Vector3::new(0.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();
        let hit = Prism::new().unwrap().intersect(&ray).unwrap();

        assert!((hit.distance - 1.6).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(0.25, 0.2, 1.4)).norm() < 1e-12);
        assert!((hit.normal - Vector3::new(1.6, 0.0, 1.0).normalize()).norm() < 1e-12);
    }

    #[test]
    fn ridge_and_shared_edges_are_not_holes() {
        let prism = Prism::new().unwrap();
        for (x, y, expected_z) in [(0.0, 0.0, 1.8), (0.5, 0.0, 1.0), (0.0, -1.0, 1.8)] {
            let ray = Ray::new(Vector3::new(x, y, 3.0), Vector3::new(0.0, 0.0, -1.0), 0.0).unwrap();
            let hit = prism.intersect(&ray).unwrap();
            assert!((hit.coords - Vector3::new(x, y, expected_z)).norm() < 1e-12);
        }
    }

    #[test]
    fn surface_start_skips_self_hit_and_can_reach_opposite_face() {
        let prism = Prism::new().unwrap();
        let origin = Vector3::new(0.0, -1.0, 1.2);
        let outward = Ray::new(origin, Vector3::new(0.0, -1.0, 0.0), 0.0).unwrap();
        let inward = Ray::new(origin, Vector3::new(0.0, 1.0, 0.0), 0.0).unwrap();

        assert!(prism.intersect(&outward).is_none());
        let exit = prism.intersect(&inward).unwrap();
        assert!((exit.distance - 2.0).abs() < 1e-12);
        assert_eq!(exit.normal, Vector3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn outside_and_away_rays_miss() {
        let prism = Prism::new().unwrap();
        for (origin, direction) in [
            (Vector3::new(2.0, 0.0, 3.0), Vector3::new(0.0, 0.0, -1.0)),
            (Vector3::new(2.0, 0.0, 1.2), Vector3::new(0.0, 1.0, 0.0)),
            (Vector3::new(0.0, 0.0, 3.0), Vector3::new(0.0, 0.0, 1.0)),
        ] {
            let ray = Ray::new(origin, direction, 0.0).unwrap();
            assert!(prism.intersect(&ray).is_none());
        }
    }

    #[test]
    fn non_finite_ray_is_rejected() {
        let prism = Prism::new().unwrap();
        let ray = Ray::new(
            Vector3::new(0.25, 0.2, 3.0),
            Vector3::new(0.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut bad_origin = ray;
            bad_origin.origin.x = value;
            assert!(prism.intersect(&bad_origin).is_none());
            let mut bad_direction = ray;
            bad_direction.direction.z = value;
            assert!(prism.intersect(&bad_direction).is_none());
        }
    }
}

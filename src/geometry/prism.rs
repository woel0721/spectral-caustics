//! 默认三角截面沿 Y 轴拉伸；from_vertices 可构建任意朝向的凸三棱柱，共 8 个三角形。

use super::hit::GeometryHit as Hit;
use super::object::{ClosedVolume, Hittable, PointLocation};
use super::ray::Ray;
use super::triangle::Triangle;
use nalgebra::Vector3;

#[derive(Debug)]
pub struct Prism {
    /// 三角形与缓存法线由 Triangle 管理，外部不能单独修改顶点。
    faces: Vec<Triangle>,
}

impl Prism {
    /// 创建内置六顶点三棱镜，供不读取 JSON 的几何示例使用。
    ///
    /// 输入：无；输出：沿 Y 轴从 -1 m 拉伸至 1 m 的三棱镜；面构建失败返回 None。
    /// 三角截面位于 XZ 平面，底边 z=1 m，顶点 z=1.8 m。
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
        Self::from_vertices(vertices)
    }

    /// 把六个顶点连接成封闭凸三棱柱，供不同朝向的三棱镜复用求交。
    ///
    /// 输入：两个三角端面的顶点 `[0,1,2]` 和 `[3,4,5]`，对应顶点以 0↔3、1↔4、2↔5 相连。
    ///
    /// 输出：含 8 个三角面的 Prism；退化、绕序错误、非凸或数值无效返回 None。
    /// 校验公式：每面外法线 n 须满足 (center-va)·n<0，且所有顶点均在该面的内部半空间。
    pub fn from_vertices(vertices: [Vector3<f64>; 6]) -> Option<Self> {
        // 两个端面各 1 个三角形，三个矩形侧面各拆成 2 个三角形。
        // 顺序保证 (vb - va) × (vc - va) 朝向三棱镜外部。
        let indices = [
            [0, 1, 2], // 首个三角形端面
            [3, 5, 4], // 对应的另一个端面，绕序相反
            [0, 3, 4], // 三个矩形侧面，各拆成两个三角形
            [0, 4, 1],
            [1, 4, 5],
            [1, 5, 2],
            [2, 5, 3],
            [2, 3, 0],
        ];

        // collect 将多个 Option<Triangle> 合并成 Option<Vec<Triangle>>。
        // 任意一个面失败，? 就让整个构造函数返回 None。
        let faces = indices
            .iter()
            .map(|&[a, b, c]| Triangle::new(vertices[a], vertices[b], vertices[c]))
            .collect::<Option<Vec<_>>>()?;

        let center = vertices
            .iter()
            .fold(Vector3::zeros(), |sum, v| sum + v / 6.0);
        for [a, b, c] in indices {
            let cross = (vertices[b] - vertices[a]).cross(&(vertices[c] - vertices[a]));
            let normal = cross / cross.norm();
            let inside = (center - vertices[a]).dot(&normal);
            if !inside.is_finite() || inside >= -1e-10 {
                return None;
            }
            if vertices.iter().any(|v| {
                let distance = (v - vertices[a]).dot(&normal);
                !distance.is_finite() || distance > 1e-8
            }) {
                return None;
            }
        }

        Some(Self { faces })
    }
}

impl ClosedVolume for Prism {
    fn classify_point(&self, point: Vector3<f64>) -> Result<PointLocation, &'static str> {
        let mut location = PointLocation::Inside;
        for face in &self.faces {
            let (distance, tolerance) = face.plane_distance(point)?;
            if distance > tolerance {
                return Ok(PointLocation::Outside);
            }
            if distance >= -tolerance {
                location = PointLocation::OnBoundary;
            }
        }
        Ok(location)
    }
}

impl Hittable for Prism {
    fn closed_volume(&self) -> Option<&dyn ClosedVolume> {
        Some(self)
    }

    /// 查询三棱镜各三角面，返回离 Ray 起点最近的正向 Hit。
    ///
    /// 输入：世界空间 Ray；输出：最近交点，所有面都未命中时返回 None。
    /// 取 min(t) 而非第一个命中的面，使结果与面存储顺序无关。
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
            let ray = Ray::new(center, point - center).unwrap();
            let hit = prism.intersect(&ray).unwrap();

            assert!((hit.coords - point).norm() < 1e-12);
            assert!((hit.distance - (point - center).norm()).abs() < 1e-12);
            assert!((hit.normal - normal.normalize()).norm() < 1e-12);
            assert!(hit.normal.dot(&ray.direction) > 0.0);
        }
    }

    #[test]
    fn nearest_surface_is_selected_instead_of_first_face() {
        let ray = Ray::new(Vector3::new(0.0, 3.0, 1.2), Vector3::new(0.0, -1.0, 0.0)).unwrap();
        let hit = Prism::new().unwrap().intersect(&ray).unwrap();

        // 索引中的前端面更早被遍历，但后端面才是距离为 2 米的入口。
        assert!((hit.distance - 2.0).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(0.0, 1.0, 1.2)).norm() < 1e-12);
        assert_eq!(hit.normal, Vector3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn downward_ray_hits_sloping_face_before_bottom() {
        let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
        let hit = Prism::new().unwrap().intersect(&ray).unwrap();

        assert!((hit.distance - 1.6).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(0.25, 0.2, 1.4)).norm() < 1e-12);
        assert!((hit.normal - Vector3::new(1.6, 0.0, 1.0).normalize()).norm() < 1e-12);
    }

    #[test]
    fn ridge_and_shared_edges_are_not_holes() {
        let prism = Prism::new().unwrap();
        for (x, y, expected_z) in [(0.0, 0.0, 1.8), (0.5, 0.0, 1.0), (0.0, -1.0, 1.8)] {
            let ray = Ray::new(Vector3::new(x, y, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
            let hit = prism.intersect(&ray).unwrap();
            assert!((hit.coords - Vector3::new(x, y, expected_z)).norm() < 1e-12);
        }
    }

    #[test]
    fn surface_start_skips_self_hit_and_can_reach_opposite_face() {
        let prism = Prism::new().unwrap();
        let origin = Vector3::new(0.0, -1.0, 1.2);
        let outward = Ray::new(origin, Vector3::new(0.0, -1.0, 0.0)).unwrap();
        let inward = Ray::new(origin, Vector3::new(0.0, 1.0, 0.0)).unwrap();

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
            let ray = Ray::new(origin, direction).unwrap();
            assert!(prism.intersect(&ray).is_none());
        }
    }

    #[test]
    fn non_finite_ray_is_rejected() {
        let prism = Prism::new().unwrap();
        let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
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

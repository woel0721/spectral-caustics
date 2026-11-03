//! 三角截面的三个顶点各截去一角，再沿 y 拉伸；只倒三条长棱。

use super::{
    hit::Hit,
    object::Hittable,
    ray::{INTERSECTION_EPSILON, PARALLEL_EPSILON, Ray},
    triangle::Triangle,
};
use nalgebra::{Vector2, Vector3};

// V0..V5 为 y_min 的逆时针 x/z 截面，V6..V11 为 y_max 的同序副本。
// 每条边由恰好两个反向的有向边共享；不引入端面中心点或 T 接点。
const FACE_INDICES: [[usize; 3]; 20] = [
    [0, 6, 7],
    [0, 7, 1],
    [1, 7, 8],
    [1, 8, 2],
    [2, 8, 9],
    [2, 9, 3],
    [3, 9, 10],
    [3, 10, 4],
    [4, 10, 11],
    [4, 11, 5],
    [5, 11, 6],
    [5, 6, 0],
    [0, 1, 2],
    [0, 2, 3],
    [0, 3, 4],
    [0, 4, 5],
    [6, 8, 7],
    [6, 9, 8],
    [6, 10, 9],
    [6, 11, 10],
];

#[derive(Debug)]
pub struct BeveledPrism {
    vertices: [Vector3<f64>; 12],
    planes: [Plane; 8],
}

#[derive(Debug, Clone, Copy)]
struct Plane {
    point: Vector3<f64>,
    normal: Vector3<f64>,
}

impl BeveledPrism {
    /// cross_section 的 Vector2 分量为 (x,z)，所有长度以米计。
    /// 顺/逆时针均可；内部统一为逆时针，并保留第一个输入顶点。
    /// bevel_size 是沿两条相邻原始边的截取长度，不是倒角面宽或圆角半径。
    /// 要求有限、非退化三角形，y_min < y_max，且 0 < 2*bevel_size < 每条边长。
    /// 支持 0.02..0.05 m；其他满足上述几何条件的正值也可使用。非法输入返回 None。
    pub fn new(
        cross_section: [Vector2<f64>; 3],
        y_min: f64,
        y_max: f64,
        bevel_size: f64,
    ) -> Option<Self> {
        let vertices = mesh_vertices(cross_section, y_min, y_max, bevel_size)?;
        for [a, b, c] in FACE_INDICES {
            Triangle::new(vertices[a], vertices[b], vertices[c])?;
        }
        let planes = std::array::from_fn(|i| {
            if i < 6 {
                let edge = vertices[(i + 1) % 6] - vertices[i];
                Plane {
                    point: vertices[i],
                    normal: Vector3::new(edge.z, 0.0, -edge.x).normalize(),
                }
            } else {
                Plane {
                    point: vertices[if i == 6 { 0 } else { 6 }],
                    normal: if i == 6 { -Vector3::y() } else { Vector3::y() },
                }
            }
        });
        if planes
            .iter()
            .any(|plane| plane.normal.iter().any(|v| !v.is_finite()))
        {
            return None;
        }
        Some(Self { vertices, planes })
    }

    /// 只读 indexed mesh：前六个顶点在 y_min，后六个在 y_max。
    pub fn vertices(&self) -> &[Vector3<f64>; 12] {
        &self.vertices
    }

    /// 六个侧矩形各两片，前后六边形各四片；索引对应 vertices()。
    pub fn triangle_indices(&self) -> &[[usize; 3]; 20] {
        &FACE_INDICES
    }
}

fn mesh_vertices(
    mut section: [Vector2<f64>; 3],
    y_min: f64,
    y_max: f64,
    bevel: f64,
) -> Option<[Vector3<f64>; 12]> {
    let length_y = y_max - y_min;
    if section.iter().flatten().any(|v| !v.is_finite())
        || !y_min.is_finite()
        || !y_max.is_finite()
        || !length_y.is_finite()
        || length_y <= 0.0
        || !bevel.is_finite()
        || bevel <= 0.0
    {
        return None;
    }
    let area_twice = (section[1] - section[0]).perp(&(section[2] - section[0]));
    if !area_twice.is_finite() || area_twice == 0.0 {
        return None;
    }
    if area_twice < 0.0 {
        section.swap(1, 2);
    }
    let mut edges = [Vector2::zeros(); 3];
    for i in 0..3 {
        let edge = section[(i + 1) % 3] - section[i];
        let length = edge.norm();
        // 写成 bevel < length/2，避免 2*bevel 的溢出。
        if !length.is_finite() || bevel >= length * 0.5 {
            return None;
        }
        edges[i] = edge / length;
    }
    let mut ring = [Vector2::zeros(); 6];
    for i in 0..3 {
        ring[2 * i] = section[i] + edges[i] * bevel;
        ring[2 * i + 1] = section[(i + 1) % 3] - edges[i] * bevel;
    }
    // 也排除截取长度低于坐标精度、溢出等造成的重合/反向顶点。
    for i in 0..6 {
        let cross = (ring[(i + 1) % 6] - ring[i]).perp(&(ring[(i + 2) % 6] - ring[(i + 1) % 6]));
        if !cross.is_finite() || cross <= 0.0 {
            return None;
        }
    }
    Some(std::array::from_fn(|i| {
        let point = ring[i % 6];
        Vector3::new(point.x, if i < 6 { y_min } else { y_max }, point.y)
    }))
}

impl Hittable for BeveledPrism {
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        if ray
            .origin
            .iter()
            .chain(ray.direction.iter())
            .any(|v| !v.is_finite())
        {
            return None;
        }
        // 该 mesh 为凸体。直接裁剪八个外向半空间，避免共享三角形边上的重心舍入缝。
        // 平面通过顶点而非世界原点表示，减小平移后的消减误差。
        let mut enter = f64::NEG_INFINITY;
        let mut exit = f64::INFINITY;
        let mut enter_normal = None;
        let mut exit_normal = None;
        for plane in self.planes {
            let offset = ray.origin - plane.point;
            let signed_distance = offset.dot(&plane.normal);
            let facing = ray.direction.dot(&plane.normal);
            if !signed_distance.is_finite() || !facing.is_finite() {
                return None;
            }
            if facing.abs() <= PARALLEL_EPSILON {
                let roundoff = 32.0 * f64::EPSILON * offset.amax().max(1.0);
                if signed_distance > roundoff {
                    return None;
                }
                continue;
            }
            let distance = -signed_distance / facing;
            if !distance.is_finite() {
                return None;
            }
            if facing < 0.0 && distance > enter {
                enter = distance;
                enter_normal = Some(plane.normal);
            } else if facing > 0.0 && distance < exit {
                exit = distance;
                exit_normal = Some(plane.normal);
            }
        }
        // 仅容许浮点精度量级的区间差，不使用米制自相交 epsilon 扩大实体。
        let roundoff = 32.0 * f64::EPSILON * enter.abs().max(exit.abs()).max(1.0);
        if enter > exit + roundoff {
            return None;
        }
        let (distance, normal) = if enter > INTERSECTION_EPSILON {
            (enter, enter_normal?)
        } else if exit > INTERSECTION_EPSILON {
            (exit, exit_normal?)
        } else {
            return None;
        };
        let coords = ray.at(distance);
        if !distance.is_finite() || coords.iter().any(|v| !v.is_finite()) {
            return None;
        }
        Some(Hit {
            coords,
            normal,
            distance,
            ..Hit::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn section() -> [Vector2<f64>; 3] {
        [
            Vector2::new(-0.5, 0.03),
            Vector2::new(0.5, 0.03),
            Vector2::new(0.0, 0.83),
        ]
    }

    #[test]
    fn mesh_is_closed_with_twelve_shared_vertices_and_twenty_consistently_wound_triangles() {
        for bevel in [0.02, 0.035, 0.05] {
            let vertices = mesh_vertices(section(), -1.0, 1.0, bevel).unwrap();
            let prism = BeveledPrism::new(section(), -1.0, 1.0, bevel).unwrap();
            assert_eq!(prism.triangle_indices().len(), 20);
            assert_eq!(prism.vertices(), &vertices);
            let mut edges = BTreeMap::<_, Vec<_>>::new();
            for [a, b, c] in FACE_INDICES {
                assert!(a != b && b != c && c != a);
                for (u, v) in [(a, b), (b, c), (c, a)] {
                    edges.entry((u.min(v), u.max(v))).or_default().push((u, v));
                }
            }
            assert_eq!(edges.len(), 30);
            assert_eq!(vertices.len() + FACE_INDICES.len(), edges.len() + 2);
            for directions in edges.values() {
                assert_eq!(directions.len(), 2);
                assert_eq!(directions[0], (directions[1].1, directions[1].0));
            }
            for i in 0..6 {
                assert_eq!(vertices[i].x.to_bits(), vertices[i + 6].x.to_bits());
                assert_eq!(vertices[i].z.to_bits(), vertices[i + 6].z.to_bits());
                for j in i + 1..12 {
                    assert_ne!(vertices[i], vertices[j]);
                }
            }
        }
    }

    #[test]
    fn corner_cuts_follow_adjacent_edges_and_preserve_the_requested_metric_length() {
        let triangle = section();
        for bevel in [0.02, 0.05] {
            let vertices = mesh_vertices(triangle, -1.0, 1.0, bevel).unwrap();
            for i in 0..3 {
                let a = Vector2::new(vertices[2 * i].x, vertices[2 * i].z);
                let b = Vector2::new(vertices[2 * i + 1].x, vertices[2 * i + 1].z);
                let direction = (triangle[(i + 1) % 3] - triangle[i]).normalize();
                assert!((a - triangle[i] - direction * bevel).norm() < 1e-14);
                assert!((b - triangle[(i + 1) % 3] + direction * bevel).norm() < 1e-14);
            }
        }
    }

    #[test]
    fn all_faces_have_outward_unit_normals_and_enclose_the_expected_cut_volume() {
        let triangle = section();
        let bevel = 0.035;
        let vertices = mesh_vertices(triangle, -1.0, 1.0, bevel).unwrap();
        let center = vertices
            .iter()
            .fold(Vector3::zeros(), |sum, v| sum + v / 12.0);
        let prism = BeveledPrism::new(triangle, -1.0, 1.0, bevel).unwrap();
        let mut volume = 0.0;
        for (index, [a, b, c]) in FACE_INDICES.into_iter().enumerate() {
            let (a, b, c) = (vertices[a], vertices[b], vertices[c]);
            let normal = (b - a).cross(&(c - a)).normalize();
            let point = (a + b + c) / 3.0;
            assert!(normal.dot(&(point - center)) > 0.0);
            assert!(vertices.iter().all(|v| normal.dot(&(v - a)) <= 1e-12));
            let hit = prism
                .intersect(&Ray::new(center, point - center).unwrap())
                .unwrap();
            assert!((hit.coords - point).norm() < 1e-12);
            assert!((hit.normal - normal).norm() < 1e-12);
            assert!((hit.normal.norm() - 1.0).abs() < 1e-12);
            if index >= 12 {
                assert_eq!(
                    normal,
                    if index < 16 {
                        -Vector3::y()
                    } else {
                        Vector3::y()
                    }
                );
            }
            volume += (a - center).dot(&(b - center).cross(&(c - center))) / 6.0;
        }
        let original_area = (triangle[1] - triangle[0]).perp(&(triangle[2] - triangle[0])) * 0.5;
        let removed_area: f64 = (0..3)
            .map(|i| {
                let a = (triangle[(i + 1) % 3] - triangle[i]).normalize();
                let b = (triangle[(i + 2) % 3] - triangle[i]).normalize();
                bevel * bevel * a.perp(&b).abs() * 0.5
            })
            .sum();
        assert!((volume - (original_area - removed_area) * 2.0).abs() < 1e-12);
    }

    #[test]
    fn rays_hit_each_bevel_rectangle_from_outside_and_inside_with_the_same_outward_normal() {
        let vertices = mesh_vertices(section(), -1.0, 1.0, 0.02).unwrap();
        let prism = BeveledPrism::new(section(), -1.0, 1.0, 0.02).unwrap();
        let center = vertices
            .iter()
            .fold(Vector3::zeros(), |sum, v| sum + v / 12.0);
        for i in [1, 3, 5] {
            let j = (i + 1) % 6;
            let edge = vertices[j] - vertices[i];
            let normal = Vector3::new(edge.z, 0.0, -edge.x).normalize();
            for (t, y) in [(0.25, -0.5), (0.75, 0.5)] {
                let mut point = vertices[i] + edge * t;
                point.y = y;
                let entry = prism
                    .intersect(&Ray::new(point + normal * 0.2, -normal).unwrap())
                    .unwrap();
                let exit = prism
                    .intersect(&Ray::new(center, point - center).unwrap())
                    .unwrap();
                assert!((entry.distance - 0.2).abs() < 1e-12);
                for hit in [entry, exit] {
                    assert!((hit.coords - point).norm() < 1e-12);
                    assert!((hit.normal - normal).norm() < 1e-12);
                }
            }
        }
    }

    #[test]
    fn shared_triangle_edges_and_vertices_do_not_leave_ray_gaps() {
        let vertices = mesh_vertices(section(), -1.0, 1.0, 0.05).unwrap();
        let prism = BeveledPrism::new(section(), -1.0, 1.0, 0.05).unwrap();
        let center = vertices
            .iter()
            .fold(Vector3::zeros(), |sum, v| sum + v / 12.0);
        let mut points = vertices.to_vec();
        for [a, b, c] in FACE_INDICES {
            for (a, b) in [(a, b), (b, c), (c, a)] {
                for t in [0.25, 0.5, 0.75] {
                    points.push(vertices[a] * (1.0 - t) + vertices[b] * t);
                }
            }
        }
        for point in points {
            let hit = prism
                .intersect(&Ray::new(center, point - center).unwrap())
                .expect("共享顶点/边不能漏光");
            assert!((hit.coords - point).norm() < 1e-10);
        }
    }

    #[test]
    fn winding_is_normalized_and_surface_starts_skip_self_hits() {
        let a = BeveledPrism::new(section(), -1.0, 1.0, 0.03).unwrap();
        let mut reversed = section();
        reversed.swap(1, 2);
        let b = BeveledPrism::new(reversed, -1.0, 1.0, 0.03).unwrap();
        let point = Vector3::new(0.0, -1.0, 0.3);
        assert!(
            a.intersect(&Ray::new(point, -Vector3::y()).unwrap())
                .is_none()
        );
        let ray = Ray::new(point, Vector3::y()).unwrap();
        let exit = a.intersect(&ray).unwrap();
        assert!((exit.distance - 2.0).abs() < 1e-12);
        assert_eq!(exit.normal, Vector3::y());
        assert_eq!(exit.coords, b.intersect(&ray).unwrap().coords);
    }

    #[test]
    fn clipped_rays_reject_outside_and_removed_corners_but_keep_boundary_tangents() {
        let prism = BeveledPrism::new(section(), -1.0, 1.0, 0.03).unwrap();
        for (origin, direction) in [
            (Vector3::new(0.6, 0.0, 3.0), -Vector3::z()),
            (Vector3::new(-0.5, 0.0, 3.0), -Vector3::z()),
            (Vector3::new(-2.0, 1.0001, 0.3), Vector3::x()),
            (Vector3::new(0.0, -3.0, 0.3), -Vector3::y()),
        ] {
            assert!(
                prism
                    .intersect(&Ray::new(origin, direction).unwrap())
                    .is_none()
            );
        }
        for point in prism.vertices()[..6].iter() {
            let origin = Vector3::new(point.x, -3.0, point.z);
            let hit = prism
                .intersect(&Ray::new(origin, Vector3::y()).unwrap())
                .unwrap();
            assert!((hit.distance - 2.0).abs() < 1e-12);
            assert_eq!(hit.normal, -Vector3::y());
        }
        let mut zero = Ray::new(Vector3::new(0.0, 0.0, 0.3), Vector3::y()).unwrap();
        zero.direction = Vector3::zeros();
        assert!(prism.intersect(&zero).is_none());
    }

    #[test]
    fn invalid_or_collapsed_parameters_and_non_finite_rays_are_rejected() {
        for bevel in [0.0, -0.01, 0.5, 1.0, 1e-30, f64::NAN, f64::INFINITY] {
            assert!(BeveledPrism::new(section(), -1.0, 1.0, bevel).is_none());
        }
        for (y_min, y_max) in [
            (1.0, 1.0),
            (1.0, -1.0),
            (f64::NEG_INFINITY, 1.0),
            (-1.0, f64::NAN),
            (-f64::MAX, f64::MAX),
        ] {
            assert!(BeveledPrism::new(section(), y_min, y_max, 0.02).is_none());
        }
        for bad in [
            [Vector2::zeros(); 3],
            [Vector2::zeros(), Vector2::x(), Vector2::x() * 2.0],
            [
                Vector2::repeat(f64::MAX),
                Vector2::repeat(-f64::MAX),
                Vector2::zeros(),
            ],
        ] {
            assert!(BeveledPrism::new(bad, -1.0, 1.0, 0.02).is_none());
        }
        let prism = BeveledPrism::new(section(), -1.0, 1.0, 0.02).unwrap();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut bad = section();
            bad[0].x = value;
            assert!(BeveledPrism::new(bad, -1.0, 1.0, 0.02).is_none());
            let mut ray = Ray::new(Vector3::new(0.0, -3.0, 0.3), Vector3::y()).unwrap();
            ray.origin.z = value;
            assert!(prism.intersect(&ray).is_none());
            ray.origin.z = 0.3;
            ray.direction.x = value;
            assert!(prism.intersect(&ray).is_none());
        }
    }
}

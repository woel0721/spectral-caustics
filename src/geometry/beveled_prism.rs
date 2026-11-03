//! 三角截面的三个角各用 8 段相切圆弧替代，再沿 y 拉伸；只圆化三条长棱。

use super::{
    hit::Hit,
    object::Hittable,
    ray::{INTERSECTION_EPSILON, PARALLEL_EPSILON, Ray},
    triangle::Triangle,
};
use nalgebra::{Vector2, Vector3};

/// 每个圆角的等角度圆弧段数，端点也作为共享 mesh 顶点保存。
pub const ROUND_SEGMENTS: usize = 8;
const ARC_POINTS: usize = ROUND_SEGMENTS + 1;
const RING_VERTICES: usize = 3 * ARC_POINTS;
const VERTEX_COUNT: usize = 2 * RING_VERTICES;
const TRIANGLE_COUNT: usize = 4 * RING_VERTICES - 4;
const FACE_INDICES: [[usize; 3]; TRIANGLE_COUNT] = face_indices();

// 两个截面共用索引拓扑，不引入端面中心点或 T 接点。
const fn face_indices() -> [[usize; 3]; TRIANGLE_COUNT] {
    let mut faces = [[0; 3]; TRIANGLE_COUNT];
    let mut i = 0;
    while i < RING_VERTICES {
        let j = (i + 1) % RING_VERTICES;
        faces[2 * i] = [i, i + RING_VERTICES, j + RING_VERTICES];
        faces[2 * i + 1] = [i, j + RING_VERTICES, j];
        i += 1;
    }
    let mut k = 1;
    while k < RING_VERTICES - 1 {
        faces[2 * RING_VERTICES + k - 1] = [0, k, k + 1];
        faces[3 * RING_VERTICES - 2 + k - 1] =
            [RING_VERTICES, RING_VERTICES + k + 1, RING_VERTICES + k];
        k += 1;
    }
    faces
}

#[derive(Debug)]
pub struct BeveledPrism {
    vertices: [Vector3<f64>; VERTEX_COUNT],
    planes: [Plane; RING_VERTICES + 2],
}

#[derive(Debug, Clone, Copy)]
struct Plane {
    point: Vector3<f64>,
    normal: Vector3<f64>,
}

impl BeveledPrism {
    /// cross_section 的 Vector2 分量为 (x,z)，所有长度以米计。
    /// 顺/逆时针均可；内部统一为逆时针，并保留第一个输入顶点。
    /// bevel_size 是沿两条相邻原始边的切点距离，不是圆角半径。
    /// 每角使用与两边相切的圆弧，等角度离散为 ROUND_SEGMENTS 个平面段。
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
            if i < RING_VERTICES {
                let edge = vertices[(i + 1) % RING_VERTICES] - vertices[i];
                Plane {
                    point: vertices[i],
                    normal: Vector3::new(edge.z, 0.0, -edge.x).normalize(),
                }
            } else {
                Plane {
                    point: vertices[if i == RING_VERTICES { 0 } else { RING_VERTICES }],
                    normal: if i == RING_VERTICES {
                        -Vector3::y()
                    } else {
                        Vector3::y()
                    },
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

    /// 只读 indexed mesh：前 27 个顶点在 y_min，后 27 个在 y_max。
    pub fn vertices(&self) -> &[Vector3<f64>; VERTEX_COUNT] {
        &self.vertices
    }

    /// 27 个侧矩形各两片，前后端面各 25 片，共 104 片；对应 vertices()。
    pub fn triangle_indices(&self) -> &[[usize; 3]; TRIANGLE_COUNT] {
        &FACE_INDICES
    }
}

fn mesh_vertices(
    mut section: [Vector2<f64>; 3],
    y_min: f64,
    y_max: f64,
    bevel: f64,
) -> Option<[Vector3<f64>; VERTEX_COUNT]> {
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
    let mut ring = [Vector2::zeros(); RING_VERTICES];
    for i in 0..3 {
        let toward_previous = -edges[(i + 2) % 3];
        let toward_next = edges[i];
        let cosine = toward_previous.dot(&toward_next).clamp(-1.0, 1.0);
        let sine = -toward_previous.perp(&toward_next);
        let denominator = 1.0 + cosine;
        if sine <= 0.0 || denominator <= 0.0 {
            return None;
        }
        // 两侧切点距角点均为 b：圆心在内角平分线上，r=b*tan(theta/2)。
        let center = section[i] + (toward_previous + toward_next) * (bevel / denominator);
        let start = section[i] + toward_previous * bevel;
        let end = section[i] + toward_next * bevel;
        let radial = start - center;
        let sweep = std::f64::consts::PI - sine.atan2(cosine);
        for step in 0..=ROUND_SEGMENTS {
            let angle = sweep * step as f64 / ROUND_SEGMENTS as f64;
            let (sin, cos) = angle.sin_cos();
            // 两个切点直接沿原边计算，避免三角函数舍入改变拼接端点。
            ring[i * ARC_POINTS + step] = if step == 0 {
                start
            } else if step == ROUND_SEGMENTS {
                end
            } else {
                center
                    + Vector2::new(
                        radial.x * cos - radial.y * sin,
                        radial.x * sin + radial.y * cos,
                    )
            };
        }
    }
    // 也排除截取长度低于坐标精度、溢出等造成的重合/反向顶点。
    for i in 0..RING_VERTICES {
        let cross = (ring[(i + 1) % RING_VERTICES] - ring[i])
            .perp(&(ring[(i + 2) % RING_VERTICES] - ring[(i + 1) % RING_VERTICES]));
        if !cross.is_finite() || cross <= 0.0 {
            return None;
        }
    }
    Some(std::array::from_fn(|i| {
        let point = ring[i % RING_VERTICES];
        Vector3::new(
            point.x,
            if i < RING_VERTICES { y_min } else { y_max },
            point.y,
        )
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
        // 该 mesh 为凸体。直接裁剪侧面与两端面的外向半空间，避免共享边重心舍入缝。
        // 平面通过顶点而非世界原点表示，减小平移后的消减误差。
        let mut enter = f64::NEG_INFINITY;
        let mut exit = f64::INFINITY;
        let mut enter_normal = None;
        let mut exit_normal = None;
        for plane in &self.planes {
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
    fn eight_segment_mesh_is_closed_with_54_vertices_and_104_outward_triangles() {
        for bevel in [0.02, 0.035, 0.05] {
            let vertices = mesh_vertices(section(), -1.0, 1.0, bevel).unwrap();
            let prism = BeveledPrism::new(section(), -1.0, 1.0, bevel).unwrap();
            assert_eq!(prism.triangle_indices().len(), 104);
            assert_eq!(vertices.len(), 54);
            assert_eq!(prism.vertices(), &vertices);
            let mut edges = BTreeMap::<_, Vec<_>>::new();
            for [a, b, c] in FACE_INDICES {
                assert!(a != b && b != c && c != a);
                for (u, v) in [(a, b), (b, c), (c, a)] {
                    edges.entry((u.min(v), u.max(v))).or_default().push((u, v));
                }
            }
            assert_eq!(edges.len(), 156);
            assert_eq!(vertices.len() + FACE_INDICES.len(), edges.len() + 2);
            for directions in edges.values() {
                assert_eq!(directions.len(), 2);
                assert_eq!(directions[0], (directions[1].1, directions[1].0));
            }
            for i in 0..RING_VERTICES {
                assert_eq!(
                    vertices[i].x.to_bits(),
                    vertices[i + RING_VERTICES].x.to_bits()
                );
                assert_eq!(
                    vertices[i].z.to_bits(),
                    vertices[i + RING_VERTICES].z.to_bits()
                );
                for j in i + 1..VERTEX_COUNT {
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
                let outgoing = vertices[i * ARC_POINTS + ROUND_SEGMENTS];
                let incoming_next = vertices[((i + 1) % 3) * ARC_POINTS];
                let a = Vector2::new(outgoing.x, outgoing.z);
                let b = Vector2::new(incoming_next.x, incoming_next.z);
                let direction = (triangle[(i + 1) % 3] - triangle[i]).normalize();
                assert!((a - triangle[i] - direction * bevel).norm() < 1e-14);
                assert!((b - triangle[(i + 1) % 3] + direction * bevel).norm() < 1e-14);
            }
        }
    }

    #[test]
    fn each_corner_has_nine_equal_angle_points_on_a_circle_tangent_to_both_original_edges() {
        for triangle in [
            section(),
            [
                Vector2::new(1.2, -0.4),
                Vector2::new(2.4, 0.2),
                Vector2::new(0.8, 1.7),
            ],
        ] {
            for bevel in [0.02, 0.05] {
                let vertices = mesh_vertices(triangle, -1.0, 1.0, bevel).unwrap();
                for i in 0..3 {
                    let previous = (triangle[(i + 2) % 3] - triangle[i]).normalize();
                    let next = (triangle[(i + 1) % 3] - triangle[i]).normalize();
                    let theta = previous.dot(&next).clamp(-1.0, 1.0).acos();
                    let radius = bevel * (theta * 0.5).tan();
                    let bisector = (previous + next).normalize();
                    let center = triangle[i] + bisector * (bevel / (theta * 0.5).cos());
                    let step_angle = (std::f64::consts::PI - theta) / 8.0;
                    for step in 0..=8 {
                        let point = vertices[i * ARC_POINTS + step];
                        let radial = Vector2::new(point.x, point.z) - center;
                        assert!((radial.norm() - radius).abs() < 1e-12);
                        if step == 0 {
                            assert!(radial.dot(&previous).abs() < 1e-12);
                        }
                        if step == 8 {
                            assert!(radial.dot(&next).abs() < 1e-12);
                        }
                        if step < 8 {
                            let following = vertices[i * ARC_POINTS + step + 1];
                            let other = Vector2::new(following.x, following.z) - center;
                            let angle = radial.perp(&other).atan2(radial.dot(&other));
                            assert!((angle - step_angle).abs() < 1e-12);
                        }
                    }
                }
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
            .fold(Vector3::zeros(), |sum, v| sum + v / VERTEX_COUNT as f64);
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
            if index >= 2 * RING_VERTICES {
                assert_eq!(
                    normal,
                    if index < 3 * RING_VERTICES - 2 {
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
                let theta = a.dot(&b).clamp(-1.0, 1.0).acos();
                let radius = bevel * (theta * 0.5).tan();
                let sweep = std::f64::consts::PI - theta;
                let chord_region = radius
                    * radius
                    * 0.5
                    * (ROUND_SEGMENTS as f64 * (sweep / ROUND_SEGMENTS as f64).sin() - sweep.sin());
                bevel * bevel * a.perp(&b).abs() * 0.5 - chord_region
            })
            .sum();
        assert!((volume - (original_area - removed_area) * 2.0).abs() < 1e-12);
    }

    #[test]
    fn rays_hit_all_24_rounding_facets_from_inside_and_outside_with_outward_normals() {
        let vertices = mesh_vertices(section(), -1.0, 1.0, 0.02).unwrap();
        let prism = BeveledPrism::new(section(), -1.0, 1.0, 0.02).unwrap();
        let center = vertices
            .iter()
            .fold(Vector3::zeros(), |sum, v| sum + v / VERTEX_COUNT as f64);
        for i in (0..RING_VERTICES).filter(|i| i % ARC_POINTS < ROUND_SEGMENTS) {
            let j = i + 1;
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
            .fold(Vector3::zeros(), |sum, v| sum + v / VERTEX_COUNT as f64);
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
        for point in prism.vertices()[..RING_VERTICES].iter() {
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

//! 三角截面的三个角各用 8 段相切圆弧替代，再沿 y 拉伸；只圆化三条长棱。

use super::hit::GeometryHit as Hit;
use super::object::Hittable;
use super::ray::INTERSECTION_EPSILON;
use super::ray::PARALLEL_EPSILON;
use super::ray::Ray;
use nalgebra::Vector2;
use nalgebra::Vector3;

/// 每个圆角的等角度圆弧段数，端点也作为共享 mesh 顶点保存。
pub const ROUND_SEGMENTS: usize = 8;
const ARC_POINTS: usize = ROUND_SEGMENTS + 1;
const RING_VERTICES: usize = 3 * ARC_POINTS;
const VERTEX_COUNT: usize = 2 * RING_VERTICES;

#[derive(Debug)]
pub struct BeveledPrism {
    y_min: f64,
    y_max: f64,
    side_planes: [SidePlane; RING_VERTICES],
}

#[derive(Debug, Clone, Copy)]
struct SidePlane {
    px: f64,
    pz: f64,
    nx: f64,
    nz: f64,
}

impl BeveledPrism {
    /// 把三角截面的三个角改为分段圆弧，再沿 Y 拉伸成带圆角的三棱柱。
    ///
    /// 输入：三个 (x,z) 截面顶点、y_min/y_max 及沿原边截取的长度 bevel_size，单位均为米。
    ///
    /// 输出：缓存裁剪平面的 BeveledPrism；退化、非有限或圆角过大时返回 None。
    /// 要求 y_min<y_max 且 0<2·bevel_size<每条边长；bevel_size 不是圆弧半径。
    /// 每个圆角离散成 8 段，法线按平面段计算。
    pub fn new(
        cross_section: [Vector2<f64>; 3],
        y_min: f64,
        y_max: f64,
        bevel_size: f64,
    ) -> Option<Self> {
        let vertices = mesh_vertices(cross_section, y_min, y_max, bevel_size)?;
        let length_y = y_max - y_min;
        let mut side_planes = [SidePlane {
            px: 0.0,
            pz: 0.0,
            nx: 0.0,
            nz: 0.0,
        }; RING_VERTICES];
        for (i, plane) in side_planes.iter_mut().enumerate() {
            let p = vertices[i];
            let next = vertices[(i + 1) % RING_VERTICES];
            let dx = next.x - p.x;
            let dz = next.z - p.z;
            let edge_len = dx.hypot(dz);
            if !edge_len.is_finite() || edge_len == 0.0 || !(edge_len * length_y).is_finite() {
                return None;
            }
            let nx = dz / edge_len;
            let nz = -dx / edge_len;
            if !nx.is_finite() || !nz.is_finite() {
                return None;
            }
            *plane = SidePlane {
                px: p.x,
                pz: p.z,
                nx,
                nz,
            };
        }
        Some(Self {
            y_min,
            y_max,
            side_planes,
        })
    }

    /// 按需重建圆角网格的世界坐标顶点（米）：前 27 点位于 y_min，后 27 点位于 y_max。
    pub fn vertices(&self) -> [Vector3<f64>; VERTEX_COUNT] {
        std::array::from_fn(|i| {
            let plane = self.side_planes[i % RING_VERTICES];
            let y = if i < RING_VERTICES {
                self.y_min
            } else {
                self.y_max
            };
            Vector3::new(plane.px, y, plane.pz)
        })
    }
}

/// 计算圆角截面并复制到两个端面，供 BeveledPrism::new 构建几何。
///
/// 输入：XZ 三角截面、Y 范围和沿边截取长度 b（米）。
///
/// 输出：两圈共 54 个世界坐标顶点；退化、圆角相交或数值无效返回 None。
///
/// 公式：内角为 θ，两条指向邻点的单位向量为 u/v；圆心 C=P+b(u+v)/(1+cosθ)，半径 r=b·tan(θ/2)。
/// 从一个切点沿圆弧旋转 π-θ，再以等角度生成 8 段。
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
        self.intersect_after(ray, INTERSECTION_EPSILON)
    }

    fn intersect_from_volume(&self, ray: &Ray) -> Option<Hit> {
        self.intersect_after(ray, 0.0)
    }
}

impl BeveledPrism {
    /// 用凸体的各个外向平面裁剪光线，避免三角面共享边的舍入缝隙。
    ///
    /// 输入：Ray 和最小距离下限；输出：最近正向 Hit，从内部发射时取出射点；无交集或数值无效返回 None。
    ///
    /// 公式：每面要求 (o+t·d-p)·n≤0，其边界距离 t=-(o-p)·n/(d·n)。
    /// 所有面的允许区间取交集 `[enter,exit]`，法线来自决定最终端点的平面。
    fn intersect_after(&self, ray: &Ray, min_distance: f64) -> Option<Hit> {
        if ray
            .origin
            .iter()
            .chain(ray.direction.iter())
            .any(|v| !v.is_finite())
        {
            return None;
        }
        // 该 mesh 为凸体。先裁剪 Y-slab 再裁剪 27 条 2D 侧面半空间，避免共享边重心舍入缝。
        // 平面通过顶点而非世界原点表示，减小平移后的消减误差。
        let mut enter = f64::NEG_INFINITY;
        let mut exit = f64::INFINITY;
        let mut enter_normal = None;
        let mut exit_normal = None;
        let dy = ray.direction.y;
        let oy_min = ray.origin.y - self.y_min;
        let oy_max = ray.origin.y - self.y_max;
        if !oy_min.is_finite() || !oy_max.is_finite() {
            return None;
        }
        if dy.abs() <= PARALLEL_EPSILON {
            let roundoff_min = 32.0 * f64::EPSILON * oy_min.abs().max(1.0);
            let roundoff_max = 32.0 * f64::EPSILON * oy_max.abs().max(1.0);
            if -oy_min > roundoff_min || oy_max > roundoff_max {
                return None;
            }
        } else {
            let t_min = -oy_min / dy;
            let t_max = -oy_max / dy;
            if !t_min.is_finite() || !t_max.is_finite() {
                return None;
            }
            if dy > 0.0 {
                enter = t_min;
                enter_normal = Some(-Vector3::y());
                exit = t_max;
                exit_normal = Some(Vector3::y());
            } else {
                enter = t_max;
                enter_normal = Some(Vector3::y());
                exit = t_min;
                exit_normal = Some(-Vector3::y());
            }
            if exit <= min_distance {
                return None;
            }
        }
        let ox0 = ray.origin.x;
        let oz0 = ray.origin.z;
        let dx = ray.direction.x;
        let dz = ray.direction.z;
        for plane in &self.side_planes {
            let ox = ox0 - plane.px;
            let oz = oz0 - plane.pz;
            let signed_distance = ox * plane.nx + oz * plane.nz;
            let facing = dx * plane.nx + dz * plane.nz;
            if !signed_distance.is_finite() || !facing.is_finite() {
                return None;
            }
            if facing.abs() <= PARALLEL_EPSILON {
                let roundoff = 32.0 * f64::EPSILON * ox.abs().max(oz.abs()).max(1.0);
                if signed_distance > roundoff {
                    return None;
                }
                continue;
            }
            let distance = -signed_distance / facing;
            if !distance.is_finite() {
                return None;
            }
            if facing < 0.0 {
                if distance > enter {
                    enter = distance;
                    enter_normal = Some(Vector3::new(plane.nx, 0.0, plane.nz));
                }
            } else if distance < exit {
                exit = distance;
                exit_normal = Some(Vector3::new(plane.nx, 0.0, plane.nz));
                if exit <= min_distance {
                    return None;
                }
            }
            if enter.is_finite() && exit.is_finite() {
                let roundoff = 32.0 * f64::EPSILON * enter.abs().max(exit.abs()).max(1.0);
                if enter > exit + roundoff {
                    return None;
                }
            }
        }
        // 仅容许浮点精度量级的区间差，不使用米制自相交 epsilon 扩大实体。
        let roundoff = 32.0 * f64::EPSILON * enter.abs().max(exit.abs()).max(1.0);
        if enter > exit + roundoff {
            return None;
        }
        let (distance, normal) = if enter > min_distance {
            (enter, enter_normal?)
        } else if exit > min_distance {
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const TRIANGLE_COUNT: usize = 4 * RING_VERTICES - 4;
    const FACE_INDICES: [[usize; 3]; TRIANGLE_COUNT] = face_indices();

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
            assert_eq!(FACE_INDICES.len(), 104);
            assert_eq!(vertices.len(), 54);
            assert_eq!(prism.vertices(), vertices);
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

    #[test]
    fn intersect_from_volume_accepts_sub_epsilon_positive_hits() {
        let prism = BeveledPrism::new(section(), -1.0, 1.0, 0.02).unwrap();
        let near = INTERSECTION_EPSILON * 0.25;
        let outside = Ray::new(Vector3::new(0.0, -1.0 - near, 0.3), Vector3::y()).unwrap();
        assert!((prism.intersect(&outside).unwrap().distance - (2.0 + near)).abs() < 1e-12);
        let hit = prism.intersect_from_volume(&outside).unwrap();
        assert!((hit.distance - near).abs() < 1e-14);
        assert_eq!(hit.normal, -Vector3::y());

        let inside = Ray::new(Vector3::new(0.0, 1.0 - near, 0.3), Vector3::y()).unwrap();
        assert!(prism.intersect(&inside).is_none());
        let exit = prism.intersect_from_volume(&inside).unwrap();
        assert!((exit.distance - near).abs() < 1e-14);
        assert_eq!(exit.normal, Vector3::y());
    }
}

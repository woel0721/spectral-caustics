//! XY 三角形沿世界 Z 轴挤出的无倒角封闭三棱柱。

use super::hit::GeometryHit as Hit;
use super::object::{ClosedVolume, Hittable};
use super::prism::Prism;
use super::ray::Ray;
use nalgebra::Vector2;
use nalgebra::Vector3;

#[derive(Debug)]
pub struct VerticalPrism {
    mesh: Prism,
}

impl VerticalPrism {
    /// 将 XY 三角截面沿 Z 轴拉伸成封闭三棱柱，用于竖直棱镜场景。
    ///
    /// 输入：三个 XY 顶点 section（米）及上下高度 z_min<z_max（米），接受顺/逆时针。
    ///
    /// 输出：端面朝外的 VerticalPrism；截面退化、高度无效或构建失败返回 None。
    /// 内部调整绕序后调用 Prism::from_vertices，共用三角面求交。
    pub fn new(mut section: [Vector2<f64>; 3], z_min: f64, z_max: f64) -> Option<Self> {
        let height = z_max - z_min;
        if section.iter().flatten().any(|v| !v.is_finite())
            || !z_min.is_finite()
            || !z_max.is_finite()
            || !height.is_finite()
            || height <= 0.0
        {
            return None;
        }
        let area = (section[1] - section[0]).perp(&(section[2] - section[0]));
        if !area.is_finite() || area == 0.0 {
            return None;
        }
        // 首端面朝 -Z，末端面朝 +Z，与通用凸三棱柱的面索引一致。
        if area > 0.0 {
            section.swap(1, 2);
        }
        let vertices = std::array::from_fn(|i| {
            let p = section[i % 3];
            Vector3::new(p.x, p.y, if i < 3 { z_min } else { z_max })
        });
        let mesh = Prism::from_vertices(vertices)?;
        Some(Self { mesh })
    }
}

impl Hittable for VerticalPrism {
    fn closed_volume(&self) -> Option<&dyn ClosedVolume> {
        Some(&self.mesh)
    }

    /// 把输入 Ray 交给内部封闭三棱柱求交，返回最近正向 Hit；未命中返回 None。
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        self.mesh.intersect(ray)
    }

    fn intersect_from_volume(&self, ray: &Ray) -> Option<Hit> {
        self.mesh.intersect_from_volume(ray)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section() -> [Vector2<f64>; 3] {
        [
            Vector2::new(-1.0, -0.5),
            Vector2::new(1.0, -0.5),
            Vector2::new(0.0, 1.0),
        ]
    }

    #[test]
    fn z_extrusion_has_triangular_caps_and_vertical_outward_sides() {
        let prism = VerticalPrism::new(section(), 0.1, 2.1).unwrap();
        let center = Vector3::new(0.0, 0.0, 1.1);
        for (point, normal) in [
            (Vector3::new(0.0, 0.0, 2.1), Vector3::z()),
            (Vector3::new(0.0, 0.0, 0.1), -Vector3::z()),
        ] {
            let hit = prism
                .intersect(&Ray::new(center, point - center).unwrap())
                .unwrap();
            assert!((hit.coords - point).norm() < 1e-12);
            assert_eq!(hit.normal, normal);
        }
        let s = section();
        for i in 0..3 {
            let a = s[i];
            let b = s[(i + 1) % 3];
            let mid = (a + b) * 0.5;
            let point = Vector3::new(mid.x, mid.y, 1.1);
            let normal = Vector3::new(b.y - a.y, a.x - b.x, 0.0).normalize();
            let hit = prism
                .intersect(&Ray::new(center, point - center).unwrap())
                .unwrap();
            assert!((hit.coords - point).norm() < 1e-12);
            assert!((hit.normal - normal).norm() < 1e-12);
            assert_eq!(hit.normal.z, 0.0);
        }
        // 在 XY 包围盒内但位于三角形外，不能误生成矩形顶盖。
        assert!(
            prism
                .intersect(&Ray::new(Vector3::new(0.9, 0.9, 3.0), -Vector3::z()).unwrap())
                .is_none()
        );
    }

    #[test]
    fn reversed_section_and_shared_edges_remain_closed() {
        let mut reversed = section();
        reversed.swap(1, 2);
        let a = VerticalPrism::new(section(), 0.1, 2.1).unwrap();
        let b = VerticalPrism::new(reversed, 0.1, 2.1).unwrap();
        for p in section()
            .into_iter()
            .chain([Vector2::zeros(), Vector2::new(0.5, 0.25)])
        {
            let ray = Ray::new(Vector3::new(p.x, p.y, 3.0), -Vector3::z()).unwrap();
            let x = a.intersect(&ray).unwrap();
            let y = b.intersect(&ray).unwrap();
            assert!((x.coords - Vector3::new(p.x, p.y, 2.1)).norm() < 1e-12);
            assert_eq!(x.coords, y.coords);
            assert_eq!(x.normal, y.normal);
        }
        let ray = Ray::new(Vector3::new(0.0, 0.0, 2.1), -Vector3::z()).unwrap();
        assert!((a.intersect(&ray).unwrap().coords.z - 0.1).abs() < 1e-12);
    }

    #[test]
    fn invalid_sections_and_z_ranges_are_rejected() {
        for (lo, hi) in [
            (0.0, 0.0),
            (2.0, 1.0),
            (f64::NAN, 2.0),
            (0.0, f64::INFINITY),
            (-f64::MAX, f64::MAX),
        ] {
            assert!(VerticalPrism::new(section(), lo, hi).is_none());
        }
        assert!(VerticalPrism::new([Vector2::zeros(); 3], 0.0, 1.0).is_none());
        assert!(
            VerticalPrism::new(
                [
                    Vector2::zeros(),
                    Vector2::new(1.0, 1.0),
                    Vector2::new(2.0, 2.0)
                ],
                0.0,
                1.0
            )
            .is_none()
        );
        let mut bad = section();
        bad[0].x = f64::NAN;
        assert!(VerticalPrism::new(bad, 0.0, 1.0).is_none());
        let bad = [
            Vector2::new(-f64::MAX, 0.0),
            Vector2::new(f64::MAX, 0.0),
            Vector2::new(0.0, 1.0),
        ];
        assert!(VerticalPrism::new(bad, 0.0, 1.0).is_none());
    }
}

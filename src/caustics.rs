//! 固定网格发射平行光，将地面落点累积为线性 XYZ 灰度图。
//! 支持 Day 2 的不透明遮挡和 Day 3 的固定折射率双界面折射，尚不计算光谱。

use crate::{
    framebuffer::Framebuffer,
    ray::Ray,
    scene::{Scene, take_photon},
    utils::bilinear_weights,
};
use nalgebra::Vector3;
use std::error::Error;

// 第一张验证图使用固定的 4×4 米接收区域与同样大小的发射区域。
const GROUND_MIN: f64 = -2.0;
const GROUND_MAX: f64 = 2.0;
const LIGHT_HEIGHT: f64 = 3.0;
const UNBLOCKED_BRIGHTNESS: f64 = 0.5;

/// 将地面世界坐标映射为连续像素坐标；世界 x、y 采用 [-2, 2) 米的半开区间。
/// 世界 +x 朝图片右侧，+y 朝图片上方；整数输出坐标是像素中心，边界在 -0.5。
/// 保留小数位置供双线性分配使用；z 只检查是否有限。
pub fn ground_to_pixel(position: Vector3<f64>, width: usize, height: usize) -> Option<(f64, f64)> {
    if width == 0
        || height == 0
        || !position.iter().all(|v| v.is_finite())
        || !(GROUND_MIN..GROUND_MAX).contains(&position.x)
        || !(GROUND_MIN..GROUND_MAX).contains(&position.y)
    {
        return None;
    }

    let span = GROUND_MAX - GROUND_MIN;
    let u = (position.x - GROUND_MIN) / span;
    let v = (position.y - GROUND_MIN) / span;
    let x = u * width as f64 - 0.5;
    let y = (1.0 - v) * height as f64 - 0.5;
    Some((x, y))
}

/// Day 2 的直接命中灰度图；未先命中地面的光线被遮挡，不继续追踪。
pub fn render_ground_hits(
    scene: &Scene,
    ground_id: usize,
    samples_per_pixel_axis: usize,
) -> Result<Framebuffer, Box<dyn Error>> {
    render_landing_points(scene, ground_id, samples_per_pixel_axis, |ray| {
        let hit = scene.intersect(&ray)?;
        (hit.object_id == Some(ground_id)).then_some(hit.coords)
    })
}

/// Day 3：每条光线调用 take_photon，累计直接落地或通过玻璃两次折射后的落点。
pub fn render_refracted_ground_hits(
    scene: &Scene,
    prism_id: usize,
    ground_id: usize,
    samples_per_pixel_axis: usize,
) -> Result<Framebuffer, Box<dyn Error>> {
    if prism_id == ground_id || prism_id >= scene.objects.len() {
        return Err("三棱镜编号无效或与地面编号相同".into());
    }
    render_landing_points(scene, ground_id, samples_per_pixel_axis, |ray| {
        take_photon(scene, ray, prism_id, ground_id)
    })
}

/// 两种光路复用相同的发射网格、通量权重与像素映射；landing_point 决定光路。
/// 每个发射小格按每轴 samples_per_pixel_axis 份细分，各取中心点，无随机数。
fn render_landing_points(
    scene: &Scene,
    ground_id: usize,
    samples_per_pixel_axis: usize,
    landing_point: impl Fn(Ray) -> Option<Vector3<f64>>,
) -> Result<Framebuffer, Box<dyn Error>> {
    if scene.width == 0 || scene.height == 0 || samples_per_pixel_axis == 0 {
        return Err("图像尺寸和每轴采样数必须大于零".into());
    }
    if ground_id >= scene.objects.len() {
        return Err("地面编号不在场景中".into());
    }

    let pixel_count = scene
        .width
        .checked_mul(scene.height)
        .ok_or("像素数量溢出")?;
    let samples_x = scene
        .width
        .checked_mul(samples_per_pixel_axis)
        .ok_or("横向采样数溢出")?;
    let samples_y = scene
        .height
        .checked_mul(samples_per_pixel_axis)
        .ok_or("纵向采样数溢出")?;
    let ray_count = samples_x.checked_mul(samples_y).ok_or("光线数量溢出")?;

    let span = GROUND_MAX - GROUND_MIN;
    let area = span * span;
    // 固定总通量，再除以光线数与像素对应的地面面积，得到每条光线的亮度贡献。
    // 这样增加采样数只增加落点密度，不会直接让图片变亮。
    let total_flux = UNBLOCKED_BRIGHTNESS * area;
    let flux_per_ray = total_flux / ray_count as f64;
    let pixel_area = area / pixel_count as f64;
    // 沿用 render.rs 中的 D65 参考白，仅生成已知灰色，不代表已实现光谱积分。
    let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
    let contribution = white_xyz * (flux_per_ray / pixel_area);
    let mut framebuffer = Framebuffer::new(scene.width, scene.height);

    for sample_y in 0..samples_y {
        for sample_x in 0..samples_x {
            let origin_x = GROUND_MIN + (sample_x as f64 + 0.5) / samples_x as f64 * span;
            let origin_y = GROUND_MAX - (sample_y as f64 + 0.5) / samples_y as f64 * span;
            let ray = Ray::new(
                Vector3::new(origin_x, origin_y, LIGHT_HEIGHT),
                Vector3::new(0.0, 0.0, -1.0),
                0.0,
            )
            .ok_or("平行光线构造失败")?;

            let Some(position) = landing_point(ray) else {
                continue;
            };
            if let Some((x, y)) = ground_to_pixel(position, scene.width, scene.height) {
                let neighbors = bilinear_weights(x, y, scene.width, scene.height)
                    .ok_or("双线性权重计算失败")?;
                // 同一条光子的 XYZ 分成四份，权重和为 1；在 sRGB 编码之前累积。
                for (pixel_x, pixel_y, weight) in neighbors {
                    if weight > 0.0
                        && !framebuffer.accumulate(pixel_x, pixel_y, contribution * weight)
                    {
                        return Err("地面像素累积失败".into());
                    }
                }
            }
        }
    }
    Ok(framebuffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ground::Ground, prism::Prism, render::save_png};

    #[test]
    fn world_coordinates_map_to_non_square_image_with_y_flipped() {
        for (position, expected) in [
            (Vector3::new(-2.0, -2.0, 0.0), (-0.5, 1.5)),
            (Vector3::new(-1.5, 1.5, 0.0), (-0.125, -0.25)),
            (Vector3::new(1.5, -1.5, 0.0), (2.125, 1.25)),
            (Vector3::new(1.99, 1.99, 0.0), (2.4925, -0.495)),
        ] {
            let actual = ground_to_pixel(position, 3, 2).unwrap();
            assert!((actual.0 - expected.0).abs() < 1e-12);
            assert!((actual.1 - expected.1).abs() < 1e-12);
        }
        assert_eq!(
            ground_to_pixel(Vector3::new(-1.5, 1.0, 0.0), 4, 2),
            Some((0.0, 0.0))
        );
        assert_eq!(
            ground_to_pixel(Vector3::new(1.5, -1.0, 0.0), 4, 2),
            Some((3.0, 1.0))
        );
    }

    #[test]
    fn mapping_rejects_outside_non_finite_and_empty_inputs() {
        for value in [-2.01, 2.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(ground_to_pixel(Vector3::new(value, 0.0, 0.0), 3, 2), None);
            assert_eq!(ground_to_pixel(Vector3::new(0.0, value, 0.0), 3, 2), None);
        }
        assert_eq!(
            ground_to_pixel(Vector3::new(0.0, 0.0, f64::NAN), 3, 2),
            None
        );
        assert_eq!(ground_to_pixel(Vector3::zeros(), 0, 2), None);
        assert_eq!(ground_to_pixel(Vector3::zeros(), 3, 0), None);
    }

    #[test]
    fn unblocked_ground_is_uniform_and_more_samples_preserve_brightness() {
        let mut scene = Scene::new(8, 4);
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let coarse = render_ground_hits(&scene, ground_id, 1).unwrap();
        let fine = render_ground_hits(&scene, ground_id, 3).unwrap();
        for (a, b) in coarse.pixels.iter().zip(&fine.pixels) {
            assert!((a.y - 0.5).abs() < 1e-12);
            assert!((a - b).norm() < 1e-12);
        }
    }

    #[test]
    fn prism_shadow_has_bilinear_edges_and_preserves_flux_in_either_object_order() {
        for ground_first in [false, true] {
            let mut scene = Scene::new(16, 8);
            if !ground_first {
                scene.add(Box::new(Prism::new().unwrap()));
            }
            let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
            if ground_first {
                scene.add(Box::new(Prism::new().unwrap()));
            }
            let framebuffer = render_ground_hits(&scene, ground_id, 2).unwrap();
            // 双线性分配让投影边缘的两个像素共享能量；深处仍黑，远处仍是 0.5。
            for (x, y, expected) in [
                (7, 3, 0.0),
                (0, 0, 0.5),
                (6, 3, 0.0625),
                (5, 3, 0.4375),
                (6, 2, 0.1171875),
            ] {
                assert!((framebuffer.get(x, y).unwrap().y - expected).abs() < 1e-12);
            }
            let total_y: f64 = framebuffer.pixels.iter().map(|pixel| pixel.y).sum();
            // 128 个像素中，几何投影遮挡等效的 16 个像素；过滤不改变总量。
            assert!((total_y - 56.0).abs() < 1e-12);
        }
    }

    #[test]
    fn bilinear_landing_distributes_expected_xyz_and_preserves_total_energy() {
        let mut scene = Scene::new(4, 3);
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        // 此世界位置映射到连续像素 (1.25, 0.75)，让所有样本都落在这里。
        let position = Vector3::new(-0.25, 1.0 / 3.0, 0.0);
        let framebuffer = render_landing_points(&scene, ground_id, 1, |_| Some(position)).unwrap();
        for (x, y, expected_y) in [(1, 0, 1.125), (2, 0, 0.375), (1, 1, 3.375), (2, 1, 1.125)] {
            assert!((framebuffer.get(x, y).unwrap().y - expected_y).abs() < 1e-12);
        }
        let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
        let total = framebuffer
            .pixels
            .iter()
            .fold(Vector3::zeros(), |sum, pixel| sum + pixel);
        assert!((total - white_xyz * 6.0).norm() < 1e-12);
    }

    #[test]
    fn bilinear_landing_keeps_all_energy_at_receiver_edges_and_for_one_pixel_images() {
        for (width, height) in [(4, 3), (1, 4), (4, 1), (1, 1)] {
            let mut scene = Scene::new(width, height);
            let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
            let framebuffer = render_landing_points(&scene, ground_id, 2, |_| {
                Some(Vector3::new(-2.0, -2.0, 0.0))
            })
            .unwrap();
            let total_y: f64 = framebuffer.pixels.iter().map(|pixel| pixel.y).sum();
            let expected = 0.5 * (width * height) as f64;
            assert!((total_y - expected).abs() < 1e-12);
            assert!((framebuffer.get(0, height - 1).unwrap().y - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn invalid_render_settings_return_errors() {
        let mut scene = Scene::new(8, 4);
        assert!(render_ground_hits(&scene, 0, 1).is_err());
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        assert!(render_ground_hits(&scene, ground_id, 0).is_err());
        assert!(render_ground_hits(&scene, ground_id, usize::MAX).is_err());
        assert!(render_refracted_ground_hits(&scene, usize::MAX, ground_id, 1).is_err());
        assert!(render_refracted_ground_hits(&scene, ground_id, ground_id, 1).is_err());
        scene.width = 0;
        assert!(render_ground_hits(&scene, ground_id, 1).is_err());
    }

    #[test]
    fn ground_hit_png_contains_gray_ground_and_black_shadow() -> Result<(), Box<dyn Error>> {
        let mut scene = Scene::new(8, 4);
        scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let framebuffer = render_ground_hits(&scene, ground_id, 1)?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("ground-hit-png-{}-{timestamp}", std::process::id()));
        let path = directory.join("ground_hits.png");
        save_png(&framebuffer, path.to_str().ok_or("临时路径必须是 UTF-8")?)?;
        let decoded = image::open(&path)?;
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        let pixels = decoded.into_rgb8();
        assert_eq!(pixels.dimensions(), (8, 4));
        for y in 0..4 {
            for x in 0..8 {
                let expected = if (3..5).contains(&x) && (1..3).contains(&y) {
                    [0, 0, 0]
                } else {
                    [188, 188, 188]
                };
                assert_eq!(pixels.get_pixel(x, y).0, expected);
            }
        }
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn refracted_render_redistributes_energy_without_creating_flux() {
        let mut scene = Scene::new(64, 32);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let refracted = render_refracted_ground_hits(&scene, prism_id, ground_id, 2).unwrap();
        // 远离三棱镜的角落仍是相对亮度 0.5，部分落点与直射光叠加后更亮。
        assert!((refracted.get(0, 0).unwrap().y - 0.5).abs() < 1e-12);
        assert!(refracted.pixels.iter().any(|pixel| pixel.y > 0.5));
        assert!(refracted.pixels.iter().any(|pixel| pixel.y == 0.0));
        assert!(
            refracted
                .pixels
                .iter()
                .all(|pixel| pixel.iter().all(|v| v.is_finite() && *v >= 0.0))
        );
        let total_y: f64 = refracted.pixels.iter().map(|pixel| pixel.y).sum();
        let emitted_y = 0.5 * (scene.width * scene.height) as f64;
        assert!(total_y > 0.0 && total_y <= emitted_y + 1e-12);
        let coarse = render_refracted_ground_hits(&scene, prism_id, ground_id, 1).unwrap();
        let coarse_y: f64 = coarse.pixels.iter().map(|pixel| pixel.y).sum();
        // 边界采样可能改变少量全反射分类，但四倍光线数不能使总能量接近四倍。
        assert!((total_y - coarse_y).abs() < emitted_y * 0.02);
    }

    #[test]
    fn refracted_png_preserves_grayscale_and_contains_brighter_landing_regions()
    -> Result<(), Box<dyn Error>> {
        let mut scene = Scene::new(64, 32);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let framebuffer = render_refracted_ground_hits(&scene, prism_id, ground_id, 2)?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("refracted-png-{}-{timestamp}", std::process::id()));
        let path = directory.join("refracted_hits.png");
        save_png(&framebuffer, path.to_str().ok_or("临时路径必须是 UTF-8")?)?;
        let decoded = image::open(&path)?;
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        let pixels = decoded.into_rgb8();
        assert_eq!(pixels.dimensions(), (64, 32));
        assert_eq!(pixels.get_pixel(0, 0).0, [188, 188, 188]);
        assert!(
            pixels
                .pixels()
                .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2])
        );
        assert!(pixels.pixels().any(|pixel| pixel[0] > 188));
        assert!(pixels.pixels().any(|pixel| pixel[0] == 0));
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}

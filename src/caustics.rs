//! Day 2 地面命中图：规则网格发射平行光，三棱镜暂时作为不透明遮挡物。
//! 只累计地面交点，不计算玻璃折射或光谱；Framebuffer 仍只保存输出像素。

use crate::{framebuffer::Framebuffer, ray::Ray, scene::Scene};
use nalgebra::Vector3;
use std::error::Error;

// 第一张验证图使用固定的 4×4 米接收区域与同样大小的发射区域。
const GROUND_MIN: f64 = -2.0;
const GROUND_MAX: f64 = 2.0;
const LIGHT_HEIGHT: f64 = 3.0;
const UNBLOCKED_BRIGHTNESS: f64 = 0.5;

/// 将地面世界坐标映射到输出图片；x、y 均采用 [-2, 2) 米的半开区间。
/// 世界 +x 朝图片右侧，+y 朝图片上方；z 只检查是否有限。
pub fn ground_to_pixel(
    position: Vector3<f64>,
    width: usize,
    height: usize,
) -> Option<(usize, usize)> {
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
    // 输入已在区域内；min 仅防止贴近上边界时浮点舍入到 width/height。
    let x = ((u * width as f64).floor() as usize).min(width - 1);
    let y_from_bottom = ((v * height as f64).floor() as usize).min(height - 1);
    Some((x, height - 1 - y_from_bottom))
}

/// 生成固定俯视接收区域的灰度图；ground_id 是 Scene::add 返回的地面编号。
/// 每个像素对应的发射区域按每轴 samples_per_pixel_axis 份均匀细分。
/// 使用各小格中心，无随机数；相同参数得到相同结果。
pub fn render_ground_hits(
    scene: &Scene,
    ground_id: usize,
    samples_per_pixel_axis: usize,
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

            let Some(hit) = scene.intersect(&ray) else {
                continue;
            };
            // 最近交点不是地面，表示被三棱镜等物体遮挡；不再继续传播。
            if hit.object_id != Some(ground_id) {
                continue;
            }
            if let Some((x, y)) = ground_to_pixel(hit.coords, scene.width, scene.height)
                && !framebuffer.accumulate(x, y, contribution)
            {
                return Err("地面像素累积失败".into());
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
            (Vector3::new(-2.0, -2.0, 0.0), (0, 1)),
            (Vector3::new(-1.5, 1.5, 0.0), (0, 0)),
            (Vector3::new(1.5, -1.5, 0.0), (2, 1)),
            (Vector3::new(1.99, 1.99, 0.0), (2, 0)),
        ] {
            assert_eq!(ground_to_pixel(position, 3, 2), Some(expected));
        }
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
    fn prism_shadow_matches_footprint_regardless_of_object_order() {
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
            for y in 0..8 {
                for x in 0..16 {
                    // 固定三棱镜的俯视投影为 x∈[-0.5,0.5]、y∈[-1,1]。
                    let expected = if (6..10).contains(&x) && (2..6).contains(&y) {
                        0.0
                    } else {
                        0.5
                    };
                    assert!((framebuffer.get(x, y).unwrap().y - expected).abs() < 1e-12);
                }
            }
        }
    }

    #[test]
    fn invalid_render_settings_return_errors() {
        let mut scene = Scene::new(8, 4);
        assert!(render_ground_hits(&scene, 0, 1).is_err());
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        assert!(render_ground_hits(&scene, ground_id, 0).is_err());
        assert!(render_ground_hits(&scene, ground_id, usize::MAX).is_err());
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
}

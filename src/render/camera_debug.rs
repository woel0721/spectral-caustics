//! 相机法线调试图与对应的像素遍历。

use super::utils::normal_to_rgb8;
use crate::scene::Scene;
use image::{Rgb, RgbImage};
use std::error::Error;

/// 从相机逐像素查询最近交点，将世界空间几何法线映射到 RGB8。
/// 保留几何法线的原始朝向，不朝相机翻转；未命中使用深色背景。
/// 此图不应用光谱积分、曝光或 sRGB 传递函数。
pub fn render_camera_debug(
    scene: &Scene,
    width: usize,
    height: usize,
) -> Result<RgbImage, Box<dyn Error>> {
    if width == 0 || height == 0 {
        return Err("相机调试图的宽高必须大于零".into());
    }
    let image_width = u32::try_from(width)?;
    let image_height = u32::try_from(height)?;
    // RGB8 每像素 3 字节，先拒绝字节数溢出或超过 Vec 容量上限的尺寸。
    width
        .checked_mul(height)
        .and_then(|count| count.checked_mul(3))
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or("相机调试图的像素缓冲大小溢出")?;
    let mut image = RgbImage::new(image_width, image_height);

    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let ray = scene
            .camera
            .ray_for_pixel(x as usize, y as usize, width, height)
            .ok_or("无法生成摄像机光线")?;
        *pixel = match scene.intersect(&ray) {
            Some(hit) => Rgb(normal_to_rgb8(hit.normal).ok_or("命中法线无效，无法可视化")?),
            None => Rgb([25, 30, 45]),
        };
    }

    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_debug_visualizes_world_normals_and_preserves_background() {
        let mut config = crate::config::sceneconfig::SceneConfig::default();
        config.camera.position = [0.0, -2.0, 1.2];
        config.camera.target = [0.0, 0.0, 0.4];
        config.camera.fov_y_degrees = 90.0;
        let scene = Scene::from_config(&config).unwrap();
        let image = render_camera_debug(&scene, 5, 3).unwrap();
        assert_eq!(image.dimensions(), (5, 3));
        assert_eq!(*image.get_pixel(2, 0), Rgb([25, 30, 45]));
        assert_eq!(*image.get_pixel(2, 1), Rgb([128, 0, 128])); // 三棱镜前端面 -y。
        assert_eq!(*image.get_pixel(2, 2), Rgb([128, 128, 255])); // 地面 +z。

        // 添加顺序改变后物体编号也改变，显示结果应保持一致。
        config.objects.reverse();
        let mut reordered = Scene::from_config(&config).unwrap();
        assert_eq!(render_camera_debug(&reordered, 5, 3).unwrap(), image);
        // 所有命中都显示法线，不依赖物体类型编号。
        reordered.prism_id = None;
        let unknown = render_camera_debug(&reordered, 5, 3).unwrap();
        assert_eq!(unknown, image);

        // 从地面上下两侧观察，世界空间法线始终为 +z，不朝相机翻转。
        config.objects.retain(|object| {
            matches!(
                object,
                crate::config::sceneconfig::ObjectConfig::Ground { .. }
            )
        });
        config.camera.target = [0.0; 3];
        config.camera.up = [0.0, 1.0, 0.0];
        for height in [-1.0, 1.0] {
            config.camera.position = [0.0, 0.0, height];
            let scene = Scene::from_config(&config).unwrap();
            let image = render_camera_debug(&scene, 1, 1).unwrap();
            assert_eq!(*image.get_pixel(0, 0), Rgb([128, 128, 255]));
        }
    }

    #[test]
    fn camera_debug_rejects_zero_or_overflowing_dimensions() {
        let scene = Scene::new();
        for (width, height) in [
            (0, 1),
            (1, 0),
            (usize::MAX, 1),
            (1, usize::MAX),
            (u32::MAX as usize, u32::MAX as usize),
        ] {
            assert!(render_camera_debug(&scene, width, height).is_err());
        }
    }
}

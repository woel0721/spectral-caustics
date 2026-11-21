//! Display transform and PNG encoding; input remains linear XYZ.
use crate::{
    color::{linear_rgb_to_srgb8, xyz_to_linear_srgb},
    film::Framebuffer,
};
use image::{ImageFormat, Rgb, RgbImage};
use nalgebra::Vector3;
use std::{error::Error, fs, path::Path};
/// 把一个曝光后的线性 RGB 通道压到显示范围，防止 HDR 高光直接截成白块。
///
/// 输入：通道 x；输出：简化 ACES 值，正常计算时位于 `[0,1]`，负输入按 0 处理。
/// 极大输入可能使中间乘法溢出而得到 NaN，调用方须检查结果。
///
/// 公式：y=clamp[x(2.51x+0.03)/(x(2.43x+0.59)+0.14),0,1]。
pub fn aces_filmic(x: f64) -> f64 {
    let x = x.max(0.0);
    let result = (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
    result.clamp(0.0, 1.0)
}

/// 对输入的曝光后线性 RGB 逐通道应用 aces_filmic，返回 `[0,1]` 显示范围颜色，供后续 sRGB 编码。
pub fn tone_map(rgb: Vector3<f64>) -> Vector3<f64> {
    rgb.map(aces_filmic)
}

pub fn save_png_with_exposure(
    framebuffer: &Framebuffer,
    path: impl AsRef<Path>,
    exposure: f64,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if !exposure.is_finite() || exposure < 0.0 {
        return Err("曝光必须有限且非负".into());
    }
    // image 使用 u32 表示宽高；无法转换时返回错误。
    let width = u32::try_from(framebuffer.width())?;
    let height = u32::try_from(framebuffer.height())?;

    let mut image = RgbImage::new(width, height);

    // Framebuffer 的构造保证像素切片包含 height 个完整的行。
    let xyz_rows = framebuffer.pixels().chunks_exact(framebuffer.width());
    for (xyz_row, rgb_row) in xyz_rows.zip(image.rows_mut()) {
        for (&xyz, pixel) in xyz_row.iter().zip(rgb_row) {
            let linear_rgb = xyz_to_linear_srgb(xyz).ok_or("XYZ 非法或颜色计算溢出")? * exposure;
            if !linear_rgb.iter().all(|v| v.is_finite()) {
                return Err("曝光计算溢出".into());
            }

            let mapped_rgb = tone_map(linear_rgb);
            if !mapped_rgb.iter().all(|v| v.is_finite()) {
                return Err("ACES 色调映射计算溢出".into());
            }

            let rgb8 = linear_rgb_to_srgb8(mapped_rgb).ok_or("sRGB 编码失败")?;
            *pixel = Rgb(rgb8);
        }
    }

    // 保存 output/test.png 时，自动创建 output 目录。
    let path = path.as_ref();
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }

    image.save_with_format(path, ImageFormat::Png)?;
    Ok(())
}

/// The output policy is shared by RGB and hero rendering; auxiliary paths never overwrite the primary.
pub fn save_components(
    images: &crate::integrator::camera::CameraLightingImages,
    primary: &Path,
    exposure: f64,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    save_png_with_exposure(&images.combined, primary, exposure)?;
    let directory = primary.parent().unwrap_or(Path::new("."));
    for (image, name) in [
        (&images.direct, "camera_direct.png"),
        (&images.caustics, "camera_caustics.png"),
        (&images.combined, "camera_combined.png"),
    ] {
        let path = directory.join(name);
        if path == primary && name == "camera_combined.png" {
            continue;
        }
        let path = if path == primary {
            directory.join(format!("aux_{name}"))
        } else {
            path
        };
        save_png_with_exposure(image, path, exposure)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn aces_maps_reference_values_and_preserves_highlight_differences_above_one() {
        for (input, expected) in [
            (-1.0, 0.0),
            (0.0, 0.0),
            (0.18, 0.2668989203894968),
            (0.5, 0.6163069544364508),
            (1.0, 0.8037974683544302),
            (2.0, 0.914855072463768),
            (4.0, 0.9734171097148379),
            (8.0, 1.0),
        ] {
            assert!((aces_filmic(input) - expected).abs() < 1e-12);
        }
        assert!(aces_filmic(1.0) < aces_filmic(2.0));
        assert!(aces_filmic(2.0) < aces_filmic(4.0));
        let rgb = Vector3::new(-0.2, 0.18, 2.0);
        let mapped = tone_map(rgb);
        assert!((mapped - Vector3::new(0.0, 0.2668989203894968, 0.914855072463768)).norm() < 1e-12);
        assert_eq!(rgb, Vector3::new(-0.2, 0.18, 2.0));
    }

    use super::*;
    use nalgebra::Vector3;

    #[test]
    fn png_roundtrip_preserves_dimensions_positions_and_aces_then_srgb_encoding()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        // 每次测试使用独立目录；路径中的时间戳只用于避免文件冲突，不影响颜色结果。
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "spectral-caustics-png-{}-{timestamp}",
            std::process::id()
        ));
        fs::create_dir(&directory)?;
        let path = directory.join("nested/roundtrip.png");

        // 宽高不同，且每个位置的颜色不同，可以检查坐标顺序和图像翻转。
        let mut framebuffer = Framebuffer::new(4, 2).unwrap();
        let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
        // 三原色 XYZ 来自上述 W3C 文档的 lin_sRGB_to_XYZ。
        let cases = [
            (
                0,
                0,
                Vector3::new(506752.0 / 1228815.0, 87098.0 / 409605.0, 7918.0 / 409605.0),
                [232, 0, 0],
            ),
            (
                1,
                0,
                Vector3::new(87881.0 / 245763.0, 175762.0 / 245763.0, 87881.0 / 737289.0),
                [0, 232, 0],
            ),
            (
                2,
                0,
                Vector3::new(12673.0 / 70218.0, 12673.0 / 175545.0, 1001167.0 / 1053270.0),
                [0, 0, 232],
            ),
            (0, 1, white_xyz, [232, 232, 232]),
            // 0.5 经 ACES 后约为 0.6163，再编码为 sRGB8 的 206。
            (1, 1, white_xyz * 0.5, [206, 206, 206]),
            (2, 1, Vector3::zeros(), [0, 0, 0]),
            // 大于 1 的高光经过曲线后仍有区别，不能预先裁剪为白色。
            (3, 0, white_xyz * 2.0, [245, 245, 245]),
            (3, 1, white_xyz * 4.0, [252, 252, 252]),
        ];

        for &(x, y, xyz, _) in &cases {
            assert!(framebuffer.set(x, y, xyz));
        }

        save_png_with_exposure(&framebuffer, &path, 1.0)?;
        let decoded = image::open(&path)?;
        // 先核对原始像素格式，避免格式转换掩盖错误的位深或通道数量。
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        let pixels = decoded.into_rgb8();
        assert_eq!(pixels.dimensions(), (4, 2));
        for (x, y, _, expected) in cases {
            assert_eq!(
                pixels.get_pixel(x as u32, y as u32).0,
                expected,
                "PNG 像素 ({x}, {y}) 的颜色不正确"
            );
        }

        fs::remove_dir_all(directory)?;
        Ok(())
    }
}

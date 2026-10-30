//! Z-up 经纬度环境贴图；U 水平周期采样，V 在图片上下边界夹紧。

use image::{DynamicImage, ImageDecoder, codecs::hdr::HdrDecoder};
use nalgebra::Vector3;
use std::{
    error::Error,
    f64::consts::{PI, TAU},
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

/// 按行保存相对线性 RGB 辐亮度，第一行为图片顶部（世界 +Z）。
/// 像素在加载时验证，之后只读；不进行曝光、色调映射或 sRGB 转换。
#[derive(Debug)]
pub struct EnvironmentMap {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 3]>,
}

impl EnvironmentMap {
    /// 只解码 Radiance RGBE HDR，不通过 8-bit 或普通图片转换路径。
    /// 使用 image 支持的标准 -Y +X 排列（从上到下、从左到右）；其他排列返回错误。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        Self::decode(BufReader::new(File::open(path)?))
    }

    /// direction 指向所查询的世界方向，内部先归一化；零向量和非有限方向返回 None。
    /// +Z/-Z 对应顶部/底部；+X、+Y、-Y 分别对应 U=0.5、0.75、0.25。
    /// -X 为 U=0/1 的接缝，极点统一取 U=0.5（经度任意）；只访问已缓存的像素。
    pub fn radiance(&self, direction: Vector3<f64>) -> Option<Vector3<f64>> {
        let (u, v) = direction_to_uv(direction)?;
        Some(self.sample_uv(u, v))
    }

    fn decode(reader: impl Read) -> Result<Self, Box<dyn Error>> {
        let decoder = HdrDecoder::new(reader)?;
        let (width, height) = decoder.dimensions();
        let (width, height) = (usize::try_from(width)?, usize::try_from(height)?);
        // 在解码分配像素缓冲之前拒绝零尺寸和地址空间溢出。
        pixel_count(width, height)?;
        // HdrDecoder 输出 Rgb32F，into_rgb32f 直接取出浮点缓冲，不施加传递函数。
        let image = DynamicImage::from_decoder(decoder)?.into_rgb32f();
        let pixels = image.pixels().map(|pixel| pixel.0).collect();
        Ok(Self::from_pixels(width, height, pixels)?)
    }

    fn from_pixels(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> Result<Self, String> {
        if pixels.len() != pixel_count(width, height)? {
            return Err("环境贴图像素数量与宽高不匹配".into());
        }
        if pixels
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err("环境贴图辐亮度必须有限且非负".into());
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// 输入有限 UV；整数像素坐标为像素中心，顶部 V=0，不做图片 y 翻转。
    fn sample_uv(&self, u: f64, v: f64) -> Vector3<f64> {
        let x = u.rem_euclid(1.0) * self.width as f64 - 0.5;
        let y = (v * self.height as f64 - 0.5).clamp(0.0, (self.height - 1) as f64);
        let x_floor = x.floor();
        // x_floor 最小为 -1；先向下取整再做整数 wrap，保留接缝两侧的插值权重。
        let x0 = (x_floor as isize).rem_euclid(self.width as isize) as usize;
        let x1 = (x0 + 1) % self.width;
        let y0 = y.floor() as usize;
        let y1 = (y0 + 1).min(self.height - 1);
        let dx = x - x_floor;
        let dy = y - y0 as f64;

        let mut rgb = Vector3::zeros();
        for (px, py, weight) in [
            (x0, y0, (1.0 - dx) * (1.0 - dy)),
            (x1, y0, dx * (1.0 - dy)),
            (x0, y1, (1.0 - dx) * dy),
            (x1, y1, dx * dy),
        ] {
            // 在 f64 中插值，保留大于 1 的亮度，也避免 f32 加法溢出。
            rgb += Vector3::from(self.pixels[py * self.width + px]).cast::<f64>() * weight;
        }
        rgb
    }
}

fn pixel_count(width: usize, height: usize) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("环境贴图宽高必须大于零".into());
    }
    let count = width.checked_mul(height).ok_or("环境贴图像素数量溢出")?;
    count
        .checked_mul(size_of::<[f32; 3]>())
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or("环境贴图像素缓冲大小溢出")?;
    Ok(count)
}

fn direction_to_uv(direction: Vector3<f64>) -> Option<(f64, f64)> {
    if !direction.iter().all(|value| value.is_finite()) {
        return None;
    }
    let scale = direction.amax();
    if scale == 0.0 {
        return None;
    }
    // 先缩放，避免有限非零向量的 norm 在平方求和时上溢或下溢。
    let direction = direction / scale;
    let direction = direction / direction.norm();
    // 极点没有唯一经度，固定 U=0.5，避免 x/y 的有符号零改变查询列。
    let phi = if direction.x == 0.0 && direction.y == 0.0 {
        0.0
    } else {
        direction.y.atan2(direction.x)
    };
    let theta = direction.z.clamp(-1.0, 1.0).acos();
    Some((phi / TAU + 0.5, theta / PI))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, codecs::hdr::HdrEncoder};
    use std::{
        fs,
        io::{Cursor, Write},
        time::{SystemTime, UNIX_EPOCH},
    };

    const PIXELS: [[f32; 3]; 4] = [
        [2.0, 4.0, 8.0],
        [6.0, 8.0, 12.0],
        [10.0, 12.0, 16.0],
        [14.0, 16.0, 20.0],
    ];

    fn test_map() -> EnvironmentMap {
        EnvironmentMap::from_pixels(2, 2, PIXELS.to_vec()).unwrap()
    }

    fn assert_rgb(actual: Vector3<f64>, expected: [f64; 3]) {
        assert!(
            (actual - Vector3::from(expected)).norm() < 1e-12,
            "actual={actual:?}, expected={expected:?}"
        );
    }

    fn direction_at(u: f64, v: f64) -> Vector3<f64> {
        let phi = (u - 0.5) * TAU;
        let theta = v * PI;
        Vector3::new(
            theta.sin() * phi.cos(),
            theta.sin() * phi.sin(),
            theta.cos(),
        )
    }

    // 手写 RGBE 字节，独立检查通道、行列顺序和线性 HDR 数值；不依赖编码器往返。
    fn synthetic_hdr() -> Vec<u8> {
        let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 2\n".to_vec();
        for exponent in 130..=133 {
            bytes.extend_from_slice(&[128, 64, 32, exponent]);
        }
        bytes
    }

    #[test]
    fn direction_uv_matches_z_up_axes_and_image_orientation() {
        for (direction, expected) in [
            (Vector3::z(), (0.5, 0.0)),
            (-Vector3::z(), (0.5, 1.0)),
            (Vector3::x(), (0.5, 0.5)),
            (Vector3::y(), (0.75, 0.5)),
            (-Vector3::y(), (0.25, 0.5)),
            (Vector3::new(-1.0, 0.0, 0.0), (1.0, 0.5)),
            (Vector3::new(-1.0, -0.0, 0.0), (0.0, 0.5)),
        ] {
            let (u, v) = direction_to_uv(direction).unwrap();
            assert!((u - expected.0).abs() < 1e-12);
            assert!((v - expected.1).abs() < 1e-12);
        }
    }

    #[test]
    fn direction_is_normalized_even_at_extreme_finite_scales() {
        let map = test_map();
        let direction = Vector3::new(1.0, -2.0, 3.0);
        let expected_uv = direction_to_uv(direction).unwrap();
        let expected_rgb = map.radiance(direction).unwrap();
        for scale in [0.25, 8.0, 1e300, 1e-300] {
            let (u, v) = direction_to_uv(direction * scale).unwrap();
            assert!((u - expected_uv.0).abs() < 1e-12);
            assert!((v - expected_uv.1).abs() < 1e-12);
            assert!((map.radiance(direction * scale).unwrap() - expected_rgb).norm() < 1e-12);
        }
        for scale in [f64::MAX, f64::from_bits(1)] {
            assert_eq!(direction_to_uv(Vector3::z() * scale), Some((0.5, 0.0)));
        }
    }

    #[test]
    fn zero_and_nonfinite_directions_return_none() {
        let map = test_map();
        assert!(direction_to_uv(Vector3::zeros()).is_none());
        assert!(map.radiance(Vector3::zeros()).is_none());
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for axis in 0..3 {
                let mut direction = Vector3::repeat(1.0);
                direction[axis] = value;
                assert!(direction_to_uv(direction).is_none());
                assert!(map.radiance(direction).is_none());
            }
        }
    }

    #[test]
    fn pixel_centers_recover_the_four_texels() {
        let map = test_map();
        for (u, v, pixel) in [
            (0.25, 0.25, PIXELS[0]),
            (0.75, 0.25, PIXELS[1]),
            (0.25, 0.75, PIXELS[2]),
            (0.75, 0.75, PIXELS[3]),
        ] {
            assert_rgb(
                map.radiance(direction_at(u, v)).unwrap(),
                pixel.map(f64::from),
            );
        }
    }

    #[test]
    fn bilinear_interpolation_matches_known_two_by_two_values() {
        let map = test_map();
        assert_rgb(map.radiance(Vector3::x()).unwrap(), [8.0, 10.0, 14.0]);
        // x=0.25, y=0.75，四个权重依次为 3/16、1/16、9/16、3/16。
        assert_rgb(
            map.radiance(direction_at(0.375, 0.625)).unwrap(),
            [9.0, 11.0, 15.0],
        );
    }

    #[test]
    fn horizontal_seam_wraps_last_and_first_columns_continuously() {
        let map = EnvironmentMap::from_pixels(
            4,
            1,
            vec![[2.0, 4.0, 8.0], [32.0; 3], [64.0; 3], [10.0, 12.0, 16.0]],
        )
        .unwrap();
        for u in [-1.0, 0.0, 1.0, 2.0] {
            assert_rgb(map.sample_uv(u, 0.5), [6.0, 8.0, 12.0]);
        }
        for u in [-0.01, 0.01, 0.99] {
            assert!((map.sample_uv(u, 0.5) - map.sample_uv(u + 1.0, 0.5)).norm() < 1e-12);
        }
        let seam = map.radiance(Vector3::new(-1.0, 0.0, 0.0)).unwrap();
        assert_rgb(seam, [6.0, 8.0, 12.0]);
        assert_eq!(seam, map.radiance(Vector3::new(-1.0, -0.0, 0.0)).unwrap());
        for y in [-1e-9, 1e-9] {
            let neighbor = map.radiance(Vector3::new(-1.0, y, 0.0)).unwrap();
            assert!((neighbor - seam).norm() < 1e-7);
        }
    }

    #[test]
    fn vertical_boundaries_clamp_without_wrapping_to_the_other_pole() {
        let map = test_map();
        for v in [-1.0, 0.0, 0.1, 0.25] {
            assert_rgb(map.sample_uv(0.25, v), [2.0, 4.0, 8.0]);
        }
        for v in [0.75, 0.9, 1.0, 2.0] {
            assert_rgb(map.sample_uv(0.25, v), [10.0, 12.0, 16.0]);
        }
        assert_rgb(map.radiance(Vector3::z()).unwrap(), [4.0, 6.0, 10.0]);
        assert_rgb(map.radiance(-Vector3::z()).unwrap(), [12.0, 14.0, 18.0]);
    }

    #[test]
    fn single_column_interpolates_only_vertically() {
        let map = EnvironmentMap::from_pixels(1, 2, vec![PIXELS[0], PIXELS[2]]).unwrap();
        for u in [-1.0, 0.0, 0.25, 0.5, 1.0, 2.0] {
            assert_rgb(map.sample_uv(u, 0.0), [2.0, 4.0, 8.0]);
            assert_rgb(map.sample_uv(u, 0.5), [6.0, 8.0, 12.0]);
            assert_rgb(map.sample_uv(u, 1.0), [10.0, 12.0, 16.0]);
        }
    }

    #[test]
    fn single_row_interpolates_only_horizontally() {
        let map = EnvironmentMap::from_pixels(2, 1, vec![PIXELS[0], PIXELS[1]]).unwrap();
        for v in [-1.0, 0.0, 0.25, 0.5, 1.0, 2.0] {
            assert_rgb(map.sample_uv(0.25, v), [2.0, 4.0, 8.0]);
            assert_rgb(map.sample_uv(0.5, v), [4.0, 6.0, 10.0]);
            assert_rgb(map.sample_uv(0.75, v), [6.0, 8.0, 12.0]);
        }
    }

    #[test]
    fn single_pixel_is_constant_and_preserves_linear_hdr_values() {
        let map = EnvironmentMap::from_pixels(1, 1, vec![[0.25, 2.0, 16.0]]).unwrap();
        for direction in [
            Vector3::x(),
            -Vector3::x(),
            Vector3::y(),
            -Vector3::y(),
            Vector3::z(),
            -Vector3::z(),
            Vector3::new(1.0, 2.0, 3.0),
        ] {
            assert_rgb(map.radiance(direction).unwrap(), [0.25, 2.0, 16.0]);
        }
    }

    #[test]
    fn interpolation_of_maximum_f32_radiance_stays_finite_and_unclipped() {
        let map = EnvironmentMap::from_pixels(2, 2, vec![[f32::MAX; 3]; 4]).unwrap();
        for direction in [Vector3::x(), Vector3::new(1.0, 2.0, 3.0)] {
            let rgb = map.radiance(direction).unwrap();
            for value in rgb.iter() {
                assert!(value.is_finite());
                assert!((value / f64::from(f32::MAX) - 1.0).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn invalid_pixels_are_rejected_in_every_channel() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01] {
            for channel in 0..3 {
                let mut pixel = [1.0; 3];
                pixel[channel] = value;
                assert!(EnvironmentMap::from_pixels(2, 1, vec![[1.0; 3], pixel]).is_err());
            }
        }
        assert!(EnvironmentMap::from_pixels(1, 1, vec![[0.0, 2.0, f32::MAX]]).is_ok());
    }

    #[test]
    fn invalid_dimensions_and_pixel_counts_are_rejected() {
        for (width, height) in [
            (0, 0),
            (0, 1),
            (1, 0),
            (usize::MAX, 2),
            (isize::MAX as usize / size_of::<[f32; 3]>() + 1, 1),
        ] {
            assert!(EnvironmentMap::from_pixels(width, height, vec![]).is_err());
        }
        assert!(EnvironmentMap::from_pixels(2, 2, vec![[1.0; 3]; 3]).is_err());
        assert!(EnvironmentMap::from_pixels(2, 2, vec![[1.0; 3]; 5]).is_err());
    }

    #[test]
    fn hdr_decoder_preserves_linear_values_and_top_down_pixel_order() {
        let map = EnvironmentMap::decode(Cursor::new(synthetic_hdr())).unwrap();
        assert_eq!((map.width, map.height), (2, 2));
        assert_eq!(
            map.pixels,
            [
                [2.0, 1.0, 0.5],
                [4.0, 2.0, 1.0],
                [8.0, 4.0, 2.0],
                [16.0, 8.0, 4.0]
            ]
        );
        assert_rgb(map.radiance(Vector3::z()).unwrap(), [3.0, 1.5, 0.75]);
        assert_rgb(map.radiance(-Vector3::z()).unwrap(), [12.0, 6.0, 3.0]);
    }

    #[test]
    fn rle_hdr_decodes_from_memory_without_clipping_or_gamma_conversion() {
        let pixels: Vec<_> = [[0.25, 2.0, 16.0], [4.0, 8.0, 32.0]]
            .into_iter()
            .flat_map(|pixel| [Rgb(pixel); 8])
            .collect();
        let mut bytes = Vec::new();
        HdrEncoder::new(&mut bytes).encode(&pixels, 8, 2).unwrap();
        let map = EnvironmentMap::decode(Cursor::new(bytes)).unwrap();
        assert_eq!(
            map.pixels,
            pixels.iter().map(|pixel| pixel.0).collect::<Vec<_>>()
        );
        assert_rgb(map.radiance(Vector3::z()).unwrap(), [0.25, 2.0, 16.0]);
        assert_rgb(map.radiance(-Vector3::z()).unwrap(), [4.0, 8.0, 32.0]);
    }

    #[test]
    fn malformed_truncated_and_unsupported_hdr_inputs_return_errors() {
        assert!(EnvironmentMap::decode(Cursor::new(b"not a Radiance HDR image")).is_err());
        let mut truncated = synthetic_hdr();
        truncated.pop();
        assert!(EnvironmentMap::decode(Cursor::new(truncated)).is_err());
        for dimensions in [
            "-Y 0 +X 1",
            "-Y 1 +X 0",
            "-Y 4294967295 +X 4294967295",
            "+Y 2 +X 2",
            "-Y 2 -X 2",
        ] {
            let bytes = format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n{dimensions}\n");
            assert!(EnvironmentMap::decode(Cursor::new(bytes)).is_err());
        }
    }

    #[test]
    fn load_caches_pixels_and_does_not_read_the_file_during_queries() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "environment-map-{}-{stamp}.hdr",
            std::process::id()
        ));
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(&synthetic_hdr()).unwrap();
        drop(file);
        let result = EnvironmentMap::load(&path);
        fs::remove_file(&path).unwrap();
        let map = result.unwrap();
        assert!(EnvironmentMap::load(&path).is_err());
        assert_rgb(map.radiance(Vector3::z()).unwrap(), [3.0, 1.5, 0.75]);
        assert_rgb(map.radiance(Vector3::x()).unwrap(), [7.5, 3.75, 1.875]);
    }
}

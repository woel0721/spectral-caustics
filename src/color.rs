//! 线性 XYZ / sRGB 颜色变换及 display sRGB 编码。

use nalgebra::Matrix3;
use nalgebra::Vector3;

/// 把线性 sRGB 转为 D65 白点的 CIE XYZ，供相机结果存入统一缓冲。
///
/// 输入：有限 RGB 向量；输出：XYZ，非有限或运算溢出返回 None；参考白 Y=1，保留范围外值。
///
/// 公式：XYZ=M·RGB；M≈[[0.412391,0.357584,0.180481],[0.212639,0.715169,0.072192],[0.019331,0.119195,0.950532]]。
/// 只转换颜色空间，不做 sRGB 解码、曝光、裁剪或光谱重建；实现使用下方精确分数。
/// 矩阵来源：<https://www.w3.org/TR/css-color-4/#color-conversion-code>
pub fn linear_srgb_to_xyz(linear_rgb: Vector3<f64>) -> Option<Vector3<f64>> {
    if !linear_rgb.iter().all(|value| value.is_finite()) {
        return None;
    }
    let matrix = Matrix3::new(
        506752.0 / 1228815.0,
        87881.0 / 245763.0,
        12673.0 / 70218.0,
        87098.0 / 409605.0,
        175762.0 / 245763.0,
        12673.0 / 175545.0,
        7918.0 / 409605.0,
        87881.0 / 737289.0,
        1001167.0 / 1053270.0,
    );
    let xyz = matrix * linear_rgb;
    xyz.iter().all(|value| value.is_finite()).then_some(xyz)
}

/// 把线性 XYZ 转为 D65 白点的线性 sRGB，供着色或 PNG 输出使用。
///
/// 输入：有限 XYZ 向量；输出：RGB，非法或运算溢出返回 None；保留负值和大于 1 的结果。
///
/// 公式：RGB=M⁻¹·XYZ；M⁻¹≈[[3.240970,-1.537383,-0.498611],[-0.969244,1.875968,0.041555],[0.055630,-0.203977,1.056972]]。
/// 参考白 Y=1，曝光与显示变换由保存入口处理；实现使用下方精确分数。
/// 矩阵来源：<https://www.w3.org/TR/css-color-4/#color-conversion-code>
pub fn xyz_to_linear_srgb(xyz: Vector3<f64>) -> Option<Vector3<f64>> {
    // 防止非法分量进入矩阵运算并传播到输出颜色。
    if !xyz.iter().all(|v| v.is_finite()) {
        return None;
    }
    //CIE XYZ 转线性 sRGB 的矩阵，参考 W3C CSS Color 4。
    // Matrix3::new 的九个参数按行填写；三行分别计算 R、G、B。
    // 第一行表示 R = (12831 / 3959) * X - (329 / 214) * Y - (1974 / 3959) * Z。
    // 保留来源文档的分数形式，带 .0 的数值使除法按浮点数进行。
    let matrix = Matrix3::new(
        12831.0 / 3959.0,
        -329.0 / 214.0,
        -1974.0 / 3959.0,
        -851781.0 / 878810.0,
        1648619.0 / 878810.0,
        36519.0 / 878810.0,
        705.0 / 12673.0,
        -2585.0 / 12673.0,
        705.0 / 667.0,
    );
    // 形状：(3×3) * (3×1) = (3×1)，得到线性颜色 (R, G, B)。
    let linear_rgb = matrix * xyz;

    // 即使输入有限，特别大的输入乘以矩阵系数仍可能溢出成无穷大，因此再检查结果。
    // 有限的负分量与大于 1 的分量仍返回给调用者，供后续显示范围处理使用。
    if linear_rgb.iter().all(|v| v.is_finite()) {
        Some(linear_rgb)
    } else {
        None
    }
}

/// 把线性亮度编码为显示器使用的 sRGB 通道，供 PNG 量化使用。
///
/// 输入：有限 c∈`[0,1]`；输出：编码值 s，越界或非有限输入返回 None；例如 c=0.5 时 s≈0.735357。
///
/// 公式：c≤0.0031308 时 s=12.92c，否则 s=1.055c^(1/2.4)-0.055。
/// 调用方先做曝光和色调映射；来源：<https://registry.color.org/rgb-registry/srgb>
pub fn linear_to_srgb_channel(linear: f64) -> Option<f64> {
    // 检查输入，避免将非有限值或未经显示范围处理的颜色直接编码。
    if !linear.is_finite() || !(0.0..=1.0).contains(&linear) {
        return None;
    }

    // 接近黑色时使用线性段，较亮时使用幂函数段；分界点属于线性段。
    // powf 表示浮点指数运算，这里的指数为 1 / 2.4。
    let encoded = if linear <= 0.0031308 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };

    Some(encoded)
}

/// 把显示范围内的线性 RGB 转成 PNG 可写的三个字节。
///
/// 输入：三个有限 `[0,1]` 通道；输出：`[R,G,B]`，任一通道非法时 None。
///
/// 公式：先逐通道 s=sRGB(c)，再 byte=round(255s)；曝光/ACES 在调用前完成。
pub fn linear_rgb_to_srgb8(linear_rgb: Vector3<f64>) -> Option<[u8; 3]> {
    let r = linear_to_srgb_channel(linear_rgb.x)?;
    let g = linear_to_srgb_channel(linear_rgb.y)?;
    let b = linear_to_srgb_channel(linear_rgb.z)?;

    // 将 [0, 1] 范围的浮点值映射到 [0, 255] 的整数范围。
    // 四舍五入后转换为 u8，确保结果在 0..=255 范围内。
    Some([
        (r * 255.0).round().clamp(0.0, 255.0) as u8,
        (g * 255.0).round().clamp(0.0, 255.0) as u8,
        (b * 255.0).round().clamp(0.0, 255.0) as u8,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caustics::map::bilinear_weights;

    // 浮点矩阵计算有舍入误差，使用向量差的长度（欧几里得范数）衡量接近程度。
    // 例如 0.9999999999999993 与 1.0 的差远小于这里使用的 1e-12 容差。
    fn assert_rgb_near(actual: Vector3<f64>, expected: Vector3<f64>) {
        assert!(
            (actual - expected).norm() < 1e-12,
            "实际颜色 {actual:?}，预期颜色 {expected:?}"
        );
    }

    #[test]
    fn black_stays_black() {
        // 没有颜色贡献的像素经过转换后仍应为黑色。
        assert_eq!(xyz_to_linear_srgb(Vector3::zeros()), Some(Vector3::zeros()));
        assert_eq!(linear_srgb_to_xyz(Vector3::zeros()), Some(Vector3::zeros()));
    }

    #[test]
    fn linear_srgb_white_and_primaries_become_d65_xyz() {
        let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
        assert_rgb_near(linear_srgb_to_xyz(Vector3::repeat(1.0)).unwrap(), white_xyz);
        for (rgb, expected) in [
            (
                Vector3::x(),
                Vector3::new(0.4123907992659595, 0.2126390058715104, 0.0193308187155919),
            ),
            (
                Vector3::y(),
                Vector3::new(0.357584339383878, 0.715168678767756, 0.119194779794626),
            ),
            (
                Vector3::z(),
                Vector3::new(0.1804807884018343, 0.0721923153607337, 0.9505321522496606),
            ),
        ] {
            assert_rgb_near(linear_srgb_to_xyz(rgb).unwrap(), expected);
        }
    }

    #[test]
    fn linear_srgb_conversion_preserves_linear_values_without_clamping_or_gamma() {
        for rgb in [Vector3::repeat(0.5), Vector3::new(-0.2, 1.5, 0.4)] {
            let xyz = linear_srgb_to_xyz(rgb).unwrap();
            assert_rgb_near(xyz_to_linear_srgb(xyz).unwrap(), rgb);
        }
        let white = linear_srgb_to_xyz(Vector3::repeat(1.0)).unwrap();
        assert_rgb_near(
            linear_srgb_to_xyz(Vector3::repeat(0.5)).unwrap(),
            white * 0.5,
        );
    }

    #[test]
    fn linear_srgb_conversion_rejects_non_finite_values_and_overflow() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for channel in 0..3 {
                let mut rgb = Vector3::repeat(0.5);
                rgb[channel] = value;
                assert_eq!(linear_srgb_to_xyz(rgb), None);
            }
        }
        assert_eq!(linear_srgb_to_xyz(Vector3::repeat(f64::MAX)), None);
    }

    #[test]
    fn d65_white_becomes_equal_rgb_channels() {
        // 检查白点与转换矩阵使用相同约定，转换后白色的三个 RGB 分量均接近 1。
        let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);

        let rgb = xyz_to_linear_srgb(white_xyz).unwrap();

        assert_rgb_near(rgb, Vector3::new(1.0, 1.0, 1.0));
    }

    #[test]
    fn srgb_primaries_have_correct_channels() {
        // XYZ 值来自同一 W3C 文档的 lin_sRGB_to_XYZ，检查通道顺序与矩阵方向。
        let cases = [
            (
                Vector3::new(506752.0 / 1228815.0, 87098.0 / 409605.0, 7918.0 / 409605.0),
                Vector3::new(1.0, 0.0, 0.0),
            ),
            (
                Vector3::new(87881.0 / 245763.0, 175762.0 / 245763.0, 87881.0 / 737289.0),
                Vector3::new(0.0, 1.0, 0.0),
            ),
            (
                Vector3::new(12673.0 / 70218.0, 12673.0 / 175545.0, 1001167.0 / 1053270.0),
                Vector3::new(0.0, 0.0, 1.0),
            ),
        ];

        for (xyz, expected) in cases {
            assert_rgb_near(xyz_to_linear_srgb(xyz).unwrap(), expected);
        }
    }

    #[test]
    fn out_of_display_range_values_are_preserved() {
        // 这组测试 XYZ 会得到负分量和大于 1 的分量，确认转换阶段保留它们。
        let rgb = xyz_to_linear_srgb(Vector3::new(0.1, 0.8, 0.1)).unwrap();

        assert!(rgb.x < 0.0);
        assert!(rgb.y > 1.0);
        assert!(rgb.z < 0.0);
    }

    #[test]
    fn non_finite_xyz_is_rejected() {
        // 任意一个颜色分量非法时，整个转换都应返回 None。
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for channel in 0..3 {
                let mut xyz = Vector3::new(0.2, 0.3, 0.4);
                xyz[channel] = value;
                assert_eq!(xyz_to_linear_srgb(xyz), None);
            }
        }
    }

    #[test]
    fn overflow_is_rejected() {
        // f64::MAX 本身有限，但乘以大于 1 的系数可能溢出。
        assert_eq!(xyz_to_linear_srgb(Vector3::new(f64::MAX, 0.0, 0.0)), None);
    }

    #[test]
    fn srgb_encoding_preserves_black_and_white() {
        assert_eq!(linear_to_srgb_channel(0.0), Some(0.0));
        assert!((linear_to_srgb_channel(1.0).unwrap() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn srgb_encoding_matches_known_brightness_values() {
        // 固定参考值检查非线性编码，避免误用直接返回输入或简单乘法。
        for (linear, expected) in [(0.18, 0.46135612950044164), (0.5, 0.7353569830524495)] {
            let encoded = linear_to_srgb_channel(linear).unwrap();
            assert!((encoded - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn srgb_encoding_handles_both_sides_of_breakpoint() {
        // 分界点两侧使用不同公式。标准中的常数经舍入，因此此处逐点核对参考值。
        let cases = [
            (0.0031307, 0.040448644),
            (0.0031308, 0.040449936),
            (0.0031309, 0.04045117777859802),
        ];
        for (linear, expected) in cases {
            let encoded = linear_to_srgb_channel(linear).unwrap();
            assert!((encoded - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn srgb_encoding_rejects_invalid_input() {
        // 这里只负责编码，超出 [0, 1] 的值应留给后续显示范围处理。
        for linear in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -f64::EPSILON,
            1.0 + f64::EPSILON,
            f64::MAX,
        ] {
            assert_eq!(linear_to_srgb_channel(linear), None);
        }
    }

    #[test]
    fn fractional_position_has_expected_four_weights() {
        assert_eq!(
            bilinear_weights(1.25, 0.75, 4, 3),
            Some([
                (1, 0, 0.1875),
                (2, 0, 0.0625),
                (1, 1, 0.5625),
                (2, 1, 0.1875)
            ])
        );
    }

    #[test]
    fn pixel_center_contributes_only_to_that_pixel() {
        let samples = bilinear_weights(1.0, 1.0, 4, 3).unwrap();
        assert_eq!(samples[0], (1, 1, 1.0));
        assert!(samples[1..].iter().all(|&(_, _, weight)| weight == 0.0));
    }

    #[test]
    fn edge_and_single_pixel_weights_stay_in_bounds_and_sum_to_one() {
        for (width, height) in [(4, 3), (1, 4), (4, 1), (1, 1)] {
            for x in [-0.5, -0.25, 0.0, (width - 1) as f64, width as f64 - 0.5] {
                for y in [-0.5, -0.25, 0.0, (height - 1) as f64, height as f64 - 0.5] {
                    let samples = bilinear_weights(x, y, width, height).unwrap();
                    assert!(
                        samples
                            .iter()
                            .all(|&(x, y, w)| x < width && y < height && w.is_finite() && w >= 0.0)
                    );
                    let total: f64 = samples.iter().map(|&(_, _, weight)| weight).sum();
                    assert!((total - 1.0).abs() < 1e-12);
                }
            }
        }
        let samples = bilinear_weights(-0.25, 1.25, 4, 3).unwrap();
        assert!(
            samples
                .iter()
                .filter(|&&(_, _, w)| w > 0.0)
                .all(|&(x, _, _)| x == 0)
        );
    }

    #[test]
    fn invalid_coordinates_and_empty_dimensions_are_rejected() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5001, 3.5001] {
            assert!(bilinear_weights(value, 0.0, 4, 4).is_none());
            assert!(bilinear_weights(0.0, value, 4, 4).is_none());
        }
        assert!(bilinear_weights(0.0, 0.0, 0, 4).is_none());
        assert!(bilinear_weights(0.0, 0.0, 4, 0).is_none());
    }
}

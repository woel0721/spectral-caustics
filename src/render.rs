use nalgebra::{Matrix3, Vector3};

/// 将 D65 白点约定下的 CIE XYZ 转为线性 sRGB，参考白的 Y = 1。
/// 保留超出 [0, 1] 的结果；sRGB 编码和显示范围处理在后续输出阶段完成。
/// 输入非有限值或计算溢出时返回 None。
///
/// 矩阵来源：W3C CSS Color 4 的 XYZ_to_lin_sRGB：
/// https://www.w3.org/TR/css-color-4/#color-conversion-code
pub fn xyz_to_linear_srgb(xyz: Vector3<f64>) -> Option<Vector3<f64>> {
    if !xyz.iter().all(|v| v.is_finite()) {
        return None;
    }

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
    let linear_rgb = matrix * xyz;

    if linear_rgb.iter().all(|v| v.is_finite()) {
        Some(linear_rgb)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rgb_near(actual: Vector3<f64>, expected: Vector3<f64>) {
        assert!(
            (actual - expected).norm() < 1e-12,
            "实际颜色 {actual:?}，预期颜色 {expected:?}"
        );
    }

    #[test]
    fn black_stays_black() {
        assert_eq!(xyz_to_linear_srgb(Vector3::zeros()), Some(Vector3::zeros()));
    }

    #[test]
    fn d65_white_becomes_equal_rgb_channels() {
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
        let rgb = xyz_to_linear_srgb(Vector3::new(0.1, 0.8, 0.1)).unwrap();

        assert!(rgb.x < 0.0);
        assert!(rgb.y > 1.0);
        assert!(rgb.z < 0.0);
    }

    #[test]
    fn non_finite_xyz_is_rejected() {
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
        assert_eq!(xyz_to_linear_srgb(Vector3::new(f64::MAX, 0.0, 0.0)), None);
    }
}

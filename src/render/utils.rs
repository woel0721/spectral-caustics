//! 双线性插值的四邻域权重；光子落点使用同样的权重向四个像素分配能量。

/// 输入连续像素坐标，返回 (像素 x, 像素 y, 权重)，顺序为左上、右上、左下、右下。
/// 整数坐标表示像素中心；图片覆盖 [-0.5, width-0.5] × [-0.5, height-0.5]。
/// 边缘半个像素内夹紧到最近的像素中心，保证四个权重之和仍为 1。
pub fn bilinear_weights(
    x: f64,
    y: f64,
    width: usize,
    height: usize,
) -> Option<[(usize, usize, f64); 4]> {
    if width == 0
        || height == 0
        || !x.is_finite()
        || !y.is_finite()
        || x < -0.5
        || x > width as f64 - 0.5
        || y < -0.5
        || y > height as f64 - 0.5
    {
        return None;
    }

    // 区域内的边缘落点仍应保留全部能量，不向图片之外分配权重。
    let x = x.clamp(0.0, (width - 1) as f64);
    let y = y.clamp(0.0, (height - 1) as f64);
    let x0 = (x.floor() as usize).min(width - 1);
    let y0 = (y.floor() as usize).min(height - 1);
    let x1 = x0.saturating_add(1).min(width - 1);
    let y1 = y0.saturating_add(1).min(height - 1);
    let dx = (x - x0 as f64).clamp(0.0, 1.0);
    let dy = (y - y0 as f64).clamp(0.0, 1.0);

    // 先在 x 方向分成 (1-dx)、dx，再在 y 方向分成 (1-dy)、dy。
    Some([
        (x0, y0, (1.0 - dx) * (1.0 - dy)),
        (x1, y0, dx * (1.0 - dy)),
        (x0, y1, (1.0 - dx) * dy),
        (x1, y1, dx * dy),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

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

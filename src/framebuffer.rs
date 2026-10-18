use nalgebra::Vector3;

pub struct Framebuffer {
    pub width: usize,
    pub height: usize,
    /// 每个输出像素的线性 XYZ 颜色值，尚未进行 sRGB 编码。
    pub pixels: Vec<Vector3<f64>>,
}

impl Framebuffer {
    pub fn new(width: usize, height: usize) -> Self {
        assert!(width > 0 && height > 0, "图片宽高必须大于0");

        let count = width.checked_mul(height).expect("图片像素数量溢出");

        Self {
            width,
            height,
            pixels: vec![Vector3::zeros(); count],
        }
    }

    fn index(&self, x: usize, y: usize) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(y * self.width + x)
    }

    pub fn get(&self, x: usize, y: usize) -> Option<&Vector3<f64>> {
        self.index(x, y).map(|idx| &self.pixels[idx])
    }

    pub fn set(&mut self, x: usize, y: usize, xyz: Vector3<f64>) -> bool {
        if !xyz.iter().all(|v| v.is_finite()) {
            return false;
        }

        let Some(i) = self.index(x, y) else {
            return false;
        };
        self.pixels[i] = xyz;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framebuffer_has_expected_size_and_starts_black() {
        let fb = Framebuffer::new(3, 2);

        assert_eq!(fb.pixels.len(), 6);
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(fb.get(x, y), Some(&Vector3::zeros()));
            }
        }
    }

    #[test]
    fn setting_color_uses_row_major_index_and_preserves_other_pixels() {
        let mut fb = Framebuffer::new(3, 2);
        let color = Vector3::new(0.2, 0.3, 0.4);

        assert!(fb.set(2, 1, color));
        assert_eq!(fb.get(2, 1), Some(&color));
        assert_eq!(fb.pixels[5], color);
        for pixel in &fb.pixels[..5] {
            assert_eq!(*pixel, Vector3::zeros());
        }
    }

    #[test]
    fn out_of_bounds_access_returns_failure_without_changes() {
        let mut fb = Framebuffer::new(3, 2);
        let original = fb.pixels.clone();
        let color = Vector3::new(0.2, 0.3, 0.4);

        for (x, y) in [(3, 0), (0, 2), (usize::MAX, usize::MAX)] {
            assert_eq!(fb.get(x, y), None);
            assert!(!fb.set(x, y, color));
        }
        assert_eq!(fb.pixels, original);
    }

    #[test]
    fn non_finite_colors_are_rejected_without_changes() {
        let mut fb = Framebuffer::new(3, 2);
        let color = Vector3::new(0.2, 0.3, 0.4);
        assert!(fb.set(2, 1, color));
        let original = *fb.get(2, 1).unwrap();

        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for channel in 0..3 {
                let mut invalid_color = color;
                invalid_color[channel] = value;
                assert!(!fb.set(2, 1, invalid_color));
                assert_eq!(fb.get(2, 1), Some(&original));
            }
        }
    }

    #[test]
    #[should_panic(expected = "图片宽高必须大于0")]
    fn zero_width_is_rejected() {
        Framebuffer::new(0, 2);
    }

    #[test]
    #[should_panic(expected = "图片宽高必须大于0")]
    fn zero_height_is_rejected() {
        Framebuffer::new(3, 0);
    }

    #[test]
    #[should_panic(expected = "图片像素数量溢出")]
    fn pixel_count_overflow_is_rejected() {
        Framebuffer::new(usize::MAX, 2);
    }
}

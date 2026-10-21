//! 输出图像的二维颜色缓冲，内部按行存储在一维数组中。
//! 图片位置使用 (x, y)，每个位置保存的三分量颜色使用 CIE (X, Y, Z)。

use nalgebra::Vector3;

/// 保存输出图像的尺寸和颜色数据。
/// 例如宽 3、高 2 的图像依次保存第 0 行的三个像素，再保存第 1 行的三个像素。
pub struct Framebuffer {
    /// 图片宽度，单位为像素。
    pub width: usize,
    /// 图片高度，单位为像素。
    pub height: usize,
    /// 每个输出像素的线性 XYZ 颜色值，尚未进行 sRGB 编码。
    /// Vec 是一维数组；Vector3<f64> 是其中一个像素的三个颜色分量。
    pub pixels: Vec<Vector3<f64>>,
}

impl Framebuffer {
    /// 创建指定尺寸的全黑图像；宽高为零或像素数量乘法溢出时会 panic。
    pub fn new(width: usize, height: usize) -> Self {
        assert!(width > 0 && height > 0, "图片宽高必须大于0");

        // checked_mul 安全计算 width * height：乘法溢出时返回 None。
        // expect 取出成功时的数量；溢出时显示指定错误，避免使用错误的数组长度。
        let count = width.checked_mul(height).expect("图片像素数量溢出");

        Self {
            width,
            height,
            // vec![值; 数量] 创建 count 个初始元素。XYZ = (0, 0, 0) 表示黑色。
            pixels: vec![Vector3::zeros(); count],
        }
    }

    /// 内部辅助方法：将图片坐标转换成数组下标，越界返回 None。
    fn index(&self, x: usize, y: usize) -> Option<usize> {
        // 下标从 0 开始，因此 x 的有效范围是 0..width，y 同理。
        // 先检查坐标，再计算下标，避免对极大越界坐标做乘法。
        if x >= self.width || y >= self.height {
            return None;
        }
        // y * width 跳过前面的 y 行，再加 x 找到当前行中的像素。
        // 例如 width = 3 时，图片位置 (2, 1) 对应下标 1 * 3 + 2 = 5。
        Some(y * self.width + x)
    }

    /// 按图片坐标读取颜色；&self 表示只读，返回的 &Vector3 借用缓冲区中的数据。
    pub fn get(&self, x: usize, y: usize) -> Option<&Vector3<f64>> {
        // map 将 Some(idx) 转为 Some(&pixels[idx])，遇到 None 则仍返回 None。
        self.index(x, y).map(|idx| &self.pixels[idx])
    }

    /// 覆盖指定位置的 XYZ 颜色；&mut self 表示需要修改缓冲区。
    /// 成功返回 true；坐标越界或颜色含 NaN、无穷大时返回 false，并保留原数据。
    pub fn set(&mut self, x: usize, y: usize, xyz: Vector3<f64>) -> bool {
        // iter().all(...) 要求三个分量都通过检查；|v| ... 是作用于每个分量的闭包。
        // 这里只检查数值是否有限，线性颜色的亮度可以大于 1。
        if !xyz.iter().all(|v| v.is_finite()) {
            return false;
        }

        // let ... else：匹配到 Some 时取出 i；遇到 None 时提前返回失败。
        let Some(i) = self.index(x, y) else {
            return false;
        };
        // 将这个位置的颜色替换为 xyz。后续的光子能量累积需要另外实现加法操作。
        self.pixels[i] = xyz;
        true
    }

    pub fn accelerated(&mut self, x: usize, y: usize, xyz: Vector3<f64>) -> bool {
        if !xyz.iter().all(|v| v.is_finite()) {
            return false;
        }

        let Some(i) = self.index(x, y) else {
            return false;
        };
        // 将这个位置的颜色替换为 xyz。后续的光子能量累积需要另外实现加法操作。
        self.pixels[i] += xyz;
        true
    }
}

// 此模块只在测试配置下编译；#[test] 标记的函数由 cargo test 执行。
#[cfg(test)]
mod tests {
    // 将父模块中的类型和导入引入测试作用域。
    use super::*;

    #[test]
    fn framebuffer_has_expected_size_and_starts_black() {
        // 非正方形尺寸能同时检查宽高的使用方式和全黑初始化。
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
        // (2, 1) 必须写入下标 5，前五个像素应保持黑色。
        assert_eq!(fb.pixels[5], color);
        for pixel in &fb.pixels[..5] {
            assert_eq!(*pixel, Vector3::zeros());
        }
    }

    #[test]
    fn out_of_bounds_access_returns_failure_without_changes() {
        let mut fb = Framebuffer::new(3, 2);
        // clone 保存一份独立快照，用于检查失败的写入没有改变任何像素。
        let original = fb.pixels.clone();
        let color = Vector3::new(0.2, 0.3, 0.4);

        // 分别检查横向越界、纵向越界和极大坐标。
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

        // 在 X、Y、Z 的每个位置分别放入三种非法值，检查写入失败后仍保留原颜色。
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for channel in 0..3 {
                let mut invalid_color = color;
                invalid_color[channel] = value;
                assert!(!fb.set(2, 1, invalid_color));
                assert_eq!(fb.get(2, 1), Some(&original));
            }
        }
    }

    // should_panic 表示测试预期发生 panic；expected 同时核对错误信息。
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

//! 输出图像的二维颜色缓冲，内部按行存储在一维数组中。
//! 图片位置使用 (x, y)，每个位置保存的三分量颜色使用 CIE (X, Y, Z)。

use nalgebra::Vector3;

/// 保存输出图像的尺寸和颜色数据。
/// 例如宽 3、高 2 的图像依次保存第 0 行的三个像素，再保存第 1 行的三个像素。
pub struct Framebuffer {
    /// 图片宽度，单位为像素。
    pub(crate) width: usize,
    /// 图片高度，单位为像素。
    pub(crate) height: usize,
    /// 每个输出像素的线性 XYZ 颜色值，尚未进行 sRGB 编码。
    /// Vec 是一维数组；`Vector3<f64>` 是其中一个像素的三个颜色分量。
    pub(crate) pixels: Vec<Vector3<f64>>,
}

impl Framebuffer {
    /// 创建全黑的线性 XYZ 缓冲，供相机和地面辐照度图存储结果。
    ///
    /// 输入：正宽高（像素）；输出：含 width·height 个零向量的 Framebuffer；零尺寸、数量溢出或分配失败时返回错误。
    pub fn new(width: usize, height: usize) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("image dimensions must be positive".into());
        }
        let count = width
            .checked_mul(height)
            .filter(|&n| n <= isize::MAX as usize / size_of::<Vector3<f64>>())
            .ok_or("image allocation size overflow")?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(count)
            .map_err(|_| "image allocation failed")?;
        pixels.resize(count, Vector3::zeros());
        Ok(Self {
            width,
            height,
            pixels,
        })
    }
    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    pub fn pixels(&self) -> &[Vector3<f64>] {
        &self.pixels
    }
    pub fn pixels_mut(&mut self) -> &mut [Vector3<f64>] {
        &mut self.pixels
    }

    /// 把图片坐标变成一维数组下标，供像素读写共用。
    ///
    /// 输入：零起始 x/y；输出：i=y·width+x，坐标越界返回 None。
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

    /// 读取图片坐标 (x,y) 处的线性 XYZ，返回缓冲中像素的只读引用；坐标越界返回 None。
    pub fn get(&self, x: usize, y: usize) -> Option<&Vector3<f64>> {
        // map 将 Some(idx) 转为 Some(&pixels[idx])，遇到 None 则仍返回 None。
        self.index(x, y).map(|idx| &self.pixels[idx])
    }

    /// 用一份线性 XYZ 覆盖像素，供写入最终相机结果。
    ///
    /// 输入：坐标 x/y 和有限 xyz；输出：成功为 true，越界或颜色非有限为 false 且保留原值。
    /// 允许负值和大于 1 的线性数据，显示范围由保存入口处理。
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
        self.pixels[i] = xyz;
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
        let fb = Framebuffer::new(3, 2).unwrap();

        assert_eq!(fb.pixels.len(), 6);
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(fb.get(x, y), Some(&Vector3::zeros()));
            }
        }
    }

    #[test]
    fn setting_color_uses_row_major_index_and_preserves_other_pixels() {
        let mut fb = Framebuffer::new(3, 2).unwrap();
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
        let mut fb = Framebuffer::new(3, 2).unwrap();
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
        let mut fb = Framebuffer::new(3, 2).unwrap();
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

    #[test]
    fn zero_width_is_rejected() {
        assert!(Framebuffer::new(0, 2).is_err());
    }

    #[test]
    fn zero_height_is_rejected() {
        assert!(Framebuffer::new(3, 0).is_err());
    }

    #[test]
    fn pixel_count_overflow_is_rejected() {
        assert!(Framebuffer::new(usize::MAX, 2).is_err());
    }
}

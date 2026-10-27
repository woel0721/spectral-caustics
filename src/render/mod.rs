//! 图像输出链路：XYZ -> 线性 sRGB -> 显示范围裁剪 -> sRGB8 -> PNG。
//! Render 使用配置中的曝光，颜色计算与存储使用 f64，导出时才进行 8 位量化。

pub mod caustics;
pub mod framebuffer;
pub mod utils;

use self::framebuffer::Framebuffer;
use crate::{config::rendersetting::RenderSetting, optics::iormode::IorMode, scene::Scene};
use image::{ImageFormat, Rgb, RgbImage};
use nalgebra::{Matrix3, Vector3};
use std::{error::Error, fs, path::Path};

/// 实际运行的渲染器：保存已校验参数和可选的专用 Rayon 线程池。
/// 场景与配置文件独立，渲染时只借用 Scene 中已建立的物体。
pub struct Render {
    settings: RenderSetting,
    pool: Option<rayon::ThreadPool>,
}

impl Render {
    pub fn new(settings: RenderSetting) -> Result<Self, Box<dyn Error>> {
        settings.validate()?;
        let pool = settings
            .thread_count
            .map(|count| rayon::ThreadPoolBuilder::new().num_threads(count).build())
            .transpose()?;
        Ok(Self { settings, pool })
    }

    pub fn settings(&self) -> &RenderSetting {
        &self.settings
    }

    pub fn thread_count(&self) -> usize {
        self.pool.as_ref().map_or_else(
            rayon::current_num_threads,
            rayon::ThreadPool::current_num_threads,
        )
    }

    pub fn render(&self, scene: &Scene) -> Result<Framebuffer, Box<dyn Error>> {
        self.render_mode(scene, self.settings.mode)
    }

    /// 用相同的其余配置生成另一种折射率模式的对照图。
    pub fn render_mode(&self, scene: &Scene, mode: IorMode) -> Result<Framebuffer, Box<dyn Error>> {
        let ground_id = scene.ground_id.ok_or("运行时场景缺少接收地面编号")?;
        let settings = RenderSetting {
            mode,
            ..self.settings.clone()
        };
        // install 的返回值跨线程传递，转换为可 Send 的错误文本，再交回公共错误接口。
        let work = || {
            caustics::render_spectral_caustics(scene, scene.prism_id, ground_id, &settings)
                .map_err(|error| error.to_string())
        };
        let result = match &self.pool {
            Some(pool) => pool.install(work),
            None => work(),
        };
        result.map_err(Into::into)
    }

    pub fn ground_hits(&self, scene: &Scene) -> Result<Framebuffer, Box<dyn Error>> {
        caustics::render_ground_hits(
            scene,
            scene.ground_id.ok_or("场景缺少接收地面")?,
            &self.settings,
        )
    }

    pub fn refracted_hits<const N: usize>(
        &self,
        scene: &Scene,
        wavelengths_nm: [Option<f64>; N],
        mode: IorMode,
    ) -> Result<[Framebuffer; N], Box<dyn Error>> {
        caustics::render_refracted_ground_hits(
            scene,
            scene.prism_id,
            scene.ground_id.ok_or("场景缺少接收地面")?,
            &self.settings,
            wavelengths_nm,
            mode,
        )
    }

    pub fn save(
        &self,
        framebuffer: &Framebuffer,
        path: impl AsRef<Path>,
    ) -> Result<(), Box<dyn Error>> {
        save_png_with_exposure(framebuffer, path, self.settings.exposure)
    }
}

/// 将 D65 白点约定下的 CIE XYZ 转为线性 sRGB，参考白的 Y = 1。
/// 保留超出 [0, 1] 的结果；save_png 负责显示范围处理和后续编码。
/// 输入非有限值或计算溢出时返回 None。
///
/// 矩阵来源：W3C CSS Color 4 的 XYZ_to_lin_sRGB：
/// https://www.w3.org/TR/css-color-4/#color-conversion-code
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

/// 将一个 [0, 1] 范围内的线性 RGB 通道编码为显示 sRGB 浮点值。
/// R、G、B 各自使用同一公式；例如线性值 0.5 编码后约为 0.735357。
/// NaN、无穷大或超出输入范围时返回 None；调用者需先处理曝光和显示范围。
///
/// 公式来源：ICC sRGB 编码登记表的 Color component transfer function：
/// https://registry.color.org/rgb-registry/srgb
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

/// 将三个 [0, 1] 内的线性通道编码并量化为 [R, G, B] 字节。
/// 任一通道非有限或超出范围时返回 None；显示范围裁剪由 save_png 完成。
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

/// 将线性 XYZ 缓冲保存为 8 位 sRGB PNG。
/// 固定曝光为 1.0，超出显示范围的线性 RGB 采用逐通道裁剪。
pub fn save_png(framebuffer: &Framebuffer, path: &str) -> Result<(), Box<dyn Error>> {
    save_png_with_exposure(framebuffer, path, 1.0)
}

/// 按配置在线性空间施加曝光，再裁剪、sRGB 编码和保存 PNG。
pub fn save_png_with_exposure(
    framebuffer: &Framebuffer,
    path: impl AsRef<Path>,
    exposure: f64,
) -> Result<(), Box<dyn Error>> {
    if !exposure.is_finite() || exposure < 0.0 {
        return Err("曝光必须有限且非负".into());
    }
    // image 使用 u32 表示宽高；无法转换时返回错误。
    let width = u32::try_from(framebuffer.width)?;
    let height = u32::try_from(framebuffer.height)?;

    // 检查公开字段是否仍满足“像素数 = 宽 × 高”。
    if width == 0
        || height == 0
        || framebuffer.width.checked_mul(framebuffer.height) != Some(framebuffer.pixels.len())
    {
        return Err("Framebuffer 尺寸与像素数量不匹配".into());
    }

    let mut image = RgbImage::new(width, height);

    for y in 0..height {
        for x in 0..width {
            // 读取当前位置的 XYZ。
            let xyz = *framebuffer
                .get(x as usize, y as usize)
                .ok_or("像素坐标越界")?;

            // XYZ -> 线性 RGB。
            let linear_rgb = xyz_to_linear_srgb(xyz).ok_or("XYZ 非法或颜色计算溢出")? * exposure;
            if !linear_rgb.iter().all(|v| v.is_finite()) {
                return Err("曝光计算溢出".into());
            }

            // 对三个通道分别裁剪，满足编码函数的输入要求。
            let bounded_rgb = linear_rgb.map(|v| v.clamp(0.0, 1.0));

            // 复用已有函数：编码并量化为 [u8; 3]。
            let rgb8 = linear_rgb_to_srgb8(bounded_rgb).ok_or("sRGB 编码失败")?;

            // Rgb(...) 将三个字节包装成 image 库的像素类型。
            image.put_pixel(x, y, Rgb(rgb8));
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn png_roundtrip_preserves_dimensions_positions_and_srgb_encoding() -> Result<(), Box<dyn Error>>
    {
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
        let mut framebuffer = Framebuffer::new(3, 2);
        let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
        // 三原色 XYZ 来自上述 W3C 文档的 lin_sRGB_to_XYZ。
        let cases = [
            (
                0,
                0,
                Vector3::new(506752.0 / 1228815.0, 87098.0 / 409605.0, 7918.0 / 409605.0),
                [255, 0, 0],
            ),
            (
                1,
                0,
                Vector3::new(87881.0 / 245763.0, 175762.0 / 245763.0, 87881.0 / 737289.0),
                [0, 255, 0],
            ),
            (
                2,
                0,
                Vector3::new(12673.0 / 70218.0, 12673.0 / 175545.0, 1001167.0 / 1053270.0),
                [0, 0, 255],
            ),
            (0, 1, white_xyz, [255, 255, 255]),
            // 线性亮度减半，经过 sRGB 编码后应量化为 188，而非直接乘 255 得到的 128。
            (1, 1, white_xyz * 0.5, [188, 188, 188]),
            (2, 1, Vector3::zeros(), [0, 0, 0]),
        ];

        for &(x, y, xyz, _) in &cases {
            assert!(framebuffer.set(x, y, xyz));
        }

        save_png(&framebuffer, path.to_str().ok_or("临时路径必须是 UTF-8")?)?;
        let decoded = image::open(&path)?;
        // 先核对原始像素格式，避免格式转换掩盖错误的位深或通道数量。
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        let pixels = decoded.into_rgb8();
        assert_eq!(pixels.dimensions(), (3, 2));
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

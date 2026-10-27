//! 图像输出链路：XYZ -> 线性 sRGB -> 显示范围裁剪 -> sRGB8 -> PNG。
//! Render 使用配置中的曝光，颜色计算与存储使用 f64，导出时才进行 8 位量化。

pub mod caustics;
pub mod framebuffer;
pub mod utils;

use self::{
    framebuffer::Framebuffer,
    utils::{linear_rgb_to_srgb8, normal_to_rgb8, xyz_to_linear_srgb},
};
use crate::{config::rendersetting::RenderSetting, optics::iormode::IorMode, scene::Scene};
use image::{ImageFormat, Rgb, RgbImage};
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
    use nalgebra::Vector3;

    #[test]
    fn camera_debug_visualizes_world_normals_and_preserves_background() {
        let mut config = crate::config::sceneconfig::SceneConfig::default();
        config.camera.position = [0.0, -4.0, 2.0];
        config.camera.target = [0.0, 0.0, 1.0];
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

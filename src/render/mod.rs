//! 图像输出链路：XYZ -> 线性 sRGB -> 显示范围裁剪 -> sRGB8 -> PNG。
//! Render 使用配置中的曝光，颜色计算与存储使用 f64，导出时才进行 8 位量化。

pub mod caustics;
pub mod framebuffer;
pub mod integrator;
pub mod utils;

pub use caustics::SpectralLighting;
pub use integrator::{
    CameraLightingImages, lambertian_radiance, render_camera_glass, render_camera_glass_components,
    sample_ground_irradiance,
};

use self::{
    framebuffer::Framebuffer,
    utils::{linear_rgb_to_srgb8, xyz_to_linear_srgb},
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

    pub fn thread_count(&self) -> usize {
        self.pool.as_ref().map_or_else(
            rayon::current_num_threads,
            rayon::ThreadPool::current_num_threads,
        )
    }

    pub fn render(&self, scene: &Scene) -> Result<Framebuffer, Box<dyn Error>> {
        self.render_mode(scene, self.settings.mode)
    }

    /// 一批光子生成直射/焦散调试图，并携带同尺度的解析太阳光照明。
    pub fn render_lighting(&self, scene: &Scene) -> Result<SpectralLighting, Box<dyn Error>> {
        let ground_id = scene.ground_id.ok_or("运行时场景缺少接收地面编号")?;
        let work = || {
            caustics::render_spectral_lighting(scene, scene.prism_id, ground_id, &self.settings)
                .map_err(|error| error.to_string())
        };
        let result = match &self.pool {
            Some(pool) => pool.install(work),
            None => work(),
        };
        result.map_err(Into::into)
    }

    /// 使用配置尺寸及专用线程池，一批相机路径同时生成三张未曝光的 XYZ 图。
    pub fn render_camera_glass_components(
        &self,
        scene: &Scene,
        lighting: &SpectralLighting,
        spp: usize,
        seed: u64,
    ) -> Result<CameraLightingImages, Box<dyn Error>> {
        let work = || {
            integrator::render_camera_glass_components_impl(
                scene,
                lighting,
                self.settings.width,
                self.settings.height,
                spp,
                seed,
                self.settings.debug_path,
            )
            .map_err(|error| error.to_string())
        };
        let result = match &self.pool {
            Some(pool) => pool.install(work),
            None => work(),
        };
        result.map_err(Into::into)
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

    pub fn save(
        &self,
        framebuffer: &Framebuffer,
        path: impl AsRef<Path>,
    ) -> Result<(), Box<dyn Error>> {
        save_png_with_exposure(framebuffer, path, self.settings.exposure)
    }
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

        save_png_with_exposure(&framebuffer, &path, 1.0)?;
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

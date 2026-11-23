//! 从相机发射光线，沿表面反射/透射路径累计照明，最后输出线性 XYZ 图像。
//!
//! 路径权重 β（throughput）是到达当前交点前所有散射权重的乘积；局部贡献按 β·L 加入图像。
//! NEE 是在交点主动抽取光源位置或方向来估计直接光；MIS 将材质和环境两种采样策略组合，
//! 用各自 PDF 修正贡献，减少噪声。RR（Russian Roulette）随机截断较深路径，再补偿存活权重。
//! Direct 包含太阳/圆盘、环境、背景及允许的后续路径贡献；Caustics 只读取首个连续地面顶点的焦散图。
//! RGB 路径传输三个线性颜色分量；Hero 路径固定一个波长并传输标量，最终再做波长积分。

mod ground;
mod lighting;
pub mod path;
mod rgb;
pub(crate) mod spectral;
pub mod transport;
pub mod types;

pub use rgb::integrate_rgb;
pub(crate) use rgb::validate_rgb_render;
pub use spectral::integrate_spectral;
pub(crate) use spectral::validate_spectral_render;
pub use types::CameraLightingImages;

/// 有介质边界时从实际交点出发，由几何求交的 epsilon 排除自交。
/// 这也保护普通表面发出的射线，法线偏移可能越过紧邻的玻璃。
/// 其余场景沿单位法线移动 side·10ε，side 为 +1 或 -1。
fn surface_ray_origin(
    has_medium_interface: bool,
    position: nalgebra::Vector3<f64>,
    normal: nalgebra::Vector3<f64>,
    side: f64,
) -> nalgebra::Vector3<f64> {
    if has_medium_interface {
        position
    } else {
        position + normal * (side * 10.0 * crate::geometry::ray::INTERSECTION_EPSILON)
    }
}

/// Placeholder RNG for streams that are statically inactive in a scene (no medium / no environment).
pub(crate) struct UnusedRng;

impl rand::RngCore for UnusedRng {
    fn next_u32(&mut self) -> u32 {
        unreachable!("inactive camera RNG stream must not be sampled")
    }
    fn next_u64(&mut self) -> u64 {
        unreachable!("inactive camera RNG stream must not be sampled")
    }
    fn fill_bytes(&mut self, _dest: &mut [u8]) {
        unreachable!("inactive camera RNG stream must not be sampled")
    }
}

pub(super) struct RenderedPixel {
    pub direct: nalgebra::Vector3<f64>,
    pub caustics: nalgebra::Vector3<f64>,
    #[cfg(feature = "diagnostics")]
    pub statistics: Option<crate::diagnostics::PathDebugPixel>,
}

pub(super) fn render_camera_pixels(
    width: usize,
    height: usize,
    #[cfg(feature = "diagnostics")] debug_target: Option<Option<usize>>,
    render_pixel: impl Fn(usize) -> Result<RenderedPixel, String> + Sync,
) -> Result<CameraLightingImages, Box<dyn std::error::Error + Send + Sync>> {
    use rayon::prelude::*;
    let mut images = CameraLightingImages {
        direct: crate::film::Framebuffer::new(width, height)?,
        caustics: crate::film::Framebuffer::new(width, height)?,
        combined: crate::film::Framebuffer::new(width, height)?,
        #[cfg(feature = "diagnostics")]
        path_debug: debug_target
            .map(|target| crate::diagnostics::PathDebugBuffer::new(width, height, target)),
    };
    let write = |index,
                 combined: &mut nalgebra::Vector3<f64>,
                 direct: &mut nalgebra::Vector3<f64>,
                 caustics: &mut nalgebra::Vector3<f64>|
     -> Result<(), String> {
        let pixel = render_pixel(index)?;
        *direct = pixel.direct;
        *caustics = pixel.caustics;
        *combined = pixel.direct + pixel.caustics;
        Ok(())
    };
    #[cfg(feature = "diagnostics")]
    if let Some(debug) = &mut images.path_debug {
        images
            .combined
            .pixels
            .par_iter_mut()
            .zip(images.direct.pixels.par_iter_mut())
            .zip(images.caustics.pixels.par_iter_mut())
            .zip(debug.pixels.par_iter_mut())
            .enumerate()
            .try_for_each(
                |(index, (((combined, direct), caustics), stats))| -> Result<(), String> {
                    let pixel = render_pixel(index)?;
                    *direct = pixel.direct;
                    *caustics = pixel.caustics;
                    *combined = pixel.direct + pixel.caustics;
                    *stats = pixel.statistics.expect("enabled observer");
                    Ok(())
                },
            )?;
    } else {
        images
            .combined
            .pixels
            .par_iter_mut()
            .zip(images.direct.pixels.par_iter_mut())
            .zip(images.caustics.pixels.par_iter_mut())
            .enumerate()
            .try_for_each(|(index, ((combined, direct), caustics))| {
                write(index, combined, direct, caustics)
            })?;
    }
    #[cfg(not(feature = "diagnostics"))]
    images
        .combined
        .pixels
        .par_iter_mut()
        .zip(images.direct.pixels.par_iter_mut())
        .zip(images.caustics.pixels.par_iter_mut())
        .enumerate()
        .try_for_each(|(index, ((combined, direct), caustics))| {
            write(index, combined, direct, caustics)
        })?;
    Ok(images)
}

#[cfg(test)]
pub(crate) mod tests;

pub use types::CameraPathState;

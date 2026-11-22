use crate::config::RenderJob;
use crate::integrator::camera::spectral::{packet_primary_sample, spectral_camera_setup};
use crate::integrator::camera::{
    UnusedRng, path::trace_camera_path, transport::Caustics, types::CameraPathState,
};
use crate::optics::dispersion::IorMode;
use crate::spectrum::emission::{WAVELENGTH_MAX_NM, WAVELENGTH_MIN_NM};
use rayon::prelude::*;
use std::error::Error;

#[derive(Debug)]
pub struct FixedWavelengthImage {
    pub width: usize,
    pub height: usize,
    pub wavelength_nm: f64,
    /// 行优先的平均单波长辐亮度，尚未乘 CIE、波长 PDF、曝光或编码。
    pub radiance: Vec<f64>,
}

/// 用相同主射线和相同初始路径随机序列，分别渲染指定波长，供色散位置对照诊断。
///
/// 输入：scene、尺寸、每张图每像素空间 spp、seed、IOR 模式和非空波长列表（380–780 nm）。
///
/// 输出：按输入波长顺序排列的 FixedWavelengthImage，保存原始平均 Lλ；非法参数或追踪失败返回 Err。
///
/// 公式：每图 Lλ=(1/spp)ΣLλ,sample，总路径数为 width·height·spp·波长数。
/// 不乘 CIE、波长 PDF 或曝光，不读取光源焦散；各波长复制 RNG 状态后独立追踪，不强制同步物理事件。
pub fn render_fixed_wavelength_camera(
    scene: &RenderJob,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    ior_mode: IorMode,
    wavelengths_nm: &[f64],
) -> Result<Vec<FixedWavelengthImage>, Box<dyn Error + Send + Sync>> {
    if spp == 0 || wavelengths_nm.is_empty() {
        return Err("固定波长相机要求正 SPP 和非空波长列表".into());
    }
    if wavelengths_nm
        .iter()
        .any(|nm| !(WAVELENGTH_MIN_NM..=WAVELENGTH_MAX_NM).contains(nm))
    {
        return Err("固定波长必须有限且位于 380–780 nm".into());
    }
    let lanes = wavelengths_nm.len();
    let paths = spp.checked_mul(lanes).ok_or("固定波长相机路径数量溢出")?;
    let (sun_wi, sun_strength) = spectral_camera_setup(scene, width, height, paths)?;
    let pixel_count = width
        .checked_mul(height)
        .ok_or("固定波长相机缓冲数量溢出")?;
    let _total_count = pixel_count
        .checked_mul(lanes)
        .ok_or("固定波长相机缓冲数量溢出")?;
    let mut images = Vec::new();
    images
        .try_reserve_exact(lanes)
        .map_err(|_| "无法分配固定波长相机缓冲")?;
    for &wavelength_nm in wavelengths_nm {
        let mut radiance = Vec::new();
        radiance
            .try_reserve_exact(pixel_count)
            .map_err(|_| "无法分配固定波长相机缓冲")?;
        radiance.resize(pixel_count, 0.0);
        images.push(FixedWavelengthImage {
            width,
            height,
            wavelength_nm,
            radiance,
        });
    }
    let has_environment = scene.scene.environment.is_some();
    let has_medium = scene.scene.has_medium();
    let strip_pixels = pixel_count.clamp(1, 4096);
    let strip_len = strip_pixels
        .checked_mul(lanes)
        .ok_or("固定波长相机缓冲数量溢出")?;
    let mut strip = Vec::new();
    strip
        .try_reserve_exact(strip_len)
        .map_err(|_| "无法分配固定波长相机缓冲")?;
    for start in (0..pixel_count).step_by(strip_pixels) {
        let count = (pixel_count - start).min(strip_pixels);
        strip.clear();
        strip.resize(count * lanes, 0.0);
        let result: Result<(), String> =
            strip
                .par_chunks_mut(lanes)
                .enumerate()
                .try_for_each(|(offset_index, pixel)| {
                    let pixel_index = start + offset_index;
                    let mut unused_environment = UnusedRng;
                    let mut unused_medium = UnusedRng;
                    for sample in 0..spp {
                        let (ray, common) = packet_primary_sample(
                            scene,
                            width,
                            height,
                            seed,
                            pixel_index,
                            sample,
                            has_environment,
                            has_medium,
                        )?;
                        let mut common = Some(common);
                        for (lane, (radiance, &nm)) in
                            pixel.iter_mut().zip(wavelengths_nm).enumerate()
                        {
                            let mut lane_rngs = if lane + 1 == lanes {
                                common.take().expect("last lane consumes common RNGs")
                            } else {
                                common.as_ref().expect("common RNGs").clone()
                            };
                            let mut state = CameraPathState::spectral(
                                ray,
                                nm,
                                scene.camera_options.max_diffuse_bounces,
                            )?;
                            trace_camera_path(
                                scene,
                                &mut state,
                                &mut lane_rngs
                                    .as_camera_rngs(&mut unused_environment, &mut unused_medium),
                                sun_wi,
                                sun_strength,
                                Caustics::None,
                                ior_mode,
                                #[cfg(feature = "diagnostics")]
                                None,
                            )
                            .map_err(|e| e.to_string())?;
                            *radiance += state.radiance.direct / spp as f64;
                        }
                    }
                    if pixel.iter().any(|v| !v.is_finite() || *v < 0.0) {
                        return Err("固定波长辐亮度累积溢出".into());
                    }
                    Ok(())
                });
        result.map_err(Box::<dyn Error + Send + Sync>::from)?;
        for (offset_index, pixel) in strip.chunks_exact(lanes).enumerate() {
            let pixel_index = start + offset_index;
            for (image, &value) in images.iter_mut().zip(pixel) {
                image.radiance[pixel_index] = value;
            }
        }
    }
    Ok(images)
}

use crate::integrator::camera::{
    path::trace_camera_path,
    transport::Caustics,
    types::{CameraPathRngs, CameraPathState},
};
// 固定波长相机诊断：相同主射线、相同路径随机流，返回未积分的标量 Lλ。

use crate::config::RenderJob;
use crate::integrator::camera::spectral::PREFIX_STREAM;
use crate::integrator::camera::spectral::packet_rng;
use crate::integrator::camera::spectral::validate_spectral_camera;
use crate::optics::dispersion::IorMode;
use crate::sampling::rng::CAMERA_STREAM;
use crate::sampling::rng::DIFFUSE_STREAM;
use crate::sampling::rng::ENVIRONMENT_STREAM;
use crate::spectrum::emission::WAVELENGTH_MAX_NM;
use crate::spectrum::emission::WAVELENGTH_MIN_NM;
use nalgebra::Vector2;
use rand::Rng;
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
    validate_spectral_camera(scene, width, height, paths)?;
    let (sun_wi, sun_strength) = scene
        .scene
        .light
        .analytic_sun()?
        .unwrap_or((nalgebra::Vector3::z(), 0.0));
    let count = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(lanes))
        .ok_or("固定波长相机缓冲数量溢出")?;
    let mut interleaved = Vec::new();
    interleaved
        .try_reserve_exact(count)
        .map_err(|_| "无法分配固定波长相机缓冲")?;
    interleaved.resize(count, 0.0);
    let result: Result<(), String> =
        interleaved
            .par_chunks_mut(lanes)
            .enumerate()
            .try_for_each(|(pixel_index, pixel)| {
                for sample in 0..spp {
                    let mut jitter = packet_rng(seed, pixel_index, sample, 0, CAMERA_STREAM);
                    let offset = Vector2::new(jitter.random::<f64>(), jitter.random::<f64>());
                    let ray = scene
                        .camera
                        .ray_for_pixel_sample(
                            pixel_index % width,
                            pixel_index / width,
                            width,
                            height,
                            offset,
                        )
                        .ok_or("固定波长相机光线无效")?;
                    let common = [
                        packet_rng(seed, pixel_index, sample, 0, PREFIX_STREAM),
                        packet_rng(seed, pixel_index, sample, 0, ENVIRONMENT_STREAM),
                        packet_rng(seed, pixel_index, sample, 0, DIFFUSE_STREAM),
                    ];
                    for (radiance, &nm) in pixel.iter_mut().zip(wavelengths_nm) {
                        let [mut prefix, mut environment, mut continuation] = common.clone();
                        let mut state = CameraPathState::spectral(
                            ray,
                            nm,
                            scene.camera_options.max_diffuse_bounces,
                        )?;
                        trace_camera_path(
                            scene,
                            &mut state,
                            &mut CameraPathRngs {
                                camera: &mut prefix,
                                environment: &mut environment,
                                diffuse: &mut continuation,
                            },
                            sun_wi,
                            sun_strength,
                            (None).map_or(Caustics::None, Caustics::Spectral),
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
    Ok(wavelengths_nm
        .iter()
        .enumerate()
        .map(|(lane, &wavelength_nm)| FixedWavelengthImage {
            width,
            height,
            wavelength_nm,
            radiance: interleaved
                .chunks_exact(lanes)
                .map(|pixel| pixel[lane])
                .collect(),
        })
        .collect())
}

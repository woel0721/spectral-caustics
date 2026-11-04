//! 固定波长相机诊断：相同主射线、相同路径随机流，返回未积分的标量 Lλ。

use super::{
    CAMERA_STREAM, DIFFUSE_STREAM, ENVIRONMENT_STREAM, PREFIX_STREAM, SpectralPathState,
    WAVELENGTH_MAX_NM, WAVELENGTH_MIN_NM, packet_rng, trace_spectral_camera_path,
    validate_spectral_camera,
};
use crate::{optics::iormode::IorMode, scene::Scene};
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

/// spp 是每张单波长图的每像素空间样本数，总路径数为 width*height*spp*波长数。
/// 每个空间样本只生成一次主射线，各波长从同样的三类 RNG 状态副本开始。
/// 使用 Packet 相机的主射线种子，因此相同 seed 下的前几个空间样本也相同。
pub fn render_fixed_wavelength_camera(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    ior_mode: IorMode,
    wavelengths_nm: &[f64],
) -> Result<Vec<FixedWavelengthImage>, Box<dyn Error>> {
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
                        let mut state = SpectralPathState::new(ray, nm, scene.max_diffuse_bounces)?;
                        trace_spectral_camera_path(
                            scene,
                            &mut state,
                            ior_mode,
                            &mut prefix,
                            &mut environment,
                            &mut continuation,
                        )
                        .map_err(|e| e.to_string())?;
                        *radiance += state.radiance / spp as f64;
                    }
                }
                if pixel.iter().any(|v| !v.is_finite() || *v < 0.0) {
                    return Err("固定波长辐亮度累积溢出".into());
                }
                Ok(())
            });
    result.map_err(Box::<dyn Error>::from)?;
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

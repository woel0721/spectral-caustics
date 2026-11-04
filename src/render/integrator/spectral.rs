//! 单波长/四波长 Packet 相机：共享主射线和路径随机序列，独立标量路径与 XYZ 积分。

mod diagnostic;
mod lighting;
mod path;
mod state;
pub use diagnostic::{FixedWavelengthImage, render_fixed_wavelength_camera};
pub use path::{trace_spectral_camera_path, trace_spectral_camera_path_with_caustics};
pub use state::SpectralPathState;

use super::{
    direct::camera_lighting_setup,
    rng::{CAMERA_STREAM, DIFFUSE_STREAM, ENVIRONMENT_STREAM, pixel_seed},
};
use crate::{
    config::rendersetting::{RenderSetting, spectral_sample_counts},
    optics::{
        emission::{WAVELENGTH_MAX_NM, WAVELENGTH_MIN_NM, WAVELENGTH_PDF},
        iormode::IorMode,
        spectrum::cie_xyz,
    },
    render::{caustics::SpectralCausticsMap, framebuffer::Framebuffer},
    scene::Scene,
};
use nalgebra::{Vector2, Vector3};
use rand::{Rng, SeedableRng, rngs::StdRng};
use rayon::prelude::*;
use std::error::Error;

const WAVELENGTH_STREAM: u64 = 0xe703_7ed1_a0b4_28db;
const PREFIX_STREAM: u64 = 0x8ebc_6af0_9c88_c6e3;
const PACKET_SAMPLE_STREAM: u64 = 0x5899_65cc_7537_4cc3;
const PACKET_LANE_STREAM: u64 = 0x1d8e_4e27_c47d_124f;

/// 兼容入口：保持原有单波长采样与像素内随机流序列。
pub fn render_spectral_camera(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    ior_mode: IorMode,
) -> Result<Framebuffer, Box<dyn Error>> {
    render_spectral_camera_with_packet(scene, width, height, spp, 1, seed, ior_mode)
}

/// 低层无焦散入口；需要 Eλ 图集时使用 render_spectral_camera_components。
/// spp 为每像素总光谱路径数；Packet=4 时只生成 spp/4 条初始相机射线。
pub fn render_spectral_camera_with_packet(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    packet_size: usize,
    seed: u64,
    ior_mode: IorMode,
) -> Result<Framebuffer, Box<dyn Error>> {
    let settings = RenderSetting {
        width,
        height,
        camera_spp: spp,
        spectral_packet_size: packet_size,
        seed,
        mode: ior_mode,
        ..RenderSetting::default()
    };
    Ok(render_spectral_camera_buffers(scene, &settings, None, false)?.combined)
}

/// 一批 Hero/Packet 样本积分为 Direct、Caustics、Combined 的未曝光线性 XYZ。
/// 不在此处重复预计算；调用者可复用只读图集，None 完全保留原有路径和 RNG。
pub fn render_spectral_camera_components(
    scene: &Scene,
    settings: &RenderSetting,
    caustics: Option<&SpectralCausticsMap>,
) -> Result<super::CameraLightingImages, Box<dyn Error>> {
    let buffers = render_spectral_camera_buffers(scene, settings, caustics, true)?;
    let (direct, caustics) = buffers.split.expect("三分量入口分配独立图像");
    Ok(super::CameraLightingImages {
        direct,
        caustics,
        combined: buffers.combined,
        path_debug: None,
    })
}

struct SpectralCameraBuffers {
    combined: Framebuffer,
    split: Option<(Framebuffer, Framebuffer)>,
}

fn render_spectral_camera_buffers(
    scene: &Scene,
    settings: &RenderSetting,
    caustics: Option<&SpectralCausticsMap>,
    split: bool,
) -> Result<SpectralCameraBuffers, Box<dyn Error>> {
    let (width, height, spp, packet_size, seed, ior_mode) = (
        settings.width,
        settings.height,
        settings.camera_spp,
        settings.spectral_packet_size,
        settings.seed,
        settings.mode,
    );
    let (spatial_spp, total_paths) = spectral_sample_counts(spp, packet_size)?;
    validate_spectral_camera(scene, width, height, total_paths)?;
    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(size_of::<Vector3<f64>>() * if split { 3 } else { 1 }))
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or("光谱相机图像缓冲大小溢出")?;
    let mut buffers = SpectralCameraBuffers {
        combined: Framebuffer::new(width, height),
        split: split.then(|| {
            (
                Framebuffer::new(width, height),
                Framebuffer::new(width, height),
            )
        }),
    };
    let render_pixel = |index| -> Result<(Vector3<f64>, Vector3<f64>), String> {
        let mut jitter = StdRng::seed_from_u64(pixel_seed(seed, index, CAMERA_STREAM));
        let mut scattering = StdRng::seed_from_u64(pixel_seed(seed, index, DIFFUSE_STREAM));
        let mut prefix = StdRng::seed_from_u64(pixel_seed(seed, index, PREFIX_STREAM));
        let mut environment = StdRng::seed_from_u64(pixel_seed(seed, index, ENVIRONMENT_STREAM));
        let mut wavelength = StdRng::seed_from_u64(pixel_seed(seed, index, WAVELENGTH_STREAM));
        let mut mean = Vector3::zeros();
        let mut caustics_mean = Vector3::zeros();
        for i in 0..spatial_spp {
            if packet_size == 4 {
                // Camera RNG 按空间样本派生；每个 Packet 只抽一次偏移。
                jitter = packet_rng(seed, index, i, 0, CAMERA_STREAM);
            }
            let offset = Vector2::new(jitter.random::<f64>(), jitter.random::<f64>());
            let ray = scene
                .camera
                .ray_for_pixel_sample(index % width, index / width, width, height, offset)
                .ok_or("光谱相机子像素光线无效")?;
            // CRN：每个 Packet 只初始化一组路径流，各 Lane 从相同状态的副本开始。
            // 仍隔离前缀、首次环境与续追；Packet=1 不改变原有像素流。
            let common_path_rngs = (packet_size == 4).then(|| {
                [
                    packet_rng(seed, index, i, 0, PREFIX_STREAM),
                    packet_rng(seed, index, i, 0, ENVIRONMENT_STREAM),
                    packet_rng(seed, index, i, 0, DIFFUSE_STREAM),
                ]
            });
            for lane in 0..packet_size {
                if let Some([common_prefix, common_environment, common_scattering]) =
                    &common_path_rngs
                {
                    // 波长层内抖动独立；路径流共享随机值，不共享可变游标。
                    wavelength = packet_rng(seed, index, i, lane, WAVELENGTH_STREAM);
                    prefix.clone_from(common_prefix);
                    environment.clone_from(common_environment);
                    scattering.clone_from(common_scattering);
                }
                let u = wavelength.random::<f64>();
                let nm = if packet_size == 1 {
                    // 保留旧公式及浮点运算顺序，维持 Packet=1 的逐位结果。
                    WAVELENGTH_MIN_NM
                        + (WAVELENGTH_MAX_NM - WAVELENGTH_MIN_NM) * ((i as f64 + u) / spp as f64)
                } else {
                    packet_wavelength_nm(i, spatial_spp, lane, u)
                };
                let mut state = SpectralPathState::new(ray, nm, scene.max_diffuse_bounces)?;
                trace_spectral_camera_path_with_caustics(
                    scene,
                    &mut state,
                    ior_mode,
                    caustics,
                    &mut prefix,
                    &mut environment,
                    &mut scattering,
                )
                .map_err(|e| e.to_string())?;
                // 每个细分层恰取一个样本，权重为 400/总路径数。
                // 不额外乘 Δλ，也不再除以 Packet 大小。
                let cmf = cie_xyz(nm).ok_or("CIE 查询失败")?;
                mean += cmf * (state.radiance / (WAVELENGTH_PDF * total_paths as f64));
                if caustics.is_some() {
                    caustics_mean +=
                        cmf * (state.caustics_radiance / (WAVELENGTH_PDF * total_paths as f64));
                }
            }
        }
        if mean
            .iter()
            .chain(caustics_mean.iter())
            .any(|v| !v.is_finite() || *v < 0.0)
            || (mean + caustics_mean).iter().any(|v| !v.is_finite())
        {
            return Err("光谱 XYZ 积分溢出".into());
        }
        Ok((mean, caustics_mean))
    };
    let result: Result<(), String> = if let Some((direct, caustics)) = &mut buffers.split {
        buffers
            .combined
            .pixels
            .par_iter_mut()
            .zip(direct.pixels.par_iter_mut())
            .zip(caustics.pixels.par_iter_mut())
            .enumerate()
            .try_for_each(|(index, ((combined, direct), caustics))| {
                let (d, c) = render_pixel(index)?;
                *direct = d;
                *caustics = c;
                *combined = d + c;
                Ok(())
            })
    } else {
        buffers
            .combined
            .pixels
            .par_iter_mut()
            .enumerate()
            .try_for_each(|(index, pixel)| {
                let (d, c) = render_pixel(index)?;
                *pixel = d + c;
                Ok(())
            })
    };
    result.map_err(Box::<dyn Error>::from)?;
    Ok(buffers)
}

fn validate_spectral_camera(
    scene: &Scene,
    width: usize,
    height: usize,
    total_paths: usize,
) -> Result<(), Box<dyn Error>> {
    camera_lighting_setup(scene, width, height)?;
    scene
        .light
        .sunlight()
        .ok_or("光谱相机要求有效的向下平行太阳")?;
    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(total_paths))
        .and_then(|n| n.checked_mul(scene.environment_samples))
        .and_then(|n| n.checked_mul(scene.max_diffuse_bounces + 1))
        .ok_or("光谱相机工作量溢出")?;
    for object in &scene.objects {
        let material = object
            .material_id()
            .and_then(|id| scene.material(id))
            .ok_or("光谱场景存在未绑定材质的物体")?;
        if !material.supports_spectral() {
            return Err("光谱相机暂不支持 GGX Metal 或其他无光谱采样接口的材质".into());
        }
    }
    Ok(())
}

fn packet_rng(seed: u64, pixel: usize, sample: usize, lane: usize, stream: u64) -> StdRng {
    let pixel = pixel_seed(seed, pixel, stream);
    let sample = pixel_seed(pixel, sample, PACKET_SAMPLE_STREAM);
    StdRng::seed_from_u64(pixel_seed(sample, lane, PACKET_LANE_STREAM))
}

fn packet_wavelength_nm(sample: usize, spatial_spp: usize, lane: usize, u: f64) -> f64 {
    let band_width = (WAVELENGTH_MAX_NM - WAVELENGTH_MIN_NM) / 4.0;
    let band_min = WAVELENGTH_MIN_NM + band_width * lane as f64;
    let lower = band_min + band_width * (sample as f64 / spatial_spp as f64);
    let upper = band_min + band_width * ((sample + 1) as f64 / spatial_spp as f64);
    let nm = WAVELENGTH_MIN_NM
        + (WAVELENGTH_MAX_NM - WAVELENGTH_MIN_NM)
            * (lane as f64 + (sample as f64 + u) / spatial_spp as f64)
            / 4.0;
    // u<1 仍可能经舍入落在右端点；保持各细分层的左闭右开边界。
    nm.clamp(lower, upper.next_down().max(lower))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;
    use std::collections::HashSet;

    #[test]
    fn packet_wavelengths_keep_fine_strata_and_open_right_edges_even_after_rounding() {
        for total in [4, 16, 512] {
            let spatial = total / 4;
            for u in [0.0, 0.37, 1.0_f64.next_down()] {
                let mut visits = vec![0; total];
                for i in 0..spatial {
                    for lane in 0..4 {
                        let nm = packet_wavelength_nm(i, spatial, lane, u);
                        let stratum = lane * spatial + i;
                        let lower = 380.0 + 400.0 * stratum as f64 / total as f64;
                        let upper = 380.0 + 400.0 * (stratum + 1) as f64 / total as f64;
                        assert!(nm >= lower && nm < upper, "{total}, {i}, {lane}, {u}: {nm}");
                        // 用层边界归类，避免逆变换的舍入把 next_down(upper) 推回整数。
                        let found = (0..total)
                            .find(|&s| {
                                let a = 380.0 + 400.0 * s as f64 / total as f64;
                                let b = 380.0 + 400.0 * (s + 1) as f64 / total as f64;
                                nm >= a && nm < b
                            })
                            .unwrap();
                        visits[found] += 1;
                    }
                }
                assert!(visits.iter().all(|&n| n == 1));
            }
        }
    }

    #[test]
    fn packet_rngs_separate_all_coordinates_and_streams_without_consumption_coupling() {
        let mut values = HashSet::new();
        for seed in [42, 43] {
            for pixel in [0, 1] {
                for sample in [0, 1, 127] {
                    for lane in 0..4 {
                        for stream in [
                            WAVELENGTH_STREAM,
                            CAMERA_STREAM,
                            PREFIX_STREAM,
                            ENVIRONMENT_STREAM,
                            DIFFUSE_STREAM,
                        ] {
                            let mut a = packet_rng(seed, pixel, sample, lane, stream);
                            let mut b = packet_rng(seed, pixel, sample, lane, stream);
                            let first = a.next_u64();
                            assert_eq!(first, b.next_u64());
                            assert!(values.insert(first));
                            // 一个 Lane 的深路径不能消费另一个 Lane 的状态。
                            let mut other = packet_rng(seed, pixel, sample, (lane + 1) % 4, stream);
                            for _ in 0..97 {
                                other.next_u64();
                            }
                            assert_eq!(a.next_u64(), b.next_u64());
                        }
                    }
                }
            }
        }
    }
}

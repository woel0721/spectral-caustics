//! 单波长/四波长 Packet 相机：共享主射线，独立标量路径，线性 XYZ Monte Carlo 积分。

mod lighting;
mod path;
mod state;
pub use path::trace_spectral_camera_path;
pub use state::SpectralPathState;

use super::{
    direct::camera_lighting_setup,
    rng::{CAMERA_STREAM, DIFFUSE_STREAM, ENVIRONMENT_STREAM, pixel_seed},
};
use crate::{
    config::rendersetting::spectral_sample_counts,
    optics::{
        emission::{WAVELENGTH_MAX_NM, WAVELENGTH_MIN_NM, WAVELENGTH_PDF},
        iormode::IorMode,
        spectrum::cie_xyz,
    },
    render::framebuffer::Framebuffer,
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
    let (spatial_spp, total_paths) = spectral_sample_counts(spp, packet_size)?;
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
    let mut image = Framebuffer::new(width, height);
    let result: Result<(), String> =
        image
            .pixels
            .par_iter_mut()
            .enumerate()
            .try_for_each(|(index, pixel)| {
                let mut jitter = StdRng::seed_from_u64(pixel_seed(seed, index, CAMERA_STREAM));
                let mut scattering = StdRng::seed_from_u64(pixel_seed(seed, index, DIFFUSE_STREAM));
                let mut prefix = StdRng::seed_from_u64(pixel_seed(seed, index, PREFIX_STREAM));
                let mut environment =
                    StdRng::seed_from_u64(pixel_seed(seed, index, ENVIRONMENT_STREAM));
                let mut wavelength =
                    StdRng::seed_from_u64(pixel_seed(seed, index, WAVELENGTH_STREAM));
                let mut mean = Vector3::zeros();
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
                    for lane in 0..packet_size {
                        if packet_size == 4 {
                            wavelength = packet_rng(seed, index, i, lane, WAVELENGTH_STREAM);
                            prefix = packet_rng(seed, index, i, lane, PREFIX_STREAM);
                            environment = packet_rng(seed, index, i, lane, ENVIRONMENT_STREAM);
                            scattering = packet_rng(seed, index, i, lane, DIFFUSE_STREAM);
                        }
                        let u = wavelength.random::<f64>();
                        let nm = if packet_size == 1 {
                            // 保留旧公式及浮点运算顺序，维持 Packet=1 的逐位结果。
                            WAVELENGTH_MIN_NM
                                + (WAVELENGTH_MAX_NM - WAVELENGTH_MIN_NM)
                                    * ((i as f64 + u) / spp as f64)
                        } else {
                            packet_wavelength_nm(i, spatial_spp, lane, u)
                        };
                        let mut state = SpectralPathState::new(ray, nm, scene.max_diffuse_bounces)?;
                        trace_spectral_camera_path(
                            scene,
                            &mut state,
                            ior_mode,
                            &mut prefix,
                            &mut environment,
                            &mut scattering,
                        )
                        .map_err(|e| e.to_string())?;
                        // 每个细分层恰取一个样本，权重为 400/总路径数。
                        // 不额外乘 Δλ，也不再除以 Packet 大小。
                        mean += cie_xyz(nm).ok_or("CIE 查询失败")?
                            * (state.radiance / (WAVELENGTH_PDF * total_paths as f64));
                    }
                }
                if mean.iter().any(|v| !v.is_finite() || *v < 0.0) {
                    return Err("光谱 XYZ 积分溢出".into());
                }
                *pixel = mean;
                Ok(())
            });
    result.map_err(Box::<dyn Error>::from)?;
    Ok(image)
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

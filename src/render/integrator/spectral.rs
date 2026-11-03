//! Hero wavelength 相机：一个样本、一条固定波长的标量路径，一次 XYZ Monte Carlo 权重。

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

pub fn render_spectral_camera(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    ior_mode: IorMode,
) -> Result<Framebuffer, Box<dyn Error>> {
    camera_lighting_setup(scene, width, height)?;
    scene
        .light
        .sunlight()
        .ok_or("光谱相机要求有效的向下平行太阳")?;
    if spp == 0 {
        return Err("光谱相机 SPP 必须为正".into());
    }
    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(spp))
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
                for _ in 0..spp {
                    let nm = WAVELENGTH_MIN_NM
                        + (WAVELENGTH_MAX_NM - WAVELENGTH_MIN_NM) * wavelength.random::<f64>();
                    let offset = Vector2::new(jitter.random::<f64>(), jitter.random::<f64>());
                    let ray = scene
                        .camera
                        .ray_for_pixel_sample(index % width, index / width, width, height, offset)
                        .ok_or("光谱相机子像素光线无效")?;
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
                    // Lλ 的单位为每 nm。仅在此乘 1/pλ=400；绝不再乘离散 Δλ。
                    mean += cie_xyz(nm).ok_or("CIE 查询失败")?
                        * (state.radiance / (WAVELENGTH_PDF * spp as f64));
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

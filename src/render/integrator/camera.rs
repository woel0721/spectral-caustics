//! 相机渲染入口、像素内 SPP 累积与 Rayon 像素并行。

use super::{
    direct::camera_lighting_setup,
    ground::{GroundLighting, bind_spectral_ground_lighting},
    path::trace_camera_path,
    rng::{CAMERA_STREAM, DIFFUSE_STREAM, ENVIRONMENT_STREAM, pixel_seed},
    types::{CameraLightingImages, CameraPathComponents, CameraPathRngs, CameraPathState},
};
use crate::{
    render::{caustics::SpectralLighting, framebuffer::Framebuffer, utils::linear_srgb_to_xyz},
    scene::Scene,
};
use nalgebra::{Vector2, Vector3};
use rand::{Rng, SeedableRng, rngs::StdRng};
use rayon::prelude::*;
use std::{error::Error, mem::size_of};

/// primary 始终保存 Combined；独立 RGB 相机只分配它，光谱三图入口另分配 split。
struct CameraBuffers {
    primary: Framebuffer,
    split: Option<(Framebuffer, Framebuffer)>,
}

/// 每像素独立追踪 spp 条路径，在线性 RGB 中平均后保存为线性 XYZ。
///
/// spp 必须大于零，表示每像素路径总数；每个样本在像素内均匀选取
/// 两个 [0, 1) 偏移，随后独立追踪玻璃路径；spp=1 也包含子像素抖动。
/// seed 为每个行优先像素派生独立 RNG；像素并行，像素内按样本（偏移、路径）顺序采样。
/// 路径最多经过八次玻璃散射，其余光照和截断规则见 trace_camera_path。
/// 返回未经曝光的线性 XYZ，交给 Render::save 统一曝光和 sRGB 编码。
pub fn render_camera_glass(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
) -> Result<Framebuffer, Box<dyn Error>> {
    Ok(render_camera_buffers(scene, width, height, spp, seed, None, false)?.primary)
}

/// 一次遍历像素和 SPP，同时估计 Direct 和 Caustics，均值相加得到 Combined。
/// Direct 包含环境背景、环境照明及解析太阳；Caustics 只包含焦散贴图的地面反射。
/// 三个输出共享当前路径和独立 RNG 流，不为选择输出分量重复追踪。
pub fn render_camera_glass_components(
    scene: &Scene,
    lighting: &SpectralLighting,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
) -> Result<CameraLightingImages, Box<dyn Error>> {
    let ground = bind_spectral_ground_lighting(scene, lighting)?;
    let buffers = render_camera_buffers(scene, width, height, spp, seed, Some(&ground), true)?;
    let (direct, caustics) = buffers.split.expect("三图入口必须分配分量缓冲");
    Ok(CameraLightingImages {
        direct,
        caustics,
        combined: buffers.primary,
    })
}

fn render_camera_buffers(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    ground: Option<&GroundLighting<'_>>,
    split: bool,
) -> Result<CameraBuffers, Box<dyn Error>> {
    if spp == 0 {
        return Err("相机 SPP 必须大于零".into());
    }
    let (light_wi, light) = camera_lighting_setup(scene, width, height)?;
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(spp))
        .and_then(|paths| paths.checked_mul(scene.environment_samples))
        .and_then(|samples| {
            samples.checked_mul(if scene.max_diffuse_bounces > 1 {
                scene.max_diffuse_bounces + 1
            } else {
                1
            })
        })
        .ok_or("相机路径样本总数溢出")?;
    if split {
        width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(3 * size_of::<Vector3<f64>>()))
            .filter(|&bytes| bytes <= isize::MAX as usize)
            .ok_or("三张相机图的像素缓冲总大小溢出")?;
    }
    let mut buffers = CameraBuffers {
        primary: Framebuffer::new(width, height),
        split: split.then(|| {
            (
                Framebuffer::new(width, height),
                Framebuffer::new(width, height),
            )
        }),
    };
    let render_pixel = |pixel_index: usize| -> Result<CameraPathComponents, String> {
        let x = pixel_index % width;
        let y = pixel_index / width;
        let mut rng = StdRng::seed_from_u64(pixel_seed(seed, pixel_index, CAMERA_STREAM));
        let mut environment_rng =
            StdRng::seed_from_u64(pixel_seed(seed, pixel_index, ENVIRONMENT_STREAM));
        let mut diffuse_rng = StdRng::seed_from_u64(pixel_seed(seed, pixel_index, DIFFUSE_STREAM));
        let mut mean = CameraPathComponents::default();
        // 像素内顺序累积，不做跨线程浮点归约，也不因输出分量重复追踪路径。
        for _ in 0..spp {
            let offset = Vector2::new(rng.random::<f64>(), rng.random::<f64>());
            let ray = scene
                .camera
                .ray_for_pixel_sample(x, y, width, height, offset)
                .ok_or("无法生成子像素摄像机光线")?;
            // 先缩放每项再相加，避免有限样本之和溢出而平均值仍有限。
            let mut state = CameraPathState::new(ray, scene.max_diffuse_bounces);
            trace_camera_path(
                scene,
                &mut state,
                &mut CameraPathRngs {
                    camera: &mut rng,
                    environment: &mut environment_rng,
                    diffuse: &mut diffuse_rng,
                },
                light_wi,
                light,
                ground,
            )
            .map_err(|error| error.to_string())?;
            mean.direct += state.radiance.direct / spp as f64;
            mean.caustics += state.radiance.caustics / spp as f64;
        }
        Ok(mean)
    };
    // 每个任务独占对应像素的可变引用；单图入口不分配 split 缓冲。
    let result: Result<(), String> = match &mut buffers.split {
        Some((direct, caustics)) => buffers
            .primary
            .pixels
            .par_iter_mut()
            .zip(direct.pixels.par_iter_mut())
            .zip(caustics.pixels.par_iter_mut())
            .enumerate()
            .try_for_each(|(index, ((primary, direct), caustics))| {
                let mean = render_pixel(index)?;
                set_camera_pixel(primary, mean.direct + mean.caustics)?;
                set_camera_pixel(direct, mean.direct)?;
                set_camera_pixel(caustics, mean.caustics)
            }),
        None => buffers
            .primary
            .pixels
            .par_iter_mut()
            .enumerate()
            .try_for_each(|(index, primary)| {
                let mean = render_pixel(index)?;
                set_camera_pixel(primary, mean.direct + mean.caustics)
            }),
    };
    // Rayon 只传递 Send 的 String，离开并行迭代后恢复原公共错误接口。
    result.map_err(Box::<dyn Error>::from)?;
    Ok(buffers)
}

fn set_camera_pixel(pixel: &mut Vector3<f64>, rgb: Vector3<f64>) -> Result<(), String> {
    let xyz = linear_srgb_to_xyz(rgb).ok_or("相机样本平均颜色非法或计算溢出")?;
    *pixel = xyz;
    Ok(())
}

//! 相机渲染入口、像素内 SPP 累积与 Rayon 像素并行。

use super::{
    debug::{PathDebugBuffer, PathDebugPixel, PathDebugSample},
    direct::camera_lighting_setup,
    ground::{GroundLighting, bind_spectral_ground_lighting},
    path::{trace_camera_path, trace_camera_path_impl},
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
    debug: Option<PathDebugBuffer>,
}

enum CameraOutput {
    Rgb,
    Components { debug_path: bool },
}

/// 每像素独立追踪 spp 条路径，在线性 RGB 中平均后保存为线性 XYZ。
///
/// spp 必须大于零，表示每像素路径总数；每个样本在像素内均匀选取
/// 两个 [0, 1) 偏移，随后独立追踪玻璃路径；spp=1 也包含子像素抖动。
/// seed 为每个行优先像素派生独立 RNG；像素并行，像素内按样本（偏移、路径）顺序采样。
/// Delta 事件预算及其余光照/截断规则见 trace_camera_path。
/// 返回未经曝光的线性 XYZ，交给 Render::save 统一曝光和 sRGB 编码。
pub fn render_camera_glass(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
) -> Result<Framebuffer, Box<dyn Error>> {
    Ok(render_camera_buffers(scene, width, height, spp, seed, None, CameraOutput::Rgb)?.primary)
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
    render_camera_glass_components_impl(scene, lighting, width, height, spp, seed, false)
}

pub(crate) fn render_camera_glass_components_impl(
    scene: &Scene,
    lighting: &SpectralLighting,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    debug_path: bool,
) -> Result<CameraLightingImages, Box<dyn Error>> {
    let ground = bind_spectral_ground_lighting(scene, lighting)?;
    let buffers = render_camera_buffers(
        scene,
        width,
        height,
        spp,
        seed,
        Some(&ground),
        CameraOutput::Components { debug_path },
    )?;
    let (direct, caustics) = buffers.split.expect("三图入口必须分配分量缓冲");
    Ok(CameraLightingImages {
        direct,
        caustics,
        combined: buffers.primary,
        path_debug: buffers.debug,
    })
}

fn render_camera_buffers(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    ground: Option<&GroundLighting<'_>>,
    output: CameraOutput,
) -> Result<CameraBuffers, Box<dyn Error>> {
    if spp == 0 {
        return Err("相机 SPP 必须大于零".into());
    }
    let (light_wi, light) = camera_lighting_setup(scene, width, height)?;
    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(spp))
        .and_then(|n| n.checked_mul(scene.light_samples))
        .and_then(|n| n.checked_mul(scene.max_diffuse_bounces + 1))
        .ok_or("有限光源样本总数溢出")?;
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
    let split = matches!(output, CameraOutput::Components { .. });
    // release 构建忽略配置开关，不分配统计缓冲，也不记录真实路径事件。
    let debug_path =
        cfg!(debug_assertions) && matches!(output, CameraOutput::Components { debug_path: true });
    if debug_path
        && scene
            .debug_object_id
            .is_some_and(|id| id >= scene.objects.len())
    {
        return Err("调试物体编号不在场景中".into());
    }
    if split {
        width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(3 * size_of::<Vector3<f64>>()))
            .filter(|&bytes| bytes <= isize::MAX as usize)
            .ok_or("三张相机图的像素缓冲总大小溢出")?;
    }
    if debug_path {
        width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(size_of::<PathDebugPixel>()))
            .filter(|&bytes| bytes <= isize::MAX as usize)
            .ok_or("路径调试缓冲总大小溢出")?;
    }
    let mut buffers = CameraBuffers {
        primary: Framebuffer::new(width, height),
        split: split.then(|| {
            (
                Framebuffer::new(width, height),
                Framebuffer::new(width, height),
            )
        }),
        debug: debug_path.then(|| PathDebugBuffer::new(width, height, scene.debug_object_id)),
    };
    let render_pixel =
        |pixel_index: usize| -> Result<(CameraPathComponents, Option<PathDebugPixel>), String> {
            let x = pixel_index % width;
            let y = pixel_index / width;
            let mut rng = StdRng::seed_from_u64(pixel_seed(seed, pixel_index, CAMERA_STREAM));
            let mut environment_rng =
                StdRng::seed_from_u64(pixel_seed(seed, pixel_index, ENVIRONMENT_STREAM));
            let mut diffuse_rng =
                StdRng::seed_from_u64(pixel_seed(seed, pixel_index, DIFFUSE_STREAM));
            let mut mean = CameraPathComponents::default();
            let mut debug_pixel = debug_path.then(PathDebugPixel::default);
            // 像素内顺序累积，不做跨线程浮点归约，也不因输出分量重复追踪路径。
            for _ in 0..spp {
                let offset = Vector2::new(rng.random::<f64>(), rng.random::<f64>());
                let ray = scene
                    .camera
                    .ray_for_pixel_sample(x, y, width, height, offset)
                    .ok_or("无法生成子像素摄像机光线")?;
                // 先缩放每项再相加，避免有限样本之和溢出而平均值仍有限。
                let mut state = CameraPathState::new(ray, scene.max_diffuse_bounces);
                let mut rngs = CameraPathRngs {
                    camera: &mut rng,
                    environment: &mut environment_rng,
                    diffuse: &mut diffuse_rng,
                };
                if let Some(pixel) = &mut debug_pixel {
                    let mut sample = PathDebugSample::default();
                    trace_camera_path_impl(
                        scene,
                        &mut state,
                        &mut rngs,
                        light_wi,
                        light,
                        ground,
                        Some(&mut sample),
                    )
                    .map_err(|error| error.to_string())?;
                    pixel.record(&sample, scene.debug_object_id)?;
                } else {
                    trace_camera_path(scene, &mut state, &mut rngs, light_wi, light, ground)
                        .map_err(|error| error.to_string())?;
                }
                mean.direct += state.radiance.direct / spp as f64;
                mean.caustics += state.radiance.caustics / spp as f64;
            }
            Ok((mean, debug_pixel))
        };
    // 每个任务独占对应像素的可变引用；单图入口不分配 split 缓冲。
    let result: Result<(), String> = match (&mut buffers.split, &mut buffers.debug) {
        (Some((direct, caustics)), Some(debug)) => buffers
            .primary
            .pixels
            .par_iter_mut()
            .zip(direct.pixels.par_iter_mut())
            .zip(caustics.pixels.par_iter_mut())
            .zip(debug.pixels.par_iter_mut())
            .enumerate()
            .try_for_each(|(index, (((primary, direct), caustics), statistics))| {
                let (mean, pixel_debug) = render_pixel(index)?;
                *statistics = pixel_debug.expect("调试入口必须记录像素统计");
                set_camera_pixel(primary, mean.direct + mean.caustics)?;
                set_camera_pixel(direct, mean.direct)?;
                set_camera_pixel(caustics, mean.caustics)
            }),
        (Some((direct, caustics)), None) => buffers
            .primary
            .pixels
            .par_iter_mut()
            .zip(direct.pixels.par_iter_mut())
            .zip(caustics.pixels.par_iter_mut())
            .enumerate()
            .try_for_each(|(index, ((primary, direct), caustics))| {
                let (mean, _) = render_pixel(index)?;
                set_camera_pixel(primary, mean.direct + mean.caustics)?;
                set_camera_pixel(direct, mean.direct)?;
                set_camera_pixel(caustics, mean.caustics)
            }),
        (None, _) => buffers
            .primary
            .pixels
            .par_iter_mut()
            .enumerate()
            .try_for_each(|(index, primary)| {
                let (mean, _) = render_pixel(index)?;
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

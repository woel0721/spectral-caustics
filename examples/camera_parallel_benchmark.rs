//! release 下比较同一 HDRI/场景/种子的 1/2/4/6 线程，逐位检查三个 XYZ 缓冲。
//! 不计场景/HDR 加载、线程池创建和 PNG 编码；total 包含光源与三图相机渲染。

use spectral_caustics::{
    config::Config,
    render::{CameraLightingImages, Render, framebuffer::Framebuffer},
    scene::Scene,
};
use std::{error::Error, time::Instant};

fn identical(actual: &Framebuffer, expected: &Framebuffer) -> bool {
    actual.width == expected.width
        && actual.height == expected.height
        && actual.pixels.len() == expected.pixels.len()
        && actual.pixels.iter().zip(&expected.pixels).all(|(a, b)| {
            a.iter()
                .zip(b.iter())
                .all(|(a, b)| a.to_bits() == b.to_bits())
        })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut config = Config::load("configs/default.json")?;
    let mut arguments = std::env::args().skip(1);
    let spp = arguments
        .next()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(config.render.camera_spp);
    let size = arguments
        .next()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(config.render.width);
    if spp == 0 || size == 0 || arguments.next().is_some() {
        return Err("用法：camera_parallel_benchmark [正整数 SPP] [正整数边长]".into());
    }
    config.render.width = size;
    config.render.height = size;
    let scene = Scene::from_config(&config.scene)?;
    println!(
        "{size}x{size}, spp={spp}, seed={}, environment_samples={}, HDRI={:?}",
        config.render.seed,
        config.scene.environment_samples,
        config
            .scene
            .environment
            .as_ref()
            .and_then(|e| e.map_path.as_ref()),
    );
    let mut reference: Option<CameraLightingImages> = None;
    let mut reference_photons = None;
    let mut baseline_camera = 0.0;
    let mut baseline_total = 0.0;
    for threads in [1, 2, 4, 6] {
        config.render.thread_count = Some(threads);
        let renderer = Render::new(config.render.clone())?;
        let started = Instant::now();
        let lighting = renderer.render_lighting(&scene)?;
        let lighting_seconds = started.elapsed().as_secs_f64();
        let camera_started = Instant::now();
        let images =
            renderer.render_camera_glass_components(&scene, &lighting, spp, config.render.seed)?;
        let camera_seconds = camera_started.elapsed().as_secs_f64();
        let total_seconds = started.elapsed().as_secs_f64();
        if let Some(expected) = &reference {
            for (actual, expected) in [
                (&images.direct, &expected.direct),
                (&images.caustics, &expected.caustics),
                (&images.combined, &expected.combined),
            ] {
                if !identical(actual, expected) {
                    return Err(format!("{threads} 线程的相机 XYZ 与单线程不一致").into());
                }
            }
        } else {
            baseline_camera = camera_seconds;
            baseline_total = total_seconds;
            reference = Some(images);
        }
        if let Some((direct, caustics)) = &reference_photons {
            if !identical(&lighting.direct, direct) || !identical(&lighting.caustics, caustics) {
                return Err(format!("{threads} 线程的光源 XYZ 与单线程不一致").into());
            }
        } else {
            reference_photons = Some((lighting.direct, lighting.caustics));
        }
        println!(
            "threads={threads}, lighting={lighting_seconds:.6}s, camera={camera_seconds:.6}s, \
             camera_speedup={:.3}x, total={total_seconds:.6}s, total_speedup={:.3}x, XYZ=bit-identical",
            baseline_camera / camera_seconds,
            baseline_total / total_seconds,
        );
    }
    Ok(())
}

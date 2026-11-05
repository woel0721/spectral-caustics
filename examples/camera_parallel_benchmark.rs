//! release 下比较同一 HDRI/场景/种子的 1/2/4/6 线程，逐位检查三个 XYZ 缓冲。
//! 不计场景/HDR 加载、线程池创建和 PNG 编码；total 包含光源与三图相机渲染。

use spectral_caustics::config::Config;
use spectral_caustics::film::Framebuffer;
use spectral_caustics::integrator::camera::CameraLightingImages;
use spectral_caustics::renderer::Renderer;
use std::error::Error;
use std::time::Instant;

fn identical(actual: &Framebuffer, expected: &Framebuffer) -> bool {
    actual.width() == expected.width()
        && actual.height() == expected.height()
        && actual.pixels().len() == expected.pixels().len()
        && actual.pixels().iter().zip(expected.pixels()).all(|(a, b)| {
            a.iter()
                .zip(b.iter())
                .all(|(a, b)| a.to_bits() == b.to_bits())
        })
}

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
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
    let scene = config.build()?;
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
    config.render.camera_spp = spp;
    let mut reference: Option<CameraLightingImages> = None;
    let mut baseline = 0.0;
    for threads in [1, 2, 4, 6] {
        config.render.thread_count = Some(threads);
        let renderer = Renderer::new(config.render.clone())?;
        let started = Instant::now();
        let images = renderer.render(&scene)?;
        let seconds = started.elapsed().as_secs_f64();
        if let Some(expected) = &reference {
            for (actual, expected) in [
                (&images.direct, &expected.direct),
                (&images.caustics, &expected.caustics),
                (&images.combined, &expected.combined),
            ] {
                if !identical(actual, expected) {
                    return Err(format!("{threads} threads changed XYZ").into());
                }
            }
        } else {
            baseline = seconds;
            reference = Some(images);
        }
        println!(
            "threads={threads}, total={seconds:.6}s, speedup={:.3}x, XYZ=bit-identical",
            baseline / seconds
        );
    }
    Ok(())
}

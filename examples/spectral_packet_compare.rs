//! 公平总路径预算下的 Packet 对比；所有统计均来自曝光/ACES 前的线性 XYZ。
use nalgebra::Vector3;
use serde_json::Value;
use serde_json::json;
use spectral_caustics::config::Config;
use spectral_caustics::config::RenderJob;
use spectral_caustics::film::Framebuffer;
use spectral_caustics::optics::dispersion::IorMode;
use spectral_caustics::renderer::Renderer;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

fn render_saved(
    config: &mut Config,
    scene: &RenderJob,
    directory: &Path,
    name: &str,
) -> Result<(Framebuffer, f64), Box<dyn Error + Send + Sync>> {
    config.render.output_path = directory.join(format!("{name}.png"));
    config.save(directory.join(format!("{name}.json")))?;
    let renderer = Renderer::new(config.render.clone())?;
    let start = Instant::now();
    let image = renderer.render(scene).map(|images| images.combined)?;
    let seconds = start.elapsed().as_secs_f64();
    spectral_caustics::output::save_png_with_exposure(
        &image,
        &config.render.output_path,
        (renderer).settings().exposure,
    )?;
    // 无头部，行优先，每像素依次 X/Y/Z，IEEE f64 little-endian；尺寸在同名 JSON 中。
    let mut raw =
        std::io::BufWriter::new(fs::File::create(directory.join(format!("{name}.xyz64")))?);
    for pixel in image.pixels() {
        for value in pixel.iter() {
            raw.write_all(&value.to_le_bytes())?;
        }
    }
    raw.flush()?;
    println!(
        "{name}: {}x{}, {} paths/pixel, packet={}, {seconds:.3}s",
        image.width(),
        image.height(),
        config.render.camera_spp,
        config.render.spectral_packet_size
    );
    Ok((image, seconds))
}

fn chromaticity(p: Vector3<f64>) -> Vector3<f64> {
    if p.sum() > 1e-15 {
        Vector3::new(p.x / p.sum(), p.y / p.sum(), 0.0)
    } else {
        Vector3::zeros()
    }
}

fn metrics(images: &[Framebuffer], reference: &Framebuffer, mask: &[bool]) -> Value {
    let n = images.len() as f64;
    let mut variance = Vector3::zeros();
    let mut xy_variance = 0.0;
    let mut mse = 0.0;
    let mut mean_mse = 0.0;
    let mut mean_xyz = Vector3::zeros();
    let mut pixels = 0;
    for (i, &include) in mask.iter().enumerate() {
        if !include {
            continue;
        }
        pixels += 1;
        let mean = images.iter().map(|im| im.pixels()[i]).sum::<Vector3<f64>>() / n;
        let xy_mean = images
            .iter()
            .map(|im| chromaticity(im.pixels()[i]))
            .sum::<Vector3<f64>>()
            / n;
        mean_xyz += mean;
        mean_mse += (mean - reference.pixels()[i]).norm_squared() / 3.0;
        for image in images {
            let d = image.pixels()[i] - mean;
            variance += d.component_mul(&d) / (n - 1.0);
            xy_variance += (chromaticity(image.pixels()[i]) - xy_mean).norm_squared() / (n - 1.0);
            mse += (image.pixels()[i] - reference.pixels()[i]).norm_squared() / (3.0 * n);
        }
    }
    let count = pixels as f64;
    json!({
        "pixels": pixels,
        "mean_xyz": [mean_xyz.x / count, mean_xyz.y / count, mean_xyz.z / count],
        "xyz_sample_variance": [variance.x / count, variance.y / count, variance.z / count],
        "xy_sample_variance_sum": xy_variance / count,
        "mean_single_image_xyz_mse": mse / count,
        "seed_mean_image_xyz_mse": mean_mse / count,
    })
}

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut config = Config::load("configs/sphere_spectral.json")?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() > 5 {
        return Err(
            "用法：spectral_packet_compare [总SPP] [边长] [输出目录] [种子数>=2] [参考SPP]".into(),
        );
    }
    let parse = |i: usize, default: usize| -> Result<usize, Box<dyn Error + Send + Sync>> {
        Ok(args
            .get(i)
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(default))
    };
    let spp = parse(0, 512)?;
    let size = parse(1, 256)?;
    let directory = std::path::PathBuf::from(
        args.get(2)
            .map(String::as_str)
            .unwrap_or("output/spectral_packet"),
    );
    let seed_count = parse(3, 3)?;
    let reference_spp = parse(4, 8192)?;
    if seed_count < 2 || reference_spp <= spp {
        return Err("至少两个种子，且参考 SPP 必须大于测试 SPP".into());
    }
    config.render.width = size;
    config.render.height = size;
    config.render.camera_spp = spp;
    config.render.spectral_packet_size = 4;
    config.validate()?;
    let scene = config.build()?;
    let threads = Renderer::new(config.render.clone())?.thread_count();
    fs::create_dir_all(&directory)?;
    let seeds: Vec<u64> = (0..seed_count)
        .map(|i| 42_u64.checked_add(i as u64).ok_or("种子溢出"))
        .collect::<Result<_, _>>()?;
    let sphere: Vec<_> = (0..size * size)
        .map(|i| {
            let ray = scene
                .camera
                .ray_for_pixel(i % size, i / size, size, size)
                .unwrap();
            scene
                .scene
                .intersect(&ray)
                .is_some_and(|hit| Some(hit.object_id) == scene.photon.caustics_object_id)
        })
        .collect();
    // 用四邻域中心射线定义球轮廓边缘；球内部不含这些像素。
    let edge: Vec<_> = (0..size * size)
        .map(|i| {
            let x = i % size;
            let y = i / size;
            [
                (x.saturating_sub(1), y),
                ((x + 1).min(size - 1), y),
                (x, y.saturating_sub(1)),
                (x, (y + 1).min(size - 1)),
            ]
            .iter()
            .any(|&(x, y)| sphere[y * size + x] != sphere[i])
        })
        .collect();
    let interior: Vec<_> = sphere.iter().zip(&edge).map(|(&s, &e)| s && !e).collect();
    let all = vec![true; size * size];
    let mut cases = Vec::new();
    for (name, mode) in [
        ("constant", IorMode::Constant),
        ("dispersion", IorMode::Dispersion),
    ] {
        config.render.mode = mode;
        config.render.spectral_packet_size = 1;
        config.render.camera_spp = reference_spp;
        config.render.seed = 20261104;
        let (reference, reference_seconds) = render_saved(
            &mut config,
            &scene,
            &directory,
            &format!("{name}_reference"),
        )?;
        let mut images = [Vec::new(), Vec::new()];
        let mut seconds = [Vec::new(), Vec::new()];
        for (i, &seed) in seeds.iter().enumerate() {
            // 交替执行顺序，减小持续负载与温度变化带来的顺序偏差。
            for p in if i % 2 == 0 { [0, 1] } else { [1, 0] } {
                let packet = [1, 4][p];
                config.render.camera_spp = spp;
                config.render.spectral_packet_size = packet;
                config.render.seed = seed;
                let (image, elapsed) = render_saved(
                    &mut config,
                    &scene,
                    &directory,
                    &format!("{name}_packet{packet}_seed{seed}"),
                )?;
                images[p].push(image);
                seconds[p].push(elapsed);
            }
        }
        for p in 0..2 {
            let mut average = Framebuffer::new(size, size)?;
            for image in &images[p] {
                for (dst, src) in average.pixels_mut().iter_mut().zip(image.pixels()) {
                    *dst += src / seed_count as f64;
                }
            }
            spectral_caustics::output::save_png_with_exposure(
                &average,
                directory.join(format!("{name}_packet{}_mean.png", [1, 4][p])),
                config.render.exposure,
            )?;
            cases.push(json!({
                "ior_mode": name, "packet_size": ([1, 4][p]),
                "render_seconds": seconds[p],
                "mean_render_seconds": seconds[p].iter().sum::<f64>() / seed_count as f64,
                "reference_seconds": reference_seconds,
                "all": metrics(&images[p], &reference, &all),
                "sphere_interior": metrics(&images[p], &reference, &interior),
                "sphere_edge": metrics(&images[p], &reference, &edge),
            }));
        }
    }
    let report = json!({
        "width": size, "height": size, "total_paths_per_pixel": spp,
        "seeds": seeds, "thread_count": threads,
        "reference": {"spp": reference_spp, "packet_size": 1, "seed": 20261104},
        "timing": "camera rendering only; excludes scene/HDR loading, pool creation and file encoding",
        "metrics": "linear XYZ before exposure/ACES; unbiased across-seed pixel variance; xy variance is Var(X/sum)+Var(Y/sum); XYZ MSE averages three channels. Finite-SPP reference contains noise.",
        "xyz64": "headerless row-major little-endian f64 X,Y,Z; dimensions in same-stem JSON config",
        "cases": cases,
    });
    fs::write(
        directory.join("metrics.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

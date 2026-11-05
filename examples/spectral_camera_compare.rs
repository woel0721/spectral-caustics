//! 同一几何、seed、波长/像素采样配置，对比光谱相机的 Constant IOR 与 Dispersion。

use spectral_caustics::config::Config;
use spectral_caustics::config::render::CameraMode;
use spectral_caustics::optics::dispersion::IorMode;
use spectral_caustics::renderer::Renderer;
use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() > 4 {
        return Err(
            "用法：spectral_camera_compare [SPP] [正方形边长] [输出目录] [配置路径]".into(),
        );
    }
    let mut config = Config::load(
        args.get(3)
            .map(String::as_str)
            .unwrap_or("configs/sphere_spectral.json"),
    )?;
    if let Some(spp) = args.first() {
        config.render.camera_spp = spp.parse()?;
    }
    if let Some(size) = args.get(1) {
        config.render.width = size.parse()?;
        config.render.height = config.render.width;
    }
    let directory = args.get(2).map(PathBuf::from).unwrap_or_else(|| {
        config
            .render
            .output_path
            .parent()
            .unwrap_or(std::path::Path::new("output"))
            .to_path_buf()
    });
    config.render.camera_mode = CameraMode::Spectral;
    let scene = config.build()?;
    std::fs::create_dir_all(&directory)?;
    for (name, mode) in [
        ("constant", IorMode::Constant),
        ("dispersion", IorMode::Dispersion),
    ] {
        let start = Instant::now();
        config.render.mode = mode;
        config.render.output_path = directory.join(format!("camera_{name}.png"));
        config.save(directory.join(format!("{name}.json")))?;
        let renderer = Renderer::new(config.render.clone())?;
        let image = renderer.render(&scene).map(|images| images.combined)?;
        spectral_caustics::output::save_png_with_exposure(
            &image,
            &config.render.output_path,
            (renderer).settings().exposure,
        )?;
        println!(
            "{name}: {}，{}×{}，{} SPP，{:.3}s",
            config.render.output_path.display(),
            config.render.width,
            config.render.height,
            config.render.camera_spp,
            start.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

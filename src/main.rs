//! Command line → validated job → renderer pipeline → display output.
use spectral_caustics::{config::Config, output, renderer::Renderer};
use std::{error::Error, path::PathBuf, time::Instant};
fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let start = Instant::now();
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().unwrap_or_else(|| "configs/default.json".into()));
    if args.next().is_some() {
        return Err("usage: spectral-caustics [config.json]".into());
    }
    let config = Config::load(&path)
        .map_err(|error| format!("读取配置 {} 失败：{error}", path.display()))?;
    let job = config.build()?;
    let renderer = Renderer::new(config.render.clone())?;
    println!(
        "{}: {}×{}, {} threads",
        path.display(),
        config.render.width,
        config.render.height,
        renderer.thread_count()
    );
    let images = renderer.render(&job)?;
    output::save_components(&images, &config.render.output_path, config.render.exposure)?;
    #[cfg(feature = "diagnostics")]
    if let Some(debug) = &images.path_debug {
        debug.save(&config.render.output_path)?;
    }
    println!(
        "{} ({:.3}s)",
        config.render.output_path.display(),
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

//! 同一批光子生成拆分照明；以同一相机种子输出 Direct / Caustics / Combined。

use spectral_caustics::config::Config;
use spectral_caustics::renderer::Renderer;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut config = Config::load("configs/default.json")?;
    let mut arguments = std::env::args().skip(1);
    let spp = arguments
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(config.render.camera_spp);
    if arguments.next().is_some() || spp == 0 {
        return Err("用法：camera_direct_caustics [正整数 SPP]".into());
    }
    config.render.camera_spp = spp;
    let scene = config.build()?;
    let renderer = Renderer::new(config.render.clone())?;
    let images = renderer.render(&scene)?;
    for (component, image, path) in [
        ("Direct", &images.direct, "output/camera_direct.png"),
        ("Caustics", &images.caustics, "output/camera_caustics.png"),
        ("Combined", &images.combined, "output/camera_combined.png"),
    ] {
        spectral_caustics::output::save_png_with_exposure(
            image,
            path,
            (renderer).settings().exposure,
        )?;
        println!("{component}: {path}，SPP={spp}");
    }
    #[cfg(feature = "diagnostics")]
    if let Some(debug) = &images.path_debug {
        for path in debug.save("output/caustics.png")? {
            println!("Path AOV: {}", path.display());
        }
    }
    Ok(())
}

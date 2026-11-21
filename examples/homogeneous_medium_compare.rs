//! Fixed-seed vacuum / absorption / isotropic / forward fog comparison.
use spectral_caustics::{
    config::{Config, render::CameraMode},
    medium::{HomogeneousMediumConfig, MediumCoefficient::Constant, MediumConfig},
    output,
    renderer::Renderer,
};
use std::{error::Error, path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() > 5 {
        return Err("usage: homogeneous_medium_compare [SPP] [width] [output_dir] [rgb|spectral] [density_scale]".into());
    }
    let mut c = Config::load("configs/homogeneous_fog.json")?;
    if let Some(spp) = args.first() {
        c.render.camera_spp = spp.parse()?;
    }
    if let Some(width) = args.get(1) {
        c.render.width = width.parse()?;
        c.render.height = (c.render.width * 2 / 3).max(1);
    }
    let directory = args
        .get(2)
        .map_or_else(|| PathBuf::from("output/medium_compare"), PathBuf::from);
    if let Some(mode) = args.get(3) {
        c.render.camera_mode = match mode.as_str() {
            "rgb" => CameraMode::Rgb,
            "spectral" => CameraMode::Spectral,
            _ => return Err("mode must be rgb or spectral".into()),
        };
    }
    let density: f64 = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(1.0);
    if !density.is_finite() || density < 0.0 {
        return Err("density_scale must be finite and nonnegative".into());
    }
    let Some(MediumConfig::Homogeneous(HomogeneousMediumConfig {
        sigma_a: Constant(sigma_a),
        sigma_s: Constant(sigma_s),
        min_m,
        max_m,
        ..
    })) = c.scene.medium.take()
    else {
        return Err(
            "fog configuration must contain a homogeneous medium with constant coefficients".into(),
        );
    };
    for (name, coefficients) in [
        ("vacuum", None),
        ("absorption", Some((sigma_a + sigma_s, 0.0, 0.0))),
        ("isotropic", Some((sigma_a, sigma_s, 0.0))),
        ("forward", Some((sigma_a, sigma_s, 0.7))),
    ] {
        c.scene.medium = coefficients.map(|(a, s, g)| {
            MediumConfig::Homogeneous(HomogeneousMediumConfig {
                min_m,
                max_m,
                sigma_a: (a * density).into(),
                sigma_s: (s * density).into(),
                g: g.into(),
            })
        });
        c.render.output_path = directory.join(name).join("camera_combined.png");
        let job = c.build()?;
        let renderer = Renderer::new(c.render.clone())?;
        let start = Instant::now();
        let images = renderer.render(&job)?;
        output::save_components(&images, &c.render.output_path, c.render.exposure)?;
        c.save(directory.join(name).join("config.json"))?;
        println!(
            "{name}: {} ({:.3}s)",
            c.render.output_path.display(),
            start.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

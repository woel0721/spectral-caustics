//! Equal-mean uniform / layered / clumped fog; extinction, one and eight scatters.
use spectral_caustics::{
    config::{Config, render::CameraMode},
    medium::{GridMediumConfig, HomogeneousMediumConfig, MediumConfig},
    output,
    renderer::Renderer,
};
use std::{error::Error, path::PathBuf, time::Instant};

fn density_field(base: &GridMediumConfig, clumped: bool) -> GridMediumConfig {
    let mut grid = base.clone();
    grid.resolution = [12, 10, 8];
    grid.densities.clear();
    for z in 0..8 {
        for y in 0..10 {
            for x in 0..12 {
                let p = [
                    (x as f64 + 0.5) / 12.0,
                    (y as f64 + 0.5) / 10.0,
                    (z as f64 + 0.5) / 8.0,
                ];
                let density = if clumped {
                    let mut d = 0.02;
                    for center in [[0.25, 0.35, 0.3], [0.7, 0.55, 0.5], [0.45, 0.8, 0.7]] {
                        let r2: f64 = (0..3).map(|i| ((p[i] - center[i]) / 0.18).powi(2)).sum();
                        d += 3.0 * (-0.5 * r2).exp();
                    }
                    d
                } else if z < 4 {
                    1.75
                } else {
                    0.25
                };
                grid.densities.push(density);
            }
        }
    }
    // Match average density (and integrated extinction for rays through the full
    // volume along an axis, averaged over its cross-section), not pixel brightness.
    let mean = grid.densities.iter().sum::<f64>() / grid.densities.len() as f64;
    for d in &mut grid.densities {
        *d /= mean;
    }
    grid
}

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() > 4 {
        return Err(
            "usage: grid_medium_compare [SPP] [width] [output_dir] [rgb|hero|packet]".into(),
        );
    }
    let mut c = Config::load("configs/grid_fog.json")?;
    if let Some(spp) = args.first() {
        c.render.camera_spp = spp.parse()?;
    }
    if let Some(width) = args.get(1) {
        c.render.width = width.parse()?;
        c.render.height = (c.render.width * 2 / 3).max(1);
    }
    let directory = args
        .get(2)
        .map_or_else(|| PathBuf::from("output/grid_compare"), PathBuf::from);
    let (mode, packet) = match args.get(3).map(String::as_str).unwrap_or("rgb") {
        "rgb" => (CameraMode::Rgb, 1),
        "hero" => (CameraMode::Spectral, 1),
        "packet" => (CameraMode::Spectral, 4),
        _ => return Err("mode must be rgb, hero or packet".into()),
    };
    c.render.camera_mode = mode;
    c.render.spectral_packet_size = packet;
    let Some(MediumConfig::Grid(mut base)) = c.scene.medium.take() else {
        return Err("grid fog configuration must contain a grid medium".into());
    };
    base.densities.fill(1.0);
    let homogeneous = MediumConfig::Homogeneous(HomogeneousMediumConfig {
        min_m: base.min_m,
        max_m: base.max_m,
        sigma_a: base.sigma_a_base.clone(),
        sigma_s: base.sigma_s_base.clone(),
        g: base.g,
    });
    let layered = MediumConfig::Grid(density_field(&base, false));
    let clumped = MediumConfig::Grid(density_field(&base, true));
    for (name, medium, budget) in [
        ("homogeneous", homogeneous, 8),
        ("uniform_grid", MediumConfig::Grid(base), 8),
        ("layered_single", layered.clone(), 1),
        ("layered_multiple", layered, 8),
        ("clumped_extinction", clumped.clone(), 0),
        ("clumped_single", clumped.clone(), 1),
        ("clumped_multiple", clumped, 8),
    ] {
        c.scene.medium = Some(medium);
        c.scene.max_volume_scatters = budget;
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

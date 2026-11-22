//! 同一 Sphere、相机、HDRI、太阳及 seed 的四种 RGB 材质对照。

use spectral_caustics::config::Config;
use spectral_caustics::config::material::MaterialConfig;
use spectral_caustics::config::scene::ObjectConfig;
use spectral_caustics::output;
use spectral_caustics::renderer::Renderer;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut config = Config::load("configs/sphere.json")?;
    let mut args = std::env::args().skip(1);
    if let Some(spp) = args.next() {
        config.render.camera_spp = spp.parse()?;
    }
    if let Some(size) = args.next() {
        config.render.width = size.parse()?;
        config.render.height = config.render.width;
    }
    let directory = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| "output/sphere_materials".into());
    if args.next().is_some() {
        return Err("用法：sphere_materials [SPP] [正方形边长] [输出目录]".into());
    }
    let renderer = Renderer::new(config.render.clone())?;
    for (name, material) in [
        (
            "dielectric",
            MaterialConfig::Dielectric {
                ior: 1.52,
                dispersion: None,
            },
        ),
        (
            "mirror",
            MaterialConfig::Mirror {
                reflectance: [0.95; 3],
            },
        ),
        (
            "ggx_0",
            MaterialConfig::GgxMetal {
                eta: [0.2; 3],
                k: [3.0; 3],
                roughness: 0.0,
            },
        ),
        (
            "ggx_0p4",
            MaterialConfig::GgxMetal {
                eta: [0.2; 3],
                k: [3.0; 3],
                roughness: 0.4,
            },
        ),
    ] {
        let start = Instant::now();
        let Some(ObjectConfig::Sphere {
            material: slot,
            caustics,
            ..
        }) = config.scene.objects.first_mut()
        else {
            return Err("configs/sphere.json 的第一个物体必须为 Sphere".into());
        };
        *caustics = matches!(material, MaterialConfig::Dielectric { .. });
        *slot = material;
        let output = directory.join(name);
        fs::create_dir_all(&output)?;
        config.render.output_path = output.join("camera_combined.png");
        config.save(output.join("config.json"))?;
        let scene = config.build()?;
        let images = renderer.render(&scene)?;
        output::save_components(&images, &config.render.output_path, config.render.exposure)?;
        #[cfg(feature = "diagnostics")]
        if let Some(debug) = &images.path_debug {
            debug.save(&config.render.output_path)?;
        }
        println!(
            "{name}: {}，{}×{}，{} SPP，{:.3}s",
            output.display(),
            config.render.width,
            config.render.height,
            config.render.camera_spp,
            start.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

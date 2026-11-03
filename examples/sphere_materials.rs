//! 同一 Sphere、相机、HDRI、太阳及 seed 的四种 RGB 材质对照。

use spectral_caustics::{
    config::{Config, materialconfig::MaterialConfig, sceneconfig::ObjectConfig},
    render::Render,
    scene::Scene,
};
use std::{error::Error, fs, path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
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
    config.validate()?;
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
        let ObjectConfig::Sphere {
            material: slot,
            caustics,
            ..
        } = &mut config.scene.objects[0]
        else {
            return Err("configs/sphere.json 的第一个物体必须为 Sphere".into());
        };
        *caustics = matches!(material, MaterialConfig::Dielectric { .. });
        *slot = material;
        let output = directory.join(name);
        fs::create_dir_all(&output)?;
        config.render.output_path = output.join("lighting.png");
        config.save(output.join("config.json"))?;
        let scene = Scene::from_config(&config.scene)?;
        let renderer = Render::new(config.render.clone())?;
        // 显式选择理想介质球体；Mirror/GGX 只参与 RGB 相机路径。
        let lighting = renderer.render_lighting(&scene)?;
        let images = renderer.render_camera_glass_components(
            &scene,
            &lighting,
            config.render.camera_spp,
            config.render.seed,
        )?;
        for (image, filename) in [
            (&images.direct, "camera_direct.png"),
            (&images.caustics, "camera_caustics.png"),
            (&images.combined, "camera_combined.png"),
        ] {
            renderer.save(image, output.join(filename))?;
        }
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

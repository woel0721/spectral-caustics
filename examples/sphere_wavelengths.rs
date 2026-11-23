//! 从同一光谱焦散图集提取 450/550/650 nm，使用 RGB 相机查看各层焦散。

use spectral_caustics::caustics::{SpectralCausticsMap, SpectralLighting};
use spectral_caustics::config::Config;
use spectral_caustics::config::RenderJob;
use spectral_caustics::film::Framebuffer;
use spectral_caustics::integrator::photon::integrate_spectral;
use spectral_caustics::spectrum::d65_normalization;
use spectral_caustics::spectrum::d65_spd;
use spectral_caustics::spectrum::wavelength_to_xyz;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

const WAVELENGTHS_NM: [f64; 3] = [450.0, 550.0, 650.0];

/// 只保留一个现有积分节点的光谱贡献，归一化仍使用完整 D65 光谱。
fn wavelength_lighting(
    atlas: &SpectralCausticsMap,
    scene: &RenderJob,
    wavelength_nm: f64,
) -> Result<(SpectralLighting, f64), Box<dyn Error + Send + Sync>> {
    let samples = atlas.wavelength_samples();
    let index = samples
        .iter()
        .position(|s| s.0 == wavelength_nm)
        .ok_or("diagnostic wavelength must be an integration node")?;
    let integration_width = samples[index].1;
    let cmf = spectral_caustics::spectrum::cie_xyz(wavelength_nm).ok_or("invalid wavelength")?;
    let (width, height) = atlas.dimensions();
    let mut caustics = Framebuffer::new(width, height)?;
    for (p, e) in caustics
        .pixels_mut()
        .iter_mut()
        .zip(atlas.layer_irradiance(index).unwrap())
    {
        *p = cmf * (e * integration_width);
    }
    let target = scene
        .photon
        .caustics_object_id
        .ok_or("missing caustic target")?;
    let material = scene
        .scene
        .material(scene.scene.objects[target].material_id());
    let ior = material
        .photon_ior(Some(wavelength_nm), atlas.mode())
        .ok_or("invalid IOR")?;
    let (light_wi, strength) = scene
        .scene
        .light
        .analytic_sun()?
        .ok_or("parallel light required")?;
    let k = d65_normalization(samples).ok_or("invalid spectrum")?;
    let white = samples
        .iter()
        .map(|&(nm, w)| wavelength_to_xyz(nm, d65_spd(nm).unwrap(), w, k).unwrap())
        .sum::<nalgebra::Vector3<f64>>();
    Ok((
        SpectralLighting {
            direct: None,
            caustics,
            receiver: *atlas.receiver(),
            light_wi,
            sun_irradiance_xyz: white * strength,
        },
        ior,
    ))
}

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
        .unwrap_or_else(|| "output/sphere/wavelengths".into());
    if args.next().is_some() {
        return Err("用法：sphere_wavelengths [SPP] [正方形边长] [输出目录]".into());
    }
    let scene = config.build()?;
    fs::create_dir_all(&directory)?;
    config.save(directory.join("config.json"))?;
    println!(
        "共享发射位置：seed={}，分层网格={}×{}，采样块上限={}，曝光={}",
        config.render.seed,
        config.render.width * config.render.samples_per_pixel_axis,
        config.render.height * config.render.samples_per_pixel_axis,
        config.render.max_spectral_blocks,
        config.render.exposure,
    );
    let start = Instant::now();
    let atlas = integrate_spectral(&scene, &config.render)?.ok_or("missing caustic target")?;
    println!("光谱焦散图集生成耗时 {:.3}s", start.elapsed().as_secs_f64());
    let camera_settings = spectral_caustics::config::render::RenderSetting {
        width: config.render.width,
        height: config.render.height,
        camera_spp: config.render.camera_spp,
        seed: config.render.seed,
        ..Default::default()
    };
    for wavelength_nm in WAVELENGTHS_NM {
        let start = Instant::now();
        let (lighting, ior) = wavelength_lighting(&atlas, &scene, wavelength_nm)?;
        spectral_caustics::output::save_png_with_exposure(
            &lighting.caustics,
            directory.join(format!("ground_caustics_{wavelength_nm:.0}nm.png")),
            config.render.exposure,
        )?;
        let images = spectral_caustics::integrator::camera::integrate_rgb(
            &scene,
            &camera_settings,
            Some(&lighting),
        )?;
        spectral_caustics::output::save_png_with_exposure(
            &images.caustics,
            directory.join(format!("camera_caustics_{wavelength_nm:.0}nm.png")),
            config.render.exposure,
        )?;
        println!(
            "{wavelength_nm:.0} nm：n={ior:.9}，地面/相机焦散已输出，{:.3}s",
            start.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectral_caustics::config::{material::MaterialConfig, scene::ObjectConfig};

    fn config() -> Config {
        let mut c = Config::load("configs/sphere.json").unwrap();
        c.render.width = 12;
        c.render.height = 10;
        c.render.samples_per_pixel_axis = 2;
        c.render.thread_count = Some(2);
        c.scene.environment = None;
        c
    }

    #[test]
    fn constant_ior_keeps_the_same_landing_pattern_after_spectral_density_normalization() {
        let mut c = config();
        if let ObjectConfig::Sphere { material, .. } = &mut c.scene.objects[0] {
            *material = MaterialConfig::Dielectric {
                ior: 1.52,
                dispersion: None,
            };
        }
        let scene = c.build().unwrap();
        let atlas = integrate_spectral(&scene, &c.render).unwrap().unwrap();
        let k = d65_normalization(atlas.wavelength_samples()).unwrap();
        let mut reference: Option<(SpectralLighting, Vec<f64>)> = None;
        for nm in WAVELENGTHS_NM {
            let (lighting, n) = wavelength_lighting(&atlas, &scene, nm).unwrap();
            assert_eq!(n, 1.52);
            let spectral_y = wavelength_to_xyz(nm, d65_spd(nm).unwrap(), 10.0, k)
                .unwrap()
                .y;
            let pattern: Vec<_> = lighting
                .caustics
                .pixels()
                .iter()
                .map(|p| p.y / spectral_y)
                .collect();
            assert!(pattern.iter().any(|p| *p > 0.0));
            if let Some((previous, expected)) = &reference {
                assert!(lighting.direct.is_none() && previous.direct.is_none());
                for (a, b) in pattern.iter().zip(expected) {
                    assert!((a - b).abs() < 1e-12);
                }
            }
            reference = Some((lighting, pattern));
        }
    }
}

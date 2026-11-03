//! 同一批发射位置的 450/550/650 nm 焦散诊断；相机仍使用原有 RGB 材质。

use spectral_caustics::{
    config::{Config, materialconfig::MaterialConfig, sceneconfig::ObjectConfig},
    optics::{
        iormode::IorMode,
        spectrum::{d65_normalization, d65_spd, wavelength_to_xyz},
    },
    render::{Render, SpectralLighting},
    scene::Scene,
};
use std::{error::Error, fs, path::PathBuf, time::Instant};

const WAVELENGTHS_NM: [f64; 3] = [450.0, 550.0, 650.0];

/// 只保留一个现有积分节点的光谱贡献，归一化仍使用完整 D65 光谱。
fn wavelength_lighting(
    config: &Config,
    scene: &Scene,
    wavelength_nm: f64,
) -> Result<(SpectralLighting, f64), Box<dyn Error>> {
    let samples = config.render.wavelength_samples()?;
    let delta_nm = samples
        .iter()
        .find(|(nm, _)| *nm == wavelength_nm)
        .ok_or("诊断波长必须是配置中现有的积分节点")?
        .1;
    let k = d65_normalization(&samples).ok_or("D65 归一化失败")?;
    let spectral_xyz = wavelength_to_xyz(
        wavelength_nm,
        d65_spd(wavelength_nm).ok_or("D65 查询失败")?,
        delta_nm,
        k,
    )
    .ok_or("单波长 XYZ 计算失败")?;

    let object_id = scene.caustics_object_id.ok_or("场景未选择焦散物体")?;
    let ior = scene.objects[object_id]
        .material_id()
        .and_then(|id| scene.material(id))
        .and_then(|material| material.photon_ior(Some(wavelength_nm), IorMode::Dispersion))
        .ok_or("焦散物体缺少有效的波长相关折射率")?;
    let mut optical_config = config.scene.clone();
    let material = match &mut optical_config.objects[object_id] {
        ObjectConfig::Sphere { material, .. } | ObjectConfig::Prism { material, .. } => material,
        _ => return Err("当前诊断要求 Sphere/Prism 焦散物体".into()),
    };
    *material = MaterialConfig::Dielectric {
        ior,
        dispersion: None,
    };
    // 此临时场景仅用于光源追踪，不重复加载相机的 HDRI。
    optical_config.environment = None;
    let optical_scene = Scene::from_config(&optical_config)?;
    let mut settings = config.render.clone();
    settings.mode = IorMode::Constant;
    let mut lighting = Render::new(settings)?.render_lighting(&optical_scene)?;

    // Constant 分支按 n(λ) 只追踪一次，每个像素 = 白光 XYZ × 加权光子通量。
    // 从 Y/白光Y 取回标量通量，再乘真实 CIE 单节点 XYZ；不把 XYZ 当 RGB 染色。
    // 几何、Fresnel、空间归一化及 splat 都来自原光源积分器。
    let strength = optical_scene.light.sunlight().ok_or("太阳参数无效")?.1;
    if strength <= 0.0 {
        return Err("单波长诊断要求正光源通量".into());
    }
    let white_y = lighting.sun_irradiance_xyz.y / strength;
    for xyz in &mut lighting.caustics.pixels {
        *xyz = spectral_xyz * (xyz.y / white_y);
    }
    // Direct/太阳仍保留原白光标尺。相机仅用单波长 Caustics，RGB 路径不变。
    Ok((lighting, ior))
}

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
        .unwrap_or_else(|| "output/sphere/wavelengths".into());
    if args.next().is_some() {
        return Err("用法：sphere_wavelengths [SPP] [正方形边长] [输出目录]".into());
    }
    config.validate()?;
    let scene = Scene::from_config(&config.scene)?;
    let renderer = Render::new(config.render.clone())?;
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
    for wavelength_nm in WAVELENGTHS_NM {
        let start = Instant::now();
        let (lighting, ior) = wavelength_lighting(&config, &scene, wavelength_nm)?;
        renderer.save(
            &lighting.caustics,
            directory.join(format!("ground_caustics_{wavelength_nm:.0}nm.png")),
        )?;
        let images = renderer.render_camera_glass_components(
            &scene,
            &lighting,
            config.render.camera_spp,
            config.render.seed,
        )?;
        renderer.save(
            &images.caustics,
            directory.join(format!("camera_caustics_{wavelength_nm:.0}nm.png")),
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
    use nalgebra::Vector3;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha12Rng;
    use spectral_caustics::{
        geometry::ray::Ray,
        render::{caustics::ground_to_pixel, framebuffer::Framebuffer, utils::bilinear_weights},
    };

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
    fn constant_ior_uses_the_same_landing_pattern_and_direct_map_for_all_wavelengths() {
        let mut c = config();
        if let ObjectConfig::Sphere { material, .. } = &mut c.scene.objects[0] {
            *material = MaterialConfig::Dielectric {
                ior: 1.52,
                dispersion: None,
            };
        }
        let scene = Scene::from_config(&c.scene).unwrap();
        let samples = c.render.wavelength_samples().unwrap();
        let k = d65_normalization(&samples).unwrap();
        let mut reference: Option<(SpectralLighting, Vec<f64>)> = None;
        for nm in WAVELENGTHS_NM {
            let (lighting, n) = wavelength_lighting(&c, &scene, nm).unwrap();
            assert_eq!(n, 1.52);
            let spectral_y = wavelength_to_xyz(nm, d65_spd(nm).unwrap(), 10.0, k)
                .unwrap()
                .y;
            let pattern: Vec<_> = lighting
                .caustics
                .pixels
                .iter()
                .map(|p| p.y / spectral_y)
                .collect();
            assert!(pattern.iter().any(|p| *p > 0.0));
            if let Some((previous, expected)) = &reference {
                assert_eq!(lighting.direct.pixels, previous.direct.pixels);
                for (a, b) in pattern.iter().zip(expected) {
                    assert!((a - b).abs() < 1e-12);
                }
            }
            reference = Some((lighting, pattern));
        }
    }

    #[test]
    fn monochromatic_maps_match_shared_ray_direct_spectral_photon_traces() {
        let c = config();
        let scene = Scene::from_config(&c.scene).unwrap();
        let samples = c.render.wavelength_samples().unwrap();
        let k = d65_normalization(&samples).unwrap();
        let nx = c.render.width * c.render.samples_per_pixel_axis;
        let ny = c.render.height * c.render.samples_per_pixel_axis;
        let spatial_weight = scene.light.total_flux
            / (nx * ny) as f64
            / (scene.receiver.area() / (c.render.width * c.render.height) as f64);
        let mut expected =
            WAVELENGTHS_NM.map(|_| Framebuffer::new(c.render.width, c.render.height));
        let mut rng = ChaCha12Rng::seed_from_u64(c.render.seed);
        let span = scene.light.aperture_max_m - scene.light.aperture_min_m;
        for y in 0..ny {
            for x in 0..nx {
                let ray = Ray::new(
                    Vector3::new(
                        scene.light.aperture_min_m.x
                            + (x as f64 + rng.random::<f64>()) / nx as f64 * span.x,
                        scene.light.aperture_min_m.y
                            + (y as f64 + rng.random::<f64>()) / ny as f64 * span.y,
                        scene.light.height_m,
                    ),
                    scene.light.direction,
                )
                .unwrap();
                if scene
                    .intersect(&ray)
                    .is_none_or(|hit| hit.object_id != scene.caustics_object_id)
                {
                    continue;
                }
                for (nm, image) in WAVELENGTHS_NM.into_iter().zip(&mut expected) {
                    let Some(landing) =
                        scene.trace_caustics_photon(ray, Some(nm), IorMode::Dispersion)
                    else {
                        continue;
                    };
                    let Some((px, py)) = ground_to_pixel(
                        landing.position,
                        image.width,
                        image.height,
                        &scene.receiver,
                    ) else {
                        continue;
                    };
                    let xyz = wavelength_to_xyz(nm, d65_spd(nm).unwrap(), 10.0, k).unwrap()
                        * spatial_weight
                        * landing.weight;
                    for (x, y, w) in bilinear_weights(px, py, image.width, image.height).unwrap() {
                        assert!(image.accumulate(x, y, xyz * w));
                    }
                }
            }
        }
        for (nm, expected) in WAVELENGTHS_NM.into_iter().zip(expected) {
            let (actual, _) = wavelength_lighting(&c, &scene, nm).unwrap();
            assert!(actual.caustics.pixels.iter().any(|p| p.y > 0.0));
            for (a, b) in actual.caustics.pixels.iter().zip(expected.pixels) {
                assert!((a - b).norm() < 1e-12 * (1.0 + b.norm()));
            }
        }
    }
}

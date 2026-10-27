//! 读取 JSON，构建运行时 Scene/Render，输出主图及同配置的对照、调试图。

use spectral_caustics::{
    caustics::DEBUG_WAVELENGTHS_NM, config::Config, iormode::IorMode, render::Render, scene::Scene,
    sceneconfig::ObjectConfig,
};
use std::{
    error::Error,
    path::{Path, PathBuf},
    time::Instant,
};

// 避免用户把主图命名为某张调试图时，后续输出覆盖主图。
fn auxiliary_path(primary: &Path, name: &str) -> PathBuf {
    let directory = primary.parent().unwrap_or(Path::new("."));
    let path = directory.join(name);
    if path == primary {
        directory.join(format!("debug_{name}"))
    } else {
        path
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    let mut arguments = std::env::args_os().skip(1);
    let path = PathBuf::from(arguments.next().unwrap_or_else(|| "src/config.json".into()));
    if arguments.next().is_some() {
        return Err("用法：spectral-caustics [config.json 路径]".into());
    }
    let config = Config::load(&path)
        .map_err(|error| format!("读取配置 {} 失败：{error}", path.display()))?;
    let scene = Scene::from_config(&config.scene)?;
    let renderer = Render::new(config.render.clone())?;
    println!(
        "配置：{}，{}×{}，{} 个渲染线程",
        path.display(),
        config.render.width,
        config.render.height,
        renderer.thread_count()
    );

    let framebuffer = renderer.render(&scene)?;
    renderer.save(&framebuffer, &config.render.output_path)?;
    println!(
        "已输出 {:?} 主图：{}",
        config.render.mode,
        config.render.output_path.display()
    );
    drop(framebuffer);

    let (comparison_mode, filename) = match config.render.mode {
        IorMode::Dispersion => (IorMode::Constant, "constant_ior.png"),
        IorMode::Constant => (IorMode::Dispersion, "dispersion.png"),
    };
    let framebuffer = renderer.render_mode(&scene, comparison_mode)?;
    let path = auxiliary_path(&config.render.output_path, filename);
    renderer.save(&framebuffer, &path)?;
    println!("已输出 {:?} 对照：{}", comparison_mode, path.display());
    drop(framebuffer);

    let [framebuffer] = renderer.refracted_hits(&scene, [None], IorMode::Constant)?;
    renderer.save(
        &framebuffer,
        auxiliary_path(&config.render.output_path, "refracted_hits.png"),
    )?;
    drop(framebuffer);
    let wavelength_images =
        renderer.refracted_hits(&scene, DEBUG_WAVELENGTHS_NM.map(Some), IorMode::Dispersion)?;
    for (wavelength_nm, framebuffer) in DEBUG_WAVELENGTHS_NM.into_iter().zip(wavelength_images) {
        let name = format!("wavelength_{wavelength_nm:.0}nm.png");
        renderer.save(
            &framebuffer,
            auxiliary_path(&config.render.output_path, &name),
        )?;
    }

    let mut reference_config = config.scene.clone();
    reference_config
        .objects
        .retain(|object| matches!(object, ObjectConfig::Ground { .. }));
    let reference_scene = Scene::from_config(&reference_config)?;
    let reference = renderer.ground_hits(&reference_scene)?;
    renderer.save(
        &reference,
        auxiliary_path(&config.render.output_path, "no_prism.png"),
    )?;
    println!("对照和调试图已保存到主图目录");
    println!("总耗时：{:.3} 秒", start.elapsed().as_secs_f64());
    Ok(())
}

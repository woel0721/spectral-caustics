//! JSON 场景：共享 BSDF 路径输出相机合成图及 Direct/Caustics 分量。
use spectral_caustics::{config::Config, render::Render, scene::Scene};
use std::{
    error::Error,
    path::{Path, PathBuf},
    time::Instant,
};

// 主图占用辅助图名称时，避免覆盖用户指定的输出。
fn auxiliary_path(primary: &Path, name: &str) -> PathBuf {
    let directory = primary.parent().unwrap_or(Path::new("."));
    let path = directory.join(name);
    if path == primary {
        directory.join(format!("aux_{name}"))
    } else {
        path
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    let mut arguments = std::env::args_os().skip(1);
    let path = PathBuf::from(
        arguments
            .next()
            .unwrap_or_else(|| "configs/default.json".into()),
    );
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

    let lighting = renderer.render_lighting(&scene)?;
    let images = renderer.render_camera_glass_components(
        &scene,
        &lighting,
        config.render.camera_spp,
        config.render.seed,
    )?;
    renderer.save(&images.combined, &config.render.output_path)?;
    println!(
        "相机合成图：{}，SPP={}",
        config.render.output_path.display(),
        config.render.camera_spp
    );
    for (image, name) in [
        (&images.direct, "camera_direct.png"),
        (&images.caustics, "camera_caustics.png"),
    ] {
        let path = auxiliary_path(&config.render.output_path, name);
        renderer.save(image, &path)?;
        println!("相机：{}，SPP={}", path.display(), config.render.camera_spp);
    }
    if let Some(debug) = &images.path_debug {
        for path in debug.save(&config.render.output_path)? {
            println!("路径调试：{}", path.display());
        }
    }
    println!("总耗时：{:.3} 秒", start.elapsed().as_secs_f64());
    Ok(())
}

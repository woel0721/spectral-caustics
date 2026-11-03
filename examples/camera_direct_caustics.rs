//! 同一批光子生成拆分照明；以同一相机种子输出 Direct / Caustics / Combined。

use spectral_caustics::{config::Config, render::Render, scene::Scene};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::load("configs/default.json")?;
    let mut arguments = std::env::args().skip(1);
    let spp = arguments
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(config.render.camera_spp);
    if arguments.next().is_some() || spp == 0 {
        return Err("用法：camera_direct_caustics [正整数 SPP]".into());
    }
    let scene = Scene::from_config(&config.scene)?;
    let renderer = Render::new(config.render.clone())?;
    // 只发射一批光子，三张相机图共享未曝光的焦散数据和太阳光谱。
    let lighting = renderer.render_lighting(&scene)?;
    // 只追踪一批相机路径，同时得到两个分量，在线性 RGB 均值相加生成 Combined。
    let images =
        renderer.render_camera_glass_components(&scene, &lighting, spp, config.render.seed)?;
    for (component, image, path) in [
        ("Direct", &images.direct, "output/camera_direct.png"),
        ("Caustics", &images.caustics, "output/camera_caustics.png"),
        ("Combined", &images.combined, "output/camera_combined.png"),
    ] {
        renderer.save(image, path)?;
        println!("{component}: {path}，SPP={spp}");
    }
    if let Some(debug) = &images.path_debug {
        for path in debug.save("output/caustics.png")? {
            println!("Path AOV: {}", path.display());
        }
    }
    Ok(())
}

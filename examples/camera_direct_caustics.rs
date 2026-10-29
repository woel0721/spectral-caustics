//! 同一批光子生成拆分照明；以同一相机种子输出 Direct / Caustics / Combined。

use spectral_caustics::{
    config::Config,
    render::{GroundLightingComponent, Render, render_camera_glass_with_lighting},
    scene::Scene,
};
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
    for (component, path) in [
        (GroundLightingComponent::Direct, "output/camera_direct.png"),
        (
            GroundLightingComponent::Caustics,
            "output/camera_caustics.png",
        ),
        (
            GroundLightingComponent::Combined,
            "output/camera_combined.png",
        ),
    ] {
        let image = render_camera_glass_with_lighting(
            &scene,
            &lighting,
            config.render.width,
            config.render.height,
            spp,
            config.render.seed,
            component,
        )?;
        renderer.save(&image, path)?;
        println!("{component:?}: {path}，SPP={spp}");
    }
    Ok(())
}

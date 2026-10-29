//! 两遍渲染：光谱地面 XYZ 辐照度 -> 玻璃相机路径 -> sRGB PNG。

use spectral_caustics::{
    config::Config,
    render::{Render, render_camera_glass_with_ground},
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
        return Err("用法：camera_glass_with_ground [正整数 SPP]".into());
    }
    let scene = Scene::from_config(&config.scene)?;
    let renderer = Render::new(config.render.clone())?;
    let ground_irradiance = renderer.render(&scene)?;
    renderer.save(&ground_irradiance, &config.render.output_path)?;
    println!(
        "已生成光谱地面辐照度预览：{}",
        config.render.output_path.display()
    );

    // 传递未曝光的内存 Framebuffer，不从预览 PNG 读取照明数据。
    let image = render_camera_glass_with_ground(
        &scene,
        &ground_irradiance,
        config.render.width,
        config.render.height,
        spp,
        config.render.seed,
    )?;
    let path = "output/camera_glass_with_ground.png";
    renderer.save(&image, path)?;
    println!("已输出光谱地面与玻璃相机图：{path}，SPP={spp}");
    Ok(())
}

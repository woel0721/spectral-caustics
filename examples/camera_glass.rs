//! 使用默认 JSON 渲染有界玻璃相机路径，输出一张独立的 PNG。

use spectral_caustics::{
    config::Config,
    render::{render_camera_glass, save_png_with_exposure},
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
    if arguments.next().is_some() {
        return Err("用法：camera_glass [SPP]".into());
    }
    let scene = Scene::from_config(&config.scene)?;
    let image = render_camera_glass(
        &scene,
        config.render.width,
        config.render.height,
        spp,
        config.render.seed,
    )?;
    let path = "output/camera_glass.png";
    save_png_with_exposure(&image, path, config.render.exposure)?;
    println!("已输出玻璃相机路径图：{path}，SPP={spp}");
    Ok(())
}

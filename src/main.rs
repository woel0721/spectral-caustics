//! Day 2 示例：发射平行光，输出带三棱镜阴影的地面命中点灰度图。

use spectral_caustics::{
    caustics::render_ground_hits, ground::Ground, prism::Prism, render::save_png, scene::Scene,
};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let mut scene = Scene::new(512, 512);
    scene.add(Box::new(Prism::new().expect("固定三棱镜应能正常构造")));
    let ground_id = scene.add(Box::new(Ground::new(0.0).expect("地面高度应为有限值")));

    // 每个输出像素对应一个发射小格，取中心发射一条向下的光线。
    // 第三个参数改为 2，会在每格发射 2×2 条光线，同时自动减小每条的权重。
    let framebuffer = render_ground_hits(&scene, ground_id, 1)?;
    let path = "output/ground_hits.png";
    save_png(&framebuffer, path)?;
    println!(
        "已输出 {}×{} 地面命中灰度图：{path}",
        framebuffer.width, framebuffer.height
    );
    Ok(())
}

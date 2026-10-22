//! Day 3：比较无三棱镜和固定折射率玻璃的地面灰度分布。

use spectral_caustics::{
    caustics::{render_ground_hits, render_refracted_ground_hits},
    ground::Ground,
    prism::Prism,
    render::save_png,
    scene::Scene,
};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let mut scene = Scene::new(512, 512);
    let prism_id = scene.add(Box::new(Prism::new().expect("固定三棱镜应能正常构造")));
    let ground_id = scene.add(Box::new(Ground::new(0.0).expect("地面高度应为有限值")));

    // 每个发射小格分为 4×4 层，每层随机发射一条光线，并通过 take_photon 查询落点。
    let samples_per_pixel_axis = 4;
    let framebuffer =
        render_refracted_ground_hits(&scene, prism_id, ground_id, samples_per_pixel_axis)?;
    let path = "output/refracted_hits.png";
    save_png(&framebuffer, path)?;
    println!(
        "已输出 {}×{} 折射地面灰度图：{path}",
        framebuffer.width, framebuffer.height
    );

    // 无三棱镜对照：相同尺寸、发射网格、总通量和曝光，便于观察折射造成的偏移。
    let mut reference_scene = Scene::new(scene.width, scene.height);
    let reference_ground_id =
        reference_scene.add(Box::new(Ground::new(0.0).expect("地面高度应为有限值")));
    let reference = render_ground_hits(
        &reference_scene,
        reference_ground_id,
        samples_per_pixel_axis,
    )?;
    let reference_path = "output/no_prism.png";
    save_png(&reference, reference_path)?;
    println!("已输出无三棱镜对照图：{reference_path}");
    Ok(())
}

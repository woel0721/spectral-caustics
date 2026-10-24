//! 输出彩色光谱焦散、固定折射率白光对照和各阶段的地面灰度调试图。

use spectral_caustics::{
    caustics::{
        DEBUG_WAVELENGTHS_NM, render_ground_hits, render_refracted_ground_hits,
        render_spectral_caustics,
    },
    ground::Ground,
    iormode::IorMode,
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

    // 两种模式使用相同空间采样、41 点 D65 光谱及固定曝光，仅改变玻璃折射率。
    // 每次循环保存后释放当前 Framebuffer，避免同时保留两套光谱渲染缓冲。
    for (mode, path, label) in [
        (IorMode::Dispersion, "output/caustics.png", "色散彩色焦散"),
        (
            IorMode::Constant,
            "output/constant_ior.png",
            "固定折射率白光对照",
        ),
    ] {
        println!("正在渲染{label}……");
        let framebuffer =
            render_spectral_caustics(&scene, prism_id, ground_id, samples_per_pixel_axis, mode)?;
        save_png(&framebuffer, path)?;
        println!(
            "已输出 {}×{} {label}：{path}",
            framebuffer.width, framebuffer.height
        );
    }

    let [framebuffer] = render_refracted_ground_hits(
        &scene,
        prism_id,
        ground_id,
        samples_per_pixel_axis,
        [None],
        IorMode::Constant,
    )?;
    let path = "output/refracted_hits.png";
    save_png(&framebuffer, path)?;
    println!(
        "已输出 {}×{} 折射地面灰度图：{path}",
        framebuffer.width, framebuffer.height
    );

    let wavelength_images = render_refracted_ground_hits(
        &scene,
        prism_id,
        ground_id,
        samples_per_pixel_axis,
        DEBUG_WAVELENGTHS_NM.map(Some),
        IorMode::Dispersion,
    )?;
    for (wavelength_nm, framebuffer) in DEBUG_WAVELENGTHS_NM.into_iter().zip(wavelength_images) {
        let path = format!("output/wavelength_{wavelength_nm:.0}nm.png");
        save_png(&framebuffer, &path)?;
        println!("已输出 {wavelength_nm:.0} nm 地面落点调试图：{path}");
    }

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

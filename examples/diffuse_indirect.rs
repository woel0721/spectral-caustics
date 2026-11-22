//! A colored diffuse wall and neutral floor at zero, one and four diffuse bounces.
use nalgebra::Vector2;
use nalgebra::Vector3;
use spectral_caustics::camera::Camera;
use spectral_caustics::config::RenderJob;
use spectral_caustics::config::camera::CameraConfig;
use spectral_caustics::config::render::RenderSetting;
use spectral_caustics::geometry::ground::Ground;
use spectral_caustics::geometry::triangle::Triangle;
use spectral_caustics::material::lambertian::Lambertian;
use spectral_caustics::renderer::Renderer;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut arguments = std::env::args().skip(1);
    let spp = arguments
        .next()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(256);
    if spp == 0 || arguments.next().is_some() {
        return Err("用法：diffuse_indirect [正整数 SPP]".into());
    }
    let mut scene = RenderJob::new();
    scene.ground_checkerboard = None;
    scene.camera = Camera::from_config(&CameraConfig {
        position: [-2.8, -3.8, 2.5],
        target: [0.0, 0.0, 0.55],
        up: [0.0, 0.0, 1.0],
        fov_y_degrees: 42.0,
    })?;
    scene.scene.light.as_parallel_mut().unwrap().direction =
        Vector3::new(1.0, 0.0, -1.0).normalize();
    scene.photon.ground_id = Some(scene.scene.add(
        Box::new(Ground::with_bounds(0.0, Vector2::repeat(-2.5), Vector2::repeat(2.5)).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.7))?),
    ));
    let corners = [
        Vector3::new(0.9, -1.2, 0.0),
        Vector3::new(0.9, -1.2, 1.8),
        Vector3::new(0.9, 1.2, 1.8),
        Vector3::new(0.9, 1.2, 0.0),
    ];
    for [a, b, c] in [[0, 1, 2], [0, 2, 3]] {
        scene.scene.add(
            Box::new(Triangle::new(corners[a], corners[b], corners[c]).unwrap()),
            Box::new(Lambertian::new(Vector3::new(0.85, 0.12, 0.04))?),
        );
    }
    let renderer = Renderer::new(RenderSetting {
        width: 320,
        height: 240,
        camera_spp: spp,
        seed: 42,
        ..Default::default()
    })?;
    for bounces in [0, 1, 4] {
        scene.camera_options.max_diffuse_bounces = bounces;
        let images = renderer.render(&scene)?;
        let path = format!("output/diffuse_{bounces}.png");
        spectral_caustics::output::save_png_with_exposure(
            &images.combined,
            &path,
            renderer.settings().exposure,
        )?;
        println!("{path}: SPP={spp}, max_diffuse_bounces={bounces}");
    }
    Ok(())
}

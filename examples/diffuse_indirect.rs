//! E1: a colored diffuse wall illuminates a neutral floor through one diffuse bounce.
use nalgebra::{Vector2, Vector3};
use spectral_caustics::{
    camera::Camera,
    config::{cameraconfig::CameraConfig, rendersetting::RenderSetting},
    geometry::{ground::Ground, triangle::Triangle},
    material::lambertian::Lambertian,
    render::Render,
    scene::Scene,
};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let spp = arguments
        .next()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(256);
    if spp == 0 || arguments.next().is_some() {
        return Err("用法：diffuse_indirect [正整数 SPP]".into());
    }
    let mut scene = Scene::new();
    scene.ground_checkerboard = None;
    scene.camera = Camera::from_config(&CameraConfig {
        position: [-2.8, -3.8, 2.5],
        target: [0.0, 0.0, 0.55],
        up: [0.0, 0.0, 1.0],
        fov_y_degrees: 42.0,
    })?;
    scene.light.direction = Vector3::new(1.0, 0.0, -1.0).normalize();
    scene.ground_id = Some(scene.add_with_material(
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
        scene.add_with_material(
            Box::new(Triangle::new(corners[a], corners[b], corners[c]).unwrap()),
            Box::new(Lambertian::new(Vector3::new(0.85, 0.12, 0.04))?),
        );
    }
    let renderer = Render::new(RenderSetting {
        width: 320,
        height: 240,
        ..Default::default()
    })?;
    let lighting = renderer.render_lighting(&scene)?;
    for bounces in [0, 1] {
        scene.max_diffuse_bounces = bounces;
        let images = renderer.render_camera_glass_components(&scene, &lighting, spp, 42)?;
        let path = format!("output/e1_diffuse_{bounces}.png");
        renderer.save(&images.combined, &path)?;
        println!("{path}: SPP={spp}, max_diffuse_bounces={bounces}");
    }
    Ok(())
}

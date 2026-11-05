use crate::integrator::camera::{
    transport::Caustics,
    types::{CameraPathRngs, CameraPathState},
};
// Helpers exercising the current path state; no frozen transport implementation.
use super::ground::GroundLighting;
use super::types::CameraPathComponents;
use crate::config::RenderJob;
use crate::geometry::hit::Hit;
use crate::geometry::ray::Ray;
use crate::material::Material;
use nalgebra::Vector3;
use std::error::Error;

pub(super) fn evaluate_path(
    scene: &RenderJob,
    ray: Ray,
    rngs: &mut CameraPathRngs<'_>,
    light_wi: Vector3<f64>,
    light: f64,
    ground: Option<&GroundLighting<'_>>,
) -> Result<CameraPathComponents, Box<dyn Error + Send + Sync>> {
    let mut state = CameraPathState::new(ray, scene.camera_options.max_diffuse_bounces);
    super::path::trace_camera_path(
        scene,
        &mut state,
        rngs,
        light_wi,
        light,
        (ground).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )?;
    Ok(state.radiance)
}

pub(super) fn trace_continuation(
    scene: &RenderJob,
    hit: &Hit,
    material: &dyn Material,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn rand::RngCore,
    state: &mut CameraPathState,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if state.max_non_delta_bounces == 0 {
        return Ok(());
    }
    let Some(first) = super::path::sample_scatter_ray(
        scene,
        hit,
        material,
        -state.ray.direction,
        crate::integrator::camera::transport::Rgb::default(),
        crate::optics::dispersion::IorMode::Constant,
        rng,
    )?
    else {
        return Ok(());
    };
    if !state.apply_scatter(first.event, first.throughput)? {
        return Ok(());
    }
    state.ray = first.ray;
    let mut unused_camera = super::diffuse_tests::SequenceRng::new(&[]);
    let mut unused_environment = super::diffuse_tests::SequenceRng::new(&[]);
    super::path::trace_camera_path(
        scene,
        state,
        &mut super::types::CameraPathRngs {
            camera: &mut unused_camera,
            environment: &mut unused_environment,
            diffuse: rng,
        },
        light.0,
        light.1,
        (ground).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
}

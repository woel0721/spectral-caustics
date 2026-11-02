//! Helpers exercising the current path state; no frozen transport implementation.
use super::{
    ground::GroundLighting,
    types::{CameraPathComponents, CameraPathRngs, CameraPathState, LastScatter},
};
use crate::{
    geometry::{hit::Hit, ray::Ray},
    material::Material,
    scene::Scene,
};
use nalgebra::Vector3;
use std::error::Error;

pub(super) fn evaluate_path(
    scene: &Scene,
    ray: Ray,
    rngs: &mut CameraPathRngs<'_>,
    light_wi: Vector3<f64>,
    light: f64,
    ground: Option<&GroundLighting<'_>>,
) -> Result<CameraPathComponents, Box<dyn Error>> {
    let mut state = CameraPathState::new(ray, scene.max_diffuse_bounces);
    super::path::trace_camera_path(scene, &mut state, rngs, light_wi, light, ground)?;
    Ok(state.radiance)
}

pub(super) fn trace_continuation(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn rand::RngCore,
    state: &mut CameraPathState,
) -> Result<(), Box<dyn Error>> {
    if state.max_diffuse_bounces == 0 {
        return Ok(());
    }
    let Some(first) =
        super::diffuse::sample_diffuse_ray(scene, hit, material, -state.ray.direction, rng)?
    else {
        return Ok(());
    };
    if !state.apply_scatter(LastScatter::Diffuse, first.throughput)? {
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
        ground,
    )
}

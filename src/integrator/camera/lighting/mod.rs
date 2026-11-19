pub(crate) mod area;
pub(crate) mod environment;
pub(crate) mod volume;
// Camera render boundary and binary shadow visibility.
use super::surface_ray_origin;
use crate::{
    config::RenderJob,
    geometry::{hit::Hit, ray::Ray},
    medium::Medium,
};
use nalgebra::Vector3;
use std::error::Error;

pub(in crate::integrator::camera) fn transmittance_to_light(
    scene: &RenderJob,
    position: Vector3<f64>,
    normal: Vector3<f64>,
    wi: Vector3<f64>,
    medium: Option<&dyn Medium>,
) -> Result<f64, Box<dyn Error + Send + Sync>> {
    let origin = surface_ray_origin(&scene.scene, position, normal, 1.0);
    let shadow_ray = Ray::new(origin, wi).ok_or("阴影光线起点或方向无效")?;
    Ok(shadow_transmittance(
        scene.scene.intersect(&shadow_ray),
        &shadow_ray,
        f64::INFINITY,
        medium,
    ))
}

/// Geometry remains opaque (including glass); an unblocked segment includes medium Tr.
pub(in crate::integrator::camera) fn shadow_transmittance(
    hit: Option<Hit>,
    ray: &Ray,
    distance: f64,
    medium: Option<&dyn Medium>,
) -> f64 {
    if hit.is_some_and(|h| h.distance < distance) {
        return 0.0;
    }
    medium.map_or(1.0, |m| m.transmittance_ray(ray, distance))
}

pub(in crate::integrator::camera) fn camera_lighting_setup(
    job: &RenderJob,
    width: usize,
    height: usize,
) -> Result<(Vector3<f64>, f64), Box<dyn Error + Send + Sync>> {
    crate::config::scene::validate_diffuse_bounces(job.camera_options.max_diffuse_bounces)?;
    crate::config::scene::validate_volume_scatters(job.camera_options.max_volume_scatters)?;
    if job.camera_options.light_samples == 0 || job.camera_options.environment_samples == 0 {
        return Err("light sample counts must be positive".into());
    }
    if width == 0 || height == 0 {
        return Err("image dimensions must be positive".into());
    }
    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(job.camera_options.light_samples))
        .and_then(|n| {
            n.checked_mul(
                job.camera_options.max_diffuse_bounces + 1 + job.camera_options.max_volume_scatters,
            )
        })
        .ok_or("camera workload overflow")?;
    Ok(job
        .scene
        .light
        .analytic_sun()?
        .unwrap_or((Vector3::z(), 0.0)))
}

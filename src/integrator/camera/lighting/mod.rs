pub(crate) mod area;
pub(crate) mod environment;
// Camera render boundary and binary shadow visibility.
use super::offset_ray_origin;
use crate::{config::RenderJob, geometry::ray::Ray};
use nalgebra::Vector3;
use std::error::Error;

pub(in crate::integrator::camera) fn visible_to_light(
    scene: &RenderJob,
    position: Vector3<f64>,
    normal: Vector3<f64>,
    wi: Vector3<f64>,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let origin = offset_ray_origin(position, normal, 1.0);
    let shadow_ray = Ray::new(origin, wi).ok_or("阴影光线起点或方向无效")?;
    Ok(scene.scene.intersect(&shadow_ray).is_none())
}

pub(in crate::integrator::camera) fn camera_lighting_setup(
    job: &RenderJob,
    width: usize,
    height: usize,
) -> Result<(Vector3<f64>, f64), Box<dyn Error + Send + Sync>> {
    crate::config::scene::validate_diffuse_bounces(job.camera_options.max_diffuse_bounces)?;
    if job.camera_options.light_samples == 0 || job.camera_options.environment_samples == 0 {
        return Err("light sample counts must be positive".into());
    }
    if width == 0 || height == 0 {
        return Err("image dimensions must be positive".into());
    }
    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(job.camera_options.light_samples))
        .and_then(|n| n.checked_mul(job.camera_options.max_diffuse_bounces + 1))
        .ok_or("camera workload overflow")?;
    Ok(job
        .scene
        .light
        .analytic_sun()?
        .unwrap_or((Vector3::z(), 0.0)))
}

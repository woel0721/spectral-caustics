pub(crate) mod area;
pub(crate) mod environment;
pub(crate) mod volume;
// Camera render boundary and binary shadow visibility.
use super::surface_ray_origin;
use crate::{
    config::RenderJob,
    geometry::{hit::Hit, ray::Ray},
    medium::{ACHROMATIC_QUERY_NM, Medium, checked_transmittance},
};
use nalgebra::Vector3;
use std::error::Error;

pub(in crate::integrator::camera) fn transmittance_to_light(
    scene: &RenderJob,
    position: Vector3<f64>,
    normal: Vector3<f64>,
    wi: Vector3<f64>,
    medium: Option<&dyn Medium>,
    wavelength_nm: Option<f64>,
) -> Result<f64, Box<dyn Error + Send + Sync>> {
    let origin = surface_ray_origin(&scene.scene, position, normal, 1.0);
    let shadow_ray = Ray::new(origin, wi).ok_or("阴影光线起点或方向无效")?;
    let tr = shadow_transmittance(
        scene.scene.intersect(&shadow_ray),
        &shadow_ray,
        f64::INFINITY,
        medium,
        wavelength_nm,
    )?;
    Ok(tr)
}

/// Geometry remains opaque (including glass); an unblocked segment includes medium Tr.
pub(in crate::integrator::camera) fn shadow_transmittance(
    hit: Option<Hit>,
    ray: &Ray,
    distance: f64,
    medium: Option<&dyn Medium>,
    wavelength_nm: Option<f64>,
) -> Result<f64, &'static str> {
    if hit.is_some_and(|h| h.distance < distance) {
        return Ok(0.0);
    }
    medium.map_or(Ok(1.0), |m| {
        checked_transmittance(
            m,
            ray,
            distance,
            wavelength_nm.unwrap_or(ACHROMATIC_QUERY_NM),
        )
    })
}

pub(in crate::integrator::camera) fn camera_lighting_setup(
    job: &RenderJob,
    width: usize,
    height: usize,
    paths_per_pixel: usize,
) -> Result<(Vector3<f64>, f64), Box<dyn Error + Send + Sync>> {
    crate::config::scene::validate_diffuse_bounces(job.camera_options.max_diffuse_bounces)?;
    crate::config::scene::validate_volume_scatters(job.camera_options.max_volume_scatters)?;
    if job.camera_options.light_samples == 0 || job.camera_options.environment_samples == 0 {
        return Err("light sample counts must be positive".into());
    }
    if width == 0 || height == 0 {
        return Err("image dimensions must be positive".into());
    }
    crate::config::render::validate_camera_workload(
        width,
        height,
        paths_per_pixel,
        1 + job.camera_options.max_diffuse_bounces + job.camera_options.max_volume_scatters,
        job.camera_options.environment_samples,
        job.camera_options.light_samples,
    )?;
    Ok(job
        .scene
        .light
        .analytic_sun()?
        .unwrap_or((Vector3::z(), 0.0)))
}

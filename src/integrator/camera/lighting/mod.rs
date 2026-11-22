pub(crate) mod area;
pub(crate) mod environment;
pub(crate) mod volume;
// Camera render boundary and binary shadow visibility.
use super::surface_ray_origin;
use crate::{
    config::{RenderJob, scene::EnvironmentMisHeuristic},
    geometry::ray::Ray,
    medium::{ACHROMATIC_QUERY_NM, Medium, checked_transmittance},
};
use nalgebra::Vector3;
use std::error::Error;

/// 按 Balance 或 Power 启发式把分子 `numerator` 与策略组合密度合成 MIS 权重项 `numerator * w_i / q_i`。
pub(super) fn mis_weighted_value(
    numerator: f64,
    sampled: f64,
    other: f64,
    heuristic: EnvironmentMisHeuristic,
) -> Option<f64> {
    match heuristic {
        EnvironmentMisHeuristic::Power => {
            environment::power_heuristic(sampled, other).map(|(_, w)| numerator * w)
        }
        EnvironmentMisHeuristic::Balance => {
            if !sampled.is_finite() || !other.is_finite() || sampled < 0.0 || other < 0.0 {
                return None;
            }
            let total = sampled + other;
            (total.is_finite() && total > 0.0).then_some(numerator / total)
        }
    }
}

pub(in crate::integrator::camera) fn transmittance_to_light(
    scene: &RenderJob,
    has_medium_interface: bool,
    position: Vector3<f64>,
    normal: Vector3<f64>,
    wi: Vector3<f64>,
    medium: Option<&dyn Medium>,
    wavelength_nm: Option<f64>,
) -> Result<f64, Box<dyn Error + Send + Sync>> {
    let origin = surface_ray_origin(has_medium_interface, position, normal, 1.0);
    let shadow_ray = Ray::new(origin, wi).ok_or("阴影光线起点或方向无效")?;
    let tr = shadow_transmittance(
        scene.scene.is_occluded(&shadow_ray, f64::INFINITY),
        &shadow_ray,
        f64::INFINITY,
        medium,
        wavelength_nm,
    )?;
    Ok(tr)
}

/// Geometry remains opaque (including glass); an unblocked segment includes medium Tr.
pub(in crate::integrator::camera) fn shadow_transmittance(
    occluded: bool,
    ray: &Ray,
    distance: f64,
    medium: Option<&dyn Medium>,
    wavelength_nm: Option<f64>,
) -> Result<f64, &'static str> {
    if occluded {
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

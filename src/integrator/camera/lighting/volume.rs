//! NEE at each volume vertex. There is no surface cosine at a medium vertex.
use super::{environment::power_heuristic, shadow_transmittance};
use crate::{
    config::{RenderJob, scene::EnvironmentMisHeuristic},
    geometry::ray::Ray,
    integrator::camera::transport::CameraTransport,
    medium::{Medium, phase::HenyeyGreenstein},
};
use nalgebra::Vector3;
use rand::{Rng, RngCore};
use std::error::Error;

#[allow(clippy::too_many_arguments)]
pub(in crate::integrator::camera) fn volume_lighting<T: CameraTransport>(
    job: &RenderJob,
    medium: &dyn Medium,
    position: Vector3<f64>,
    wo: Vector3<f64>,
    phase: HenyeyGreenstein,
    sun_wi: Vector3<f64>,
    sun: T::Value,
    transport: T,
    rng: &mut dyn RngCore,
) -> Result<T::Value, Box<dyn Error + Send + Sync>> {
    let transmittance = |ray: &Ray, distance| {
        shadow_transmittance(
            job.scene.intersect_from_volume(ray),
            ray,
            distance,
            Some(medium),
        )
    };
    let mut result = T::Value::default();
    if sun != T::Value::default() {
        let ray = Ray::new(position, sun_wi).ok_or("invalid volume shadow ray")?;
        result += sun * (phase.eval(wo, sun_wi) * transmittance(&ray, f64::INFINITY));
    }
    if let Some(disk) = job.scene.light.as_disk() {
        for _ in 0..job.camera_options.light_samples {
            if let Some(s) = disk.sample_direct(position, [rng.random(), rng.random()])? {
                let ray = Ray::new(position, s.wi).ok_or("invalid volume disk ray")?;
                // incident_weight already contains Le cos_light / (r² p_area).
                let weight =
                    s.incident_weight * phase.eval(wo, s.wi) * transmittance(&ray, s.distance)
                        / job.camera_options.light_samples as f64;
                result += transport.disk_emission() * weight;
            }
        }
    }
    if let Some(env) = &job.scene.environment {
        let n = job.camera_options.environment_samples;
        let phase_count = n / 2;
        let (a, b) = if n == 1 {
            (0.5, 0.5)
        } else {
            (phase_count as f64, (n - phase_count) as f64)
        };
        for i in 0..n {
            let use_phase = if n == 1 {
                rng.random::<f64>() < a
            } else {
                i < phase_count
            };
            let wi = if use_phase {
                phase.sample(wo, rng).wi
            } else {
                env.sample_direction(rng)
                    .ok_or("invalid volume environment sample")?
                    .direction
            };
            let p = phase.eval(wo, wi);
            let q_phase = a * phase.pdf(wo, wi);
            let q_env = b * env
                .pdf_direction(wi)
                .ok_or("invalid volume environment PDF")?;
            let weight =
                if job.camera_options.environment_mis_heuristic == EnvironmentMisHeuristic::Power {
                    let (sampled, other) = if use_phase {
                        (q_phase, q_env)
                    } else {
                        (q_env, q_phase)
                    };
                    p * power_heuristic(sampled, other)
                        .ok_or("invalid volume MIS PDF")?
                        .1
                } else {
                    p / (q_phase + q_env)
                };
            let ray = Ray::new(position, wi).ok_or("invalid volume environment ray")?;
            let li = transport
                .emission(
                    env.radiance(wi)
                        .ok_or("invalid volume environment radiance")?,
                )
                .ok_or("volume emission overflow")?;
            result += li * (weight * transmittance(&ray, f64::INFINITY));
        }
    }
    if !transport.finite(result) {
        return Err("volume lighting overflow".into());
    }
    Ok(result)
}

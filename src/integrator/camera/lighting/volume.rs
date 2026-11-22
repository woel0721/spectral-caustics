//! NEE at each volume vertex. There is no surface cosine at a medium vertex.
use super::{mis_weighted_value, shadow_transmittance};
use crate::{
    config::RenderJob,
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
            job.scene.is_occluded_from_volume(ray, distance),
            ray,
            distance,
            Some(medium),
            transport.wavelength_nm(),
        )
    };
    let mut result = T::Value::default();
    if sun != T::Value::default() {
        let ray = Ray::new(position, sun_wi).ok_or("invalid volume shadow ray")?;
        result += sun * (phase.eval(wo, sun_wi) * transmittance(&ray, f64::INFINITY)?);
    }
    if let Some(disk) = job.scene.light.as_disk() {
        let emission = transport.disk_emission();
        let inv_samples = 1.0 / job.camera_options.light_samples as f64;
        for _ in 0..job.camera_options.light_samples {
            if let Some(s) = disk.sample_direct(position, [rng.random(), rng.random()])? {
                if s.incident_weight == 0.0 {
                    continue;
                }
                let ray = Ray::new(position, s.wi).ok_or("invalid volume disk ray")?;
                // incident_weight already contains Le cos_light / (r² p_area).
                let weight = s.incident_weight
                    * phase.eval(wo, s.wi)
                    * transmittance(&ray, s.distance)?
                    * inv_samples;
                result += emission * weight;
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
            let (wi, p, phase_pdf) = if use_phase {
                let s = phase.sample(wo, rng);
                (s.wi, s.p, s.pdf)
            } else {
                let wi = env
                    .sample_direction_only(rng)
                    .ok_or("invalid volume environment sample")?;
                let p = phase.eval(wo, wi);
                (wi, p, p)
            };
            let q_phase = a * phase_pdf;
            let (env_rgb, pdf_env) = env
                .radiance_and_pdf(wi)
                .ok_or("invalid volume environment PDF")?;
            if !pdf_env.is_finite() || pdf_env < 0.0 {
                return Err("invalid volume environment PDF".into());
            }
            let q_env = b * pdf_env;
            let (sampled, other) = if use_phase {
                (q_phase, q_env)
            } else {
                (q_env, q_phase)
            };
            let weight = mis_weighted_value(
                p,
                sampled,
                other,
                job.camera_options.environment_mis_heuristic,
            )
            .ok_or("invalid volume MIS PDF")?;
            let li = transport
                .emission(env_rgb)
                .ok_or("volume emission overflow")?;
            if weight == 0.0 || li == T::Value::default() {
                continue;
            }
            let ray = Ray::new(position, wi).ok_or("invalid volume environment ray")?;
            result += li * (weight * transmittance(&ray, f64::INFINITY)?);
        }
    }
    if !transport.finite(result) {
        return Err("volume lighting overflow".into());
    }
    Ok(result)
}

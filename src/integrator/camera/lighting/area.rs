//! Disk NEE owns its RNG cursor; changing its quota cannot consume path or environment draws.
use super::super::{surface_ray_origin, transport::CameraTransport};
use super::shadow_transmittance;
use crate::{
    config::RenderJob,
    geometry::{hit::Hit, ray::Ray},
    material::{Material, unit_vector},
    medium::Medium,
    sampling::rng::RenderRng,
};
use nalgebra::Vector3;
use rand::{Rng, SeedableRng};
use std::error::Error;
pub struct AreaLightSampler {
    seed: Option<u64>,
    rng: Option<RenderRng>,
    has_medium_interface: bool,
}
impl AreaLightSampler {
    pub fn new(
        job: &RenderJob,
        prefix: &mut dyn rand::RngCore,
        has_medium_interface: bool,
    ) -> Self {
        Self {
            seed: job.scene.light.as_disk().map(|_| prefix.next_u64()),
            rng: None,
            has_medium_interface,
        }
    }
    pub fn lighting<T: CameraTransport>(
        &mut self,
        job: &RenderJob,
        hit: &Hit,
        m: &dyn Material,
        wo: Vector3<f64>,
        transport: T,
        medium: Option<&dyn Medium>,
    ) -> Result<T::Value, Box<dyn Error + Send + Sync>> {
        let mut result = T::Value::default();
        let Some(seed) = self.seed else {
            return Ok(result);
        };
        let rng = self
            .rng
            .get_or_insert_with(|| RenderRng::seed_from_u64(seed));
        let disk = job.scene.light.as_disk().expect("disk-bound sampler");
        let normal = unit_vector(hit.normal).ok_or("invalid surface normal")?;
        let emission = transport.disk_emission();
        let samples = job.camera_options.light_samples as f64;
        for _ in 0..job.camera_options.light_samples {
            let Some(sample) = disk.sample_direct(hit.coords, [rng.random(), rng.random()])? else {
                continue;
            };
            let cosine = normal.dot(&sample.wi).max(0.0);
            if cosine == 0.0 || sample.incident_weight == 0.0 {
                continue;
            }
            let tr = transmittance_to_point(
                job,
                self.has_medium_interface,
                hit.coords,
                normal,
                sample.position,
                medium,
                transport.wavelength_nm(),
            )?;
            if tr == 0.0 {
                continue;
            }
            let f = transport.evaluate(m, sample.wi, wo, normal);
            if !transport.valid_weight(f) {
                return Err("invalid area BSDF".into());
            }
            let factor = (sample.incident_weight * cosine / samples) * tr;
            result += transport.multiply(f, emission) * factor;
        }
        if !transport.finite(result) {
            return Err("area light overflow".into());
        }
        Ok(result)
    }
}
fn transmittance_to_point(
    job: &RenderJob,
    has_medium_interface: bool,
    position: Vector3<f64>,
    normal: Vector3<f64>,
    target: Vector3<f64>,
    medium: Option<&dyn Medium>,
    wavelength_nm: Option<f64>,
) -> Result<f64, Box<dyn Error + Send + Sync>> {
    let origin = surface_ray_origin(has_medium_interface, position, normal, 1.0);
    let delta = target - origin;
    let distance = delta.norm();
    let ray = Ray::new(origin, delta).ok_or("invalid shadow segment")?;
    let tr = shadow_transmittance(
        job.scene.is_occluded(&ray, distance),
        &ray,
        distance,
        medium,
        wavelength_nm,
    )?;
    Ok(tr)
}

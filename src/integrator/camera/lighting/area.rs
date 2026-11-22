//! Disk NEE owns its RNG cursor; changing its quota cannot consume path or environment draws.
use super::super::{surface_ray_origin, transport::CameraTransport};
use super::shadow_transmittance;
use crate::{
    config::RenderJob,
    geometry::{hit::Hit, ray::Ray},
    material::{Material, unit_vector},
    medium::Medium,
};
use nalgebra::Vector3;
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::error::Error;
pub struct AreaLightSampler(Option<StdRng>);
impl AreaLightSampler {
    pub fn new(job: &RenderJob, prefix: &mut dyn rand::RngCore) -> Self {
        Self(
            job.scene
                .light
                .as_disk()
                .map(|_| StdRng::seed_from_u64(prefix.next_u64())),
        )
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
        let Some(rng) = &mut self.0 else {
            return Ok(result);
        };
        let disk = job.scene.light.as_disk().expect("disk-bound sampler");
        let normal = unit_vector(hit.normal).ok_or("invalid surface normal")?;
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
            let factor =
                (sample.incident_weight * cosine / job.camera_options.light_samples as f64) * tr;
            result += transport.multiply(f, transport.disk_emission()) * factor;
        }
        if !transport.finite(result) {
            return Err("area light overflow".into());
        }
        Ok(result)
    }
}
fn transmittance_to_point(
    job: &RenderJob,
    position: Vector3<f64>,
    normal: Vector3<f64>,
    target: Vector3<f64>,
    medium: Option<&dyn Medium>,
    wavelength_nm: Option<f64>,
) -> Result<f64, Box<dyn Error + Send + Sync>> {
    let origin = surface_ray_origin(&job.scene, position, normal, 1.0);
    let delta = target - origin;
    let distance = delta.norm();
    let ray = Ray::new(origin, delta).ok_or("invalid shadow segment")?;
    let tr = shadow_transmittance(
        job.scene.intersect(&ray),
        &ray,
        distance,
        medium,
        wavelength_nm,
    )?;
    Ok(tr)
}

//! Binding for the selected ideal dielectric; path transport lives in photon/path.
mod emission;
mod integrate;
mod path;
#[cfg(test)]
mod tests;

use crate::config::RenderJob;
use crate::config::render::{MAX_PHOTON_VOLUME_SCATTERS, RenderSetting};
use crate::geometry::ray::Ray;
use crate::medium::ACHROMATIC_QUERY_NM;
use crate::optics::dispersion::IorMode;
use crate::spectrum::emission::{WAVELENGTH_MAX_NM, WAVELENGTH_MIN_NM};
pub(crate) use integrate::integrate_camera_xyz;
pub use integrate::{integrate_spectral, integrate_xyz};
use nalgebra::Vector3;
pub use path::PhotonMedium;
pub use path::PhotonPathResult;
pub use path::PhotonPathState;
pub use path::PhotonTermination;
pub(crate) use path::PhotonTracer;

pub(super) type PhotonError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotonLanding {
    pub position: Vector3<f64>,
    /// Product of compensated medium weights and interface flux weights.
    pub weight: f64,
}

/// Trace one photon with deterministic extinction in participating media.
pub fn trace_photon(
    job: &RenderJob,
    ray: Ray,
    wavelength_nm: Option<f64>,
    mode: IorMode,
    max_events: usize,
    rng: &mut dyn rand::RngCore,
) -> Result<PhotonPathResult, &'static str> {
    let medium_wavelength_nm = medium_wavelength(
        wavelength_nm,
        job.scene.has_wavelength_dependent_medium_coefficients(),
    )?;
    default_selected_tracer(job, wavelength_nm, mode, max_events)?.trace(
        job,
        ray,
        job.scene.intersect(&ray),
        medium_wavelength_nm,
        rng,
    )
}

/// Trace one photon with up to `max_volume_scatters` real volume events
/// (0 through 1024), starting after the first glass reflection or refraction.
/// The initial segment and segments after the volume budget use deterministic
/// extinction. A zero volume budget is equivalent to [trace_photon].
pub fn trace_photon_with_volume(
    job: &RenderJob,
    ray: Ray,
    wavelength_nm: Option<f64>,
    mode: IorMode,
    max_events: usize,
    max_volume_scatters: usize,
    rng: &mut dyn rand::RngCore,
) -> Result<PhotonPathResult, String> {
    if max_volume_scatters > MAX_PHOTON_VOLUME_SCATTERS {
        return Err(format!(
            "photon volume scatter budget must be in 0..={MAX_PHOTON_VOLUME_SCATTERS}"
        ));
    }
    if max_volume_scatters == 0 {
        return Ok(trace_photon(
            job,
            ray,
            wavelength_nm,
            mode,
            max_events,
            rng,
        )?);
    }
    let medium_wavelength_nm =
        medium_wavelength(wavelength_nm, job.scene.has_wavelength_dependent_medium())?;
    let tracer = default_selected_tracer(job, wavelength_nm, mode, max_events)?;
    let first_hit = job.scene.intersect(&ray);
    let state = tracer.initial_state(job, ray, first_hit, medium_wavelength_nm)?;
    tracer.trace_state_with_volume(
        job,
        state,
        first_hit,
        medium_wavelength_nm,
        max_volume_scatters,
        rng,
    )
}

fn medium_wavelength(
    wavelength_nm: Option<f64>,
    wavelength_dependent: bool,
) -> Result<f64, &'static str> {
    if !wavelength_dependent {
        return Ok(ACHROMATIC_QUERY_NM);
    }
    let nm = wavelength_nm.ok_or("wavelength-dependent photon media require a wavelength")?;
    if !(WAVELENGTH_MIN_NM..=WAVELENGTH_MAX_NM).contains(&nm) {
        return Err("photon medium wavelength must be in 380–780 nm");
    }
    Ok(nm)
}

pub(crate) fn selected_tracer(
    job: &RenderJob,
    object_id: usize,
    ground_id: usize,
    wavelength_nm: Option<f64>,
    mode: IorMode,
    max_events: usize,
) -> Result<PhotonTracer, &'static str> {
    PhotonTracer::new(
        job,
        object_id,
        ground_id,
        job.photon_ior(object_id, wavelength_nm, mode)
            .ok_or("光子 IOR 无效")?,
        max_events,
    )
}

pub(super) fn photon_tracer(
    job: &RenderJob,
    settings: &RenderSetting,
    target: Option<usize>,
    ground: usize,
    nm: f64,
) -> Result<Option<PhotonTracer>, &'static str> {
    target
        .map(|id| {
            selected_tracer(
                job,
                id,
                ground,
                Some(nm),
                settings.mode,
                settings.photon_max_events,
            )
        })
        .transpose()
}

fn default_selected_tracer(
    job: &RenderJob,
    wavelength_nm: Option<f64>,
    mode: IorMode,
    max_events: usize,
) -> Result<PhotonTracer, &'static str> {
    let object_id = job.photon.caustics_object_id.ok_or("未选择焦散物体")?;
    let ground_id = job.photon.ground_id.ok_or("光子路径缺少地面")?;
    selected_tracer(job, object_id, ground_id, wavelength_nm, mode, max_events)
}

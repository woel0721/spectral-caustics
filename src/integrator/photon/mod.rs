//! Binding for the selected ideal dielectric; path transport lives in photon/path.
mod generation;
mod path;
use crate::config::RenderJob;
use crate::geometry::ray::Ray;
use crate::optics::dispersion::IorMode;
pub(crate) use generation::integrate_camera_xyz;
pub use generation::{integrate_spectral, integrate_xyz};
use nalgebra::Vector3;
pub use path::PhotonMedium;
pub use path::PhotonPathResult;
pub use path::PhotonPathState;
pub use path::PhotonTermination;
pub(crate) use path::PhotonTracer;
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotonLanding {
    pub position: Vector3<f64>,
    /// Flux coefficient / event probability; excludes radiance eta squared.
    pub weight: f64,
}
pub fn trace_photon(
    job: &RenderJob,
    ray: Ray,
    wavelength_nm: Option<f64>,
    mode: IorMode,
    max_events: usize,
    rng: &mut dyn rand::RngCore,
) -> Result<PhotonPathResult, &'static str> {
    let object_id = job.photon.caustics_object_id.ok_or("未选择焦散物体")?;
    let tracer = PhotonTracer::new(
        job,
        object_id,
        job.photon.ground_id.ok_or("光子路径缺少地面")?,
        job.photon_ior(object_id, wavelength_nm, mode)
            .ok_or("光子 IOR 无效")?,
        max_events,
    )?;
    tracer.trace(job, ray, job.scene.intersect(&ray), rng)
}

//! Pipeline orchestration. Transport and light estimators live in their integrators.
use crate::{
    config::{
        RenderJob,
        render::{CameraMode, RenderSetting},
    },
    integrator::camera::{self, CameraLightingImages},
};
use std::error::Error;
pub struct Renderer {
    settings: RenderSetting,
    pool: Option<rayon::ThreadPool>,
}
impl Renderer {
    pub fn new(settings: RenderSetting) -> Result<Self, Box<dyn Error + Send + Sync>> {
        settings.validate()?;
        let pool = settings
            .thread_count
            .map(|n| rayon::ThreadPoolBuilder::new().num_threads(n).build())
            .transpose()?;
        Ok(Self { settings, pool })
    }
    pub fn settings(&self) -> &RenderSetting {
        &self.settings
    }
    pub fn thread_count(&self) -> usize {
        self.pool.as_ref().map_or_else(
            rayon::current_num_threads,
            rayon::ThreadPool::current_num_threads,
        )
    }
    pub fn render(
        &self,
        job: &RenderJob,
    ) -> Result<CameraLightingImages, Box<dyn Error + Send + Sync>> {
        let work = || match self.settings.camera_mode {
            CameraMode::Spectral => {
                camera::validate_spectral_render(job, &self.settings)?;
                let atlas = crate::integrator::photon::integrate_spectral(job, &self.settings)?;
                camera::integrate_spectral(job, &self.settings, atlas.as_ref())
            }
            CameraMode::Rgb => {
                camera::validate_rgb_render(
                    job,
                    &self.settings,
                    job.photon.caustics_object_id.is_some(),
                )?;
                if let Some(target) = job.photon.caustics_object_id {
                    let lighting = crate::integrator::photon::integrate_camera_xyz(
                        job,
                        target,
                        job.photon.ground_id.ok_or("caustics need a receiver")?,
                        &self.settings,
                    )?;
                    camera::integrate_rgb(job, &self.settings, Some(&lighting))
                } else {
                    camera::integrate_rgb(job, &self.settings, None)
                }
            }
        };
        match &self.pool {
            Some(pool) => pool.install(work),
            None => work(),
        }
    }
}

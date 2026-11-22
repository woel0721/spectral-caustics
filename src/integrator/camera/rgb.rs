//! RGB pixel sampling. The path walk is shared with the spectral camera.
use super::{
    RenderedPixel, UnusedRng,
    ground::bind_spectral_ground_lighting,
    lighting::camera_lighting_setup,
    path::trace_camera_path,
    render_camera_pixels,
    transport::Caustics,
    types::{CameraLightingImages, CameraPathRngs, CameraPathState},
};
#[cfg(feature = "diagnostics")]
use crate::diagnostics::{PathDebugAccumulator, PathDebugSample};
use crate::{caustics::SpectralLighting, color::linear_srgb_to_xyz, config::RenderJob};
use nalgebra::{Vector2, Vector3};
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::error::Error;

pub(crate) fn validate_rgb_render(
    job: &RenderJob,
    settings: &crate::config::render::RenderSetting,
    has_caustics: bool,
) -> Result<(Vector3<f64>, f64), Box<dyn Error + Send + Sync>> {
    if job.scene.has_wavelength_dependent_medium() {
        return Err(crate::medium::SPECTRAL_CAMERA_REQUIRED.into());
    }
    if settings.camera_spp == 0 {
        return Err("camera SPP must be positive".into());
    }
    #[cfg(feature = "diagnostics")]
    if job
        .diagnostic_target
        .is_some_and(|id| id >= job.scene.objects.len())
    {
        return Err("invalid diagnostic target".into());
    }
    if has_caustics {
        super::ground::validate_rgb_caustics_ground(job)?;
    }
    camera_lighting_setup(job, settings.width, settings.height, settings.camera_spp)
}

/// Integrate RGB radiance; the optional map contributes only at the first non-delta receiver.
pub fn integrate_rgb(
    job: &RenderJob,
    settings: &crate::config::render::RenderSetting,
    lighting: Option<&SpectralLighting>,
) -> Result<CameraLightingImages, Box<dyn Error + Send + Sync>> {
    let (light_wi, light) = validate_rgb_render(job, settings, lighting.is_some())?;
    let (width, height, spp, seed) = (
        settings.width,
        settings.height,
        settings.camera_spp,
        settings.seed,
    );
    #[cfg(feature = "diagnostics")]
    let debug_path = settings.diagnostics.path;
    let binding = lighting
        .map(|l| bind_spectral_ground_lighting(job, l))
        .transpose()?;
    let ground = binding.as_ref();
    // Sun color uses the source's quadrature even without a caustics target.
    // Previously the high-level pipeline obtained this white from the photon preview.
    let sun_color = if ground.is_none() && light > 0.0 {
        let samples = settings.wavelength_samples()?;
        crate::color::xyz_to_linear_srgb(
            job.scene
                .light
                .white_xyz(&samples)
                .ok_or("source XYZ overflow")?,
        )
        .ok_or("sun color overflow")?
    } else {
        Vector3::repeat(1.0)
    };

    let has_environment = job.scene.environment.is_some();
    let has_medium = job.scene.has_medium();
    let caustics_map = ground.map_or(Caustics::None, Caustics::Rgb);
    render_camera_pixels(
        width,
        height,
        #[cfg(feature = "diagnostics")]
        debug_path.then_some(job.diagnostic_target),
        |index: usize| -> Result<RenderedPixel, String> {
            let mut camera = StdRng::seed_from_u64(pixel_seed(seed, index, CAMERA_STREAM));
            let mut environment = has_environment
                .then(|| StdRng::seed_from_u64(pixel_seed(seed, index, ENVIRONMENT_STREAM)));
            let mut medium = has_medium.then(|| {
                StdRng::seed_from_u64(pixel_seed(seed, index, crate::sampling::rng::MEDIUM_STREAM))
            });
            let mut continuation = StdRng::seed_from_u64(pixel_seed(seed, index, DIFFUSE_STREAM));
            let mut unused_environment = UnusedRng;
            let mut unused_medium = UnusedRng;
            let (mut d, mut c) = (Vector3::zeros(), Vector3::zeros());
            #[cfg(feature = "diagnostics")]
            let mut statistics = debug_path.then(PathDebugAccumulator::default);
            for _ in 0..spp {
                let offset = Vector2::new(camera.random::<f64>(), camera.random::<f64>());
                let ray = job
                    .camera
                    .ray_for_pixel_sample(index % width, index / width, width, height, offset)
                    .ok_or("invalid camera ray")?;
                let mut state = CameraPathState::new(ray, job.camera_options.max_diffuse_bounces);
                state.transport.sun_color = sun_color;
                let mut rngs = CameraPathRngs {
                    camera: &mut camera,
                    environment: match &mut environment {
                        Some(rng) => rng,
                        None => &mut unused_environment,
                    },
                    diffuse: &mut continuation,
                    medium: match &mut medium {
                        Some(rng) => rng,
                        None => &mut unused_medium,
                    },
                };
                #[cfg(feature = "diagnostics")]
                let mut sample = PathDebugSample::default();
                trace_camera_path(
                    job,
                    &mut state,
                    &mut rngs,
                    light_wi,
                    light,
                    caustics_map,
                    crate::optics::dispersion::IorMode::Constant,
                    #[cfg(feature = "diagnostics")]
                    statistics.as_ref().map(|_| &mut sample),
                )
                .map_err(|e| e.to_string())?;
                #[cfg(feature = "diagnostics")]
                if let Some(stats) = &mut statistics {
                    stats.record(&sample, job.diagnostic_target)?;
                }
                d += state.radiance.direct / spp as f64;
                c += state.radiance.caustics / spp as f64;
            }
            let direct = linear_srgb_to_xyz(d).ok_or("RGB integration overflow")?;
            let caustics = linear_srgb_to_xyz(c).ok_or("RGB integration overflow")?;
            if (direct + caustics).iter().any(|v| !v.is_finite()) {
                return Err("RGB integration overflow".into());
            }
            Ok(RenderedPixel {
                direct,
                caustics,
                #[cfg(feature = "diagnostics")]
                statistics: statistics.map(PathDebugAccumulator::finish),
            })
        },
    )
}

use crate::sampling::rng::{CAMERA_STREAM, DIFFUSE_STREAM, ENVIRONMENT_STREAM, pixel_seed};

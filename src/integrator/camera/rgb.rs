//! RGB pixel sampling. The path walk is shared with the spectral camera.
use super::{
    ground::bind_spectral_ground_lighting,
    lighting::camera_lighting_setup,
    path::trace_camera_path,
    transport::Caustics,
    types::{CameraLightingImages, CameraPathRngs, CameraPathState},
};
#[cfg(feature = "diagnostics")]
use crate::diagnostics::{PathDebugBuffer, PathDebugPixel, PathDebugSample};
use crate::{
    caustics::SpectralLighting, color::linear_srgb_to_xyz, config::RenderJob, film::Framebuffer,
};
use nalgebra::{Vector2, Vector3};
use rand::{Rng, SeedableRng, rngs::StdRng};
use rayon::prelude::*;
use std::error::Error;

/// Integrate RGB radiance; the optional map contributes only at the first non-delta receiver.
pub fn integrate_rgb(
    job: &RenderJob,
    settings: &crate::config::render::RenderSetting,
    lighting: Option<&SpectralLighting>,
) -> Result<CameraLightingImages, Box<dyn Error + Send + Sync>> {
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
    if spp == 0 {
        return Err("camera SPP must be positive".into());
    }
    let (light_wi, light) = camera_lighting_setup(job, width, height)?;
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

    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(spp))
        .and_then(|n| n.checked_mul(job.camera_options.environment_samples))
        .and_then(|n| {
            n.checked_mul(
                job.camera_options.max_diffuse_bounces + 1 + job.camera_options.max_volume_scatters,
            )
        })
        .ok_or("camera workload overflow")?;
    #[cfg(feature = "diagnostics")]
    if job
        .diagnostics
        .target
        .is_some_and(|id| id >= job.scene.objects.len())
    {
        return Err("invalid diagnostic target".into());
    }
    let mut images = CameraLightingImages {
        direct: Framebuffer::new(width, height)?,
        caustics: Framebuffer::new(width, height)?,
        combined: Framebuffer::new(width, height)?,
        #[cfg(feature = "diagnostics")]
        path_debug: debug_path.then(|| PathDebugBuffer::new(width, height, job.diagnostics.target)),
    };
    struct Pixel {
        direct: Vector3<f64>,
        caustics: Vector3<f64>,
        #[cfg(feature = "diagnostics")]
        statistics: Option<PathDebugPixel>,
    }
    let pixel = |index: usize| -> Result<Pixel, String> {
        let mut camera = StdRng::seed_from_u64(pixel_seed(seed, index, CAMERA_STREAM));
        let mut environment = StdRng::seed_from_u64(pixel_seed(seed, index, ENVIRONMENT_STREAM));
        let mut medium =
            StdRng::seed_from_u64(pixel_seed(seed, index, crate::sampling::rng::MEDIUM_STREAM));
        let mut continuation = StdRng::seed_from_u64(pixel_seed(seed, index, DIFFUSE_STREAM));
        let (mut d, mut c) = (Vector3::zeros(), Vector3::zeros());
        #[cfg(feature = "diagnostics")]
        let mut statistics = debug_path.then(PathDebugPixel::default);
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
                environment: &mut environment,
                diffuse: &mut continuation,
                medium: &mut medium,
            };
            #[cfg(feature = "diagnostics")]
            let mut sample = PathDebugSample::default();
            trace_camera_path(
                job,
                &mut state,
                &mut rngs,
                light_wi,
                light,
                ground.map_or(Caustics::None, Caustics::Rgb),
                crate::optics::dispersion::IorMode::Constant,
                #[cfg(feature = "diagnostics")]
                statistics.as_ref().map(|_| &mut sample),
            )
            .map_err(|e| e.to_string())?;
            #[cfg(feature = "diagnostics")]
            if let Some(stats) = &mut statistics {
                stats.record(&sample, job.diagnostics.target)?;
            }
            d += state.radiance.direct / spp as f64;
            c += state.radiance.caustics / spp as f64;
        }
        Ok(Pixel {
            direct: linear_srgb_to_xyz(d).ok_or("RGB integration overflow")?,
            caustics: linear_srgb_to_xyz(c).ok_or("RGB integration overflow")?,
            #[cfg(feature = "diagnostics")]
            statistics,
        })
    };
    let write =
        |(index, (d, c)): (usize, (&mut Vector3<f64>, &mut Vector3<f64>))| -> Result<(), String> {
            let p = pixel(index)?;
            *d = p.direct;
            *c = p.caustics;
            Ok(())
        };
    #[cfg(feature = "diagnostics")]
    if let Some(debug) = &mut images.path_debug {
        images
            .direct
            .pixels
            .par_iter_mut()
            .zip(images.caustics.pixels.par_iter_mut())
            .zip(debug.pixels.par_iter_mut())
            .enumerate()
            .try_for_each(|(i, ((d, c), stats))| -> Result<(), String> {
                let p = pixel(i)?;
                *d = p.direct;
                *c = p.caustics;
                *stats = p.statistics.expect("enabled observer");
                Ok(())
            })?;
    } else {
        images
            .direct
            .pixels
            .par_iter_mut()
            .zip(images.caustics.pixels.par_iter_mut())
            .enumerate()
            .try_for_each(write)?;
    }
    #[cfg(not(feature = "diagnostics"))]
    images
        .direct
        .pixels
        .par_iter_mut()
        .zip(images.caustics.pixels.par_iter_mut())
        .enumerate()
        .try_for_each(write)?;
    for ((d, c), combined) in images
        .direct
        .pixels
        .iter()
        .zip(&images.caustics.pixels)
        .zip(&mut images.combined.pixels)
    {
        *combined = d + c;
    }
    Ok(images)
}

use crate::sampling::rng::{CAMERA_STREAM, DIFFUSE_STREAM, ENVIRONMENT_STREAM, pixel_seed};

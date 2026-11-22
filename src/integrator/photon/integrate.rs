//! 基于 PhotonPlan 的逐波长追踪、标量辐照度层归约以及 XYZ / Eλ 焦散积分。

use super::{
    PhotonError, PhotonTracer,
    emission::{EmissionBlock, EmissionHit, PhotonPlan, deposit, event_rng_from_base},
    path::{PhotonPathState, PhotonSegment},
    photon_tracer,
};
use crate::{
    caustics::{SpectralCausticsMap, SpectralLighting},
    config::{RenderJob, render::RenderSetting},
    film::Framebuffer,
    geometry::hit::Hit,
    spectrum::cie_xyz,
};
use nalgebra::Vector3;
use rayon::prelude::*;

struct IrradianceLayer {
    direct: Option<Vec<f64>>,
    caustics: Option<Vec<f64>>,
}

fn trace_block_caustics(
    job: &RenderJob,
    plan: &PhotonPlan,
    source: &EmissionBlock,
    tracer: Option<&PhotonTracer>,
    nm: f64,
    caustics: &mut [f64],
) -> Result<(), PhotonError> {
    let transmittance = |segment: PhotonSegment| segment.transmittance(&job.scene, nm);
    let dimensions = [plan.width, plan.height];
    let receiver = &job.photon.receiver;
    for (landing, segments) in &source.paths.landings {
        let mut weight = landing.weight;
        for &segment in &source.paths.segments[segments.clone()] {
            weight *= transmittance(segment)?;
        }
        deposit(
            caustics,
            landing.position,
            plan.weight * weight,
            dimensions,
            receiver,
        )?;
    }
    if let Some(tracer) = tracer {
        let material_id = job.scene.objects[tracer.object_id].material_id();
        for &EmissionHit {
            index,
            segment,
            coords,
            normal,
            tr,
        } in &source.hits
        {
            let tr = match tr {
                Some(tr) => tr,
                None => transmittance(segment)?,
            };
            let hit = Hit {
                coords,
                normal,
                distance: segment.distance,
                object_id: tracer.object_id,
                material_id,
            };
            let state = PhotonPathState::new(segment.ray, segment.region, tracer.object_id, tr);
            let mut events = event_rng_from_base(&plan.base_events, index);
            let path = if plan.max_volume_scatters == 0 {
                tracer.trace_state(job, state, Some(hit), &mut events, transmittance)?
            } else {
                tracer.trace_state_with_volume(
                    job,
                    state,
                    Some(hit),
                    nm,
                    plan.max_volume_scatters,
                    &mut events,
                )?
            };
            if let Some(landing) = path.landing() {
                deposit(
                    caustics,
                    landing.position,
                    plan.weight * landing.weight,
                    dimensions,
                    receiver,
                )?;
            }
        }
    }
    Ok(())
}

fn trace_block_direct(
    job: &RenderJob,
    plan: &PhotonPlan,
    source: &EmissionBlock,
    nm: f64,
    values: &mut [f64],
) -> Result<(), PhotonError> {
    let transmittance = |segment: PhotonSegment| segment.transmittance(&job.scene, nm);
    let dimensions = [plan.width, plan.height];
    let receiver = &job.photon.receiver;
    for &(segment, position) in &source.spectral_direct {
        deposit(
            values,
            position,
            plan.weight * transmittance(segment)?,
            dimensions,
            receiver,
        )?;
    }
    Ok(())
}

fn irradiance_layer(
    job: &RenderJob,
    settings: &RenderSetting,
    plan: &PhotonPlan,
    nm: f64,
    need_caustics: bool,
    need_direct: bool,
    parallel: bool,
) -> Result<IrradianceLayer, PhotonError> {
    let count = plan.width * plan.height;
    let tracer = if need_caustics {
        photon_tracer(job, settings, plan.uncached_target, plan.ground, nm)?
    } else {
        None
    };
    let mut caustics = need_caustics.then(|| vec![0.0; count]);
    let mut direct = need_direct.then(|| {
        plan.fixed_direct
            .clone()
            .unwrap_or_else(|| vec![0.0; count])
    });
    if parallel {
        let parts: Vec<_> = plan
            .blocks
            .par_iter()
            .map(|source| -> Result<IrradianceLayer, PhotonError> {
                let block_caustics = if need_caustics
                    && (!source.paths.landings.is_empty()
                        || (tracer.is_some() && !source.hits.is_empty()))
                {
                    let mut buf = vec![0.0; count];
                    trace_block_caustics(job, plan, source, tracer.as_ref(), nm, &mut buf)?;
                    Some(buf)
                } else {
                    None
                };
                let block_direct = if need_direct && !source.spectral_direct.is_empty() {
                    let mut buf = vec![0.0; count];
                    trace_block_direct(job, plan, source, nm, &mut buf)?;
                    Some(buf)
                } else {
                    None
                };
                Ok(IrradianceLayer {
                    direct: block_direct,
                    caustics: block_caustics,
                })
            })
            .collect();
        for part in parts {
            let part = part?;
            if let (Some(dst), Some(src)) = (&mut caustics, part.caustics) {
                for (a, b) in dst.iter_mut().zip(src) {
                    *a += b;
                }
            }
            if let (Some(dst), Some(src)) = (&mut direct, part.direct) {
                for (a, b) in dst.iter_mut().zip(src) {
                    *a += b;
                }
            }
        }
    } else {
        let mut scratch = Vec::new();
        for source in &plan.blocks {
            if let Some(dst) = &mut caustics
                && (!source.paths.landings.is_empty()
                    || (tracer.is_some() && !source.hits.is_empty()))
            {
                if scratch.is_empty() {
                    scratch.resize(count, 0.0);
                } else {
                    scratch.fill(0.0);
                }
                trace_block_caustics(job, plan, source, tracer.as_ref(), nm, &mut scratch)?;
                for (a, &b) in dst.iter_mut().zip(&scratch) {
                    *a += b;
                }
            }
            if let Some(dst) = &mut direct
                && !source.spectral_direct.is_empty()
            {
                if scratch.is_empty() {
                    scratch.resize(count, 0.0);
                } else {
                    scratch.fill(0.0);
                }
                trace_block_direct(job, plan, source, nm, &mut scratch)?;
                for (a, &b) in dst.iter_mut().zip(&scratch) {
                    *a += b;
                }
            }
        }
    }
    Ok(IrradianceLayer { direct, caustics })
}

fn accumulate_xyz(
    frame: &mut Framebuffer,
    values: &[f64],
    xyz_per_unit: Vector3<f64>,
) -> Result<(), PhotonError> {
    for (pixel, &value) in frame.pixels.iter_mut().zip(values) {
        *pixel += xyz_per_unit * value;
        if !pixel.iter().all(|v| v.is_finite()) {
            return Err("photon XYZ accumulation overflow".into());
        }
    }
    Ok(())
}

pub fn integrate_xyz(
    job: &RenderJob,
    target: Option<usize>,
    ground: usize,
    settings: &RenderSetting,
) -> Result<SpectralLighting, PhotonError> {
    integrate_xyz_impl(job, target, ground, settings, true)
}

pub(crate) fn integrate_camera_xyz(
    job: &RenderJob,
    target: usize,
    ground: usize,
    settings: &RenderSetting,
) -> Result<SpectralLighting, PhotonError> {
    integrate_xyz_impl(job, Some(target), ground, settings, false)
}

fn integrate_xyz_impl(
    job: &RenderJob,
    target: Option<usize>,
    ground: usize,
    settings: &RenderSetting,
    direct: bool,
) -> Result<SpectralLighting, PhotonError> {
    let plan = PhotonPlan::new(job, settings, target, ground, direct)?;
    let mut caustics = Framebuffer::new(plan.width, plan.height)?;
    let mut direct_frame = direct
        .then(|| Framebuffer::new(plan.width, plan.height))
        .transpose()?;
    let white = job
        .scene
        .light
        .white_xyz(&plan.nodes)
        .ok_or("source XYZ overflow")?;
    let spectral_caustics = plan.spectral_caustics;
    let spectral_direct = direct && plan.spectral_extinction;
    if !spectral_caustics && let Some(fixed) = &plan.fixed_caustics {
        accumulate_xyz(&mut caustics, fixed, white)?;
    }
    if direct
        && !spectral_direct
        && let (Some(frame), Some(fixed)) = (&mut direct_frame, &plan.fixed_direct)
    {
        accumulate_xyz(frame, fixed, white)?;
    }
    if spectral_caustics || spectral_direct {
        if plan.nodes.len() == 1 {
            let (nm, width) = plan.nodes[0];
            let layer = irradiance_layer(
                job,
                settings,
                &plan,
                nm,
                spectral_caustics,
                spectral_direct,
                true,
            )?;
            let xyz =
                cie_xyz(nm).expect("validated wavelength") * (plan.source_density(nm) * width);
            if let Some(values) = &layer.caustics {
                accumulate_xyz(&mut caustics, values, xyz)?;
            }
            if let (Some(frame), Some(values)) = (&mut direct_frame, &layer.direct) {
                accumulate_xyz(frame, values, xyz)?;
            }
        } else {
            // Parallelize across wavelength chunks while accumulating XYZ in fixed node order.
            for chunk in plan.nodes.chunks(8) {
                let batch: Result<Vec<_>, String> = chunk
                    .par_iter()
                    .map(|&(nm, width)| {
                        let layer = irradiance_layer(
                            job,
                            settings,
                            &plan,
                            nm,
                            spectral_caustics,
                            spectral_direct,
                            false,
                        )
                        .map_err(|e| e.to_string())?;
                        Ok((nm, width, layer))
                    })
                    .collect();
                for (nm, width, layer) in batch? {
                    let xyz = cie_xyz(nm).expect("validated wavelength")
                        * (plan.source_density(nm) * width);
                    if let Some(values) = &layer.caustics {
                        accumulate_xyz(&mut caustics, values, xyz)?;
                    }
                    if let (Some(frame), Some(values)) = (&mut direct_frame, &layer.direct) {
                        accumulate_xyz(frame, values, xyz)?;
                    }
                }
            }
        }
    }
    let (light_wi, strength) = job
        .scene
        .light
        .analytic_sun()?
        .unwrap_or((Vector3::z(), 0.0));
    Ok(SpectralLighting {
        direct: direct_frame,
        caustics,
        receiver: job.photon.receiver,
        light_wi,
        sun_irradiance_xyz: white * strength,
    })
}

pub fn integrate_spectral(
    job: &RenderJob,
    settings: &RenderSetting,
) -> Result<Option<SpectralCausticsMap>, PhotonError> {
    let Some(target) = job.photon.caustics_object_id else {
        return Ok(None);
    };
    let ground = job
        .photon
        .ground_id
        .ok_or("spectral caustics require ground")?;
    settings.validate()?;
    if ground >= job.scene.objects.len() || target == ground || target >= job.scene.objects.len() {
        return Err("invalid photon target or receiver".into());
    }
    if !job.supports_ideal_caustics(target) {
        return Ok(None);
    }
    let material = job.scene.material(job.scene.objects[ground].material_id());
    if material.irradiance_reflectance().is_none() {
        return Err("caustics require a Lambertian receiver".into());
    }
    let nodes = settings.wavelength_samples()?;
    settings
        .width
        .checked_mul(settings.height)
        .and_then(|n| n.checked_mul(nodes.len()))
        .and_then(|n| n.checked_mul(size_of::<f64>()))
        .filter(|&n| n <= isize::MAX as usize)
        .ok_or("atlas size overflow")?;
    let plan = PhotonPlan::new(job, settings, Some(target), ground, false)?;
    let layers: Result<Vec<_>, String> = plan
        .nodes
        .par_iter()
        .map(|&(nm, _)| {
            let mut layer = if let Some(g) = &plan.fixed_caustics {
                g.clone()
            } else {
                irradiance_layer(job, settings, &plan, nm, true, false, false)
                    .map_err(|e| e.to_string())?
                    .caustics
                    .expect("caustics requested")
            };
            let density = plan.source_density(nm);
            for value in &mut layer {
                *value *= density;
                if !value.is_finite() {
                    return Err("spectral irradiance overflow".into());
                }
            }
            Ok(layer)
        })
        .collect();
    Ok(Some(SpectralCausticsMap {
        width: plan.width,
        height: plan.height,
        receiver: job.photon.receiver,
        ground_id: ground,
        mode: settings.mode,
        samples: plan.nodes,
        layers: layers?,
    }))
}

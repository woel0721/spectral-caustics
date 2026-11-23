//! 基于 PhotonPlan 的逐波长追踪、标量辐照度层归约以及 XYZ / Eλ 焦散积分。

use super::{
    PhotonError, PhotonTracer,
    emission::{
        BLOCK_SCRATCH_BUDGET_BYTES, EmissionBlock, EmissionHit, PhotonPlan, accumulate_irradiance,
        alloc_zeroed_irradiance, block_batch_size, deposit, event_rng_from_base,
        irradiance_batch_size, irradiance_buffer_bytes, prepare_scratch_buffer,
    },
    path::{PhotonPathState, PhotonSegment},
    photon_tracer,
};
use crate::{
    caustics::{SpectralCausticsMap, SpectralLighting, map::SpectralAtlasStorage},
    config::{RenderJob, render::RenderSetting},
    film::Framebuffer,
    geometry::hit::Hit,
    spectrum::cie_xyz,
};
use nalgebra::Vector3;
use rayon::prelude::*;

pub(super) struct IrradianceLayer {
    pub(super) direct: Option<Vec<f64>>,
    pub(super) caustics: Option<Vec<f64>>,
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
            let mut events = event_rng_from_base(plan.base_seed, index);
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

impl IrradianceLayer {
    fn new(count: usize, caustics: bool, direct: bool) -> Result<Self, PhotonError> {
        Ok(Self {
            caustics: caustics
                .then(|| alloc_zeroed_irradiance(count))
                .transpose()?,
            direct: direct.then(|| alloc_zeroed_irradiance(count)).transpose()?,
        })
    }
}

#[derive(Default)]
struct BlockScratch {
    values: Option<Vec<f64>>,
    written: bool,
}

fn trace_blocks(
    plan: &PhotonPlan,
    scratch: &mut [BlockScratch],
    layer: &mut Option<Vec<f64>>,
    contributes: impl Fn(&EmissionBlock) -> bool + Sync,
    trace: impl Fn(&EmissionBlock, &mut [f64]) -> Result<(), PhotonError> + Sync,
) -> Result<(), PhotonError> {
    let trace_slot = |source: &EmissionBlock, slot: &mut BlockScratch| {
        slot.written = false;
        if contributes(source) {
            let values = prepare_scratch_buffer(
                &mut slot.values,
                &mut slot.written,
                plan.width * plan.height,
            )?;
            trace(source, values)?;
        }
        Ok::<_, PhotonError>(())
    };
    let mut reduce = |slot: &BlockScratch| -> Result<(), PhotonError> {
        if slot.written {
            accumulate_irradiance(layer, slot.values.as_deref().expect("written scratch"))?;
        }
        Ok(())
    };
    if let [slot] = scratch {
        for source in &plan.blocks {
            trace_slot(source, slot)?;
            reduce(slot)?;
        }
    } else {
        for chunk in plan.blocks.chunks(scratch.len()) {
            let slots = &mut scratch[..chunk.len()];
            chunk
                .par_iter()
                .zip(slots.par_iter_mut())
                .try_for_each(|(source, slot)| trace_slot(source, slot))?;
            for slot in slots {
                reduce(slot)?;
            }
        }
    }
    Ok(())
}

fn trace_wavelength(
    job: &RenderJob,
    settings: &RenderSetting,
    plan: &PhotonPlan,
    nm: f64,
    layer: &mut IrradianceLayer,
    scratch: &mut [BlockScratch],
) -> Result<(), PhotonError> {
    // The components are independent and share the same scratch pool.
    if let Some(values) = &mut layer.caustics {
        values.fill(0.0);
        let tracer = photon_tracer(job, settings, plan.uncached_target, plan.ground, nm)?;
        trace_blocks(
            plan,
            scratch,
            &mut layer.caustics,
            |source| {
                !source.paths.landings.is_empty() || (tracer.is_some() && !source.hits.is_empty())
            },
            |source, values| trace_block_caustics(job, plan, source, tracer.as_ref(), nm, values),
        )?;
    }
    if let Some(values) = &mut layer.direct {
        match &plan.fixed_direct {
            Some(fixed) => values.copy_from_slice(fixed),
            None => values.fill(0.0),
        }
        trace_blocks(
            plan,
            scratch,
            &mut layer.direct,
            |source| !source.spectral_direct.is_empty(),
            |source, values| trace_block_direct(job, plan, source, nm, values),
        )?;
    }
    Ok(())
}

pub(super) fn xyz_block_batch_size(
    total_blocks: usize,
    pixels: usize,
    active_layers: usize,
    threads: usize,
) -> usize {
    let remaining_budget =
        BLOCK_SCRATCH_BUDGET_BYTES.saturating_sub(irradiance_buffer_bytes(pixels, active_layers));
    irradiance_batch_size(total_blocks, pixels, 1, remaining_budget, threads).max(1)
}

fn scratch_pool(batch: usize) -> Vec<BlockScratch> {
    (0..batch).map(|_| BlockScratch::default()).collect()
}

#[cfg(test)]
pub(super) fn irradiance_layer(
    job: &RenderJob,
    settings: &RenderSetting,
    plan: &PhotonPlan,
    nm: f64,
    need_caustics: bool,
    need_direct: bool,
    parallel: bool,
) -> Result<IrradianceLayer, PhotonError> {
    let count = plan.width * plan.height;
    let batch = if parallel {
        block_batch_size(plan.blocks.len(), count, 1).max(1)
    } else {
        1
    };
    let mut layer = IrradianceLayer::new(count, need_caustics, need_direct)?;
    trace_wavelength(
        job,
        settings,
        plan,
        nm,
        &mut layer,
        &mut scratch_pool(batch),
    )?;
    Ok(layer)
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
    let active_layers = usize::from(spectral_caustics) + usize::from(spectral_direct);
    if active_layers > 0 {
        let count = plan.width * plan.height;
        let threads = rayon::current_num_threads();
        let wavelength_batch = irradiance_batch_size(
            plan.nodes.len(),
            count,
            active_layers + 1,
            BLOCK_SCRATCH_BUDGET_BYTES,
            threads,
        );
        let mut accumulate_layer = |(nm, width), layer: &IrradianceLayer| {
            let xyz =
                cie_xyz(nm).expect("validated wavelength") * (plan.source_density(nm) * width);
            if let Some(values) = &layer.caustics {
                accumulate_xyz(&mut caustics, values, xyz)?;
            }
            if let (Some(frame), Some(values)) = (&mut direct_frame, &layer.direct) {
                accumulate_xyz(frame, values, xyz)?;
            }
            Ok::<_, PhotonError>(())
        };
        if wavelength_batch >= 2 {
            let mut slots = (0..wavelength_batch)
                .map(|_| {
                    Ok((
                        IrradianceLayer::new(count, spectral_caustics, spectral_direct)?,
                        BlockScratch::default(),
                    ))
                })
                .collect::<Result<Vec<_>, PhotonError>>()?;
            for chunk in plan.nodes.chunks(wavelength_batch) {
                let slots = &mut slots[..chunk.len()];
                chunk.par_iter().zip(slots.par_iter_mut()).try_for_each(
                    |(&(nm, _), (layer, scratch))| {
                        trace_wavelength(
                            job,
                            settings,
                            &plan,
                            nm,
                            layer,
                            std::slice::from_mut(scratch),
                        )
                    },
                )?;
                for (&node, (layer, _)) in chunk.iter().zip(slots) {
                    accumulate_layer(node, layer)?;
                }
            }
        } else {
            let block_batch =
                xyz_block_batch_size(plan.blocks.len(), count, active_layers, threads);
            let mut scratch = scratch_pool(block_batch);
            let mut layer = IrradianceLayer::new(count, spectral_caustics, spectral_direct)?;
            for &node in &plan.nodes {
                trace_wavelength(job, settings, &plan, node.0, &mut layer, &mut scratch)?;
                accumulate_layer(node, &layer)?;
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
    let plan = PhotonPlan::new_atlas(job, settings, target, ground)?;
    let count = plan.width * plan.height;
    let storage = if plan.spectral_caustics {
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(plan.nodes.len())
            .map_err(|_| "cannot allocate photon irradiance buffer")?;
        // Atlas layers are returned to the caller. Only the scratch pool is temporary.
        let wavelength_batch = irradiance_batch_size(
            plan.nodes.len(),
            count,
            1,
            BLOCK_SCRATCH_BUDGET_BYTES,
            rayon::current_num_threads(),
        );
        let trace_layer = |nm, scratch: &mut [BlockScratch]| {
            let mut layer = IrradianceLayer::new(count, true, false)?;
            trace_wavelength(job, settings, &plan, nm, &mut layer, scratch)?;
            let mut values = layer.caustics.expect("caustics requested");
            scale_irradiance(&mut values, plan.source_density(nm))?;
            Ok::<_, PhotonError>(values)
        };
        if wavelength_batch >= 2 {
            let mut scratch = scratch_pool(wavelength_batch);
            for chunk in plan.nodes.chunks(wavelength_batch) {
                let batch: Result<Vec<_>, PhotonError> = chunk
                    .par_iter()
                    .zip(scratch[..chunk.len()].par_iter_mut())
                    .map(|(&(nm, _), slot)| trace_layer(nm, std::slice::from_mut(slot)))
                    .collect();
                layers.extend(batch?);
            }
        } else {
            let mut scratch = scratch_pool(block_batch_size(plan.blocks.len(), count, 1).max(1));
            for &(nm, _) in &plan.nodes {
                layers.push(trace_layer(nm, &mut scratch)?);
            }
        }
        SpectralAtlasStorage::PerWavelength(layers)
    } else {
        let mut node_densities = Vec::new();
        node_densities
            .try_reserve_exact(plan.nodes.len())
            .map_err(|_| "cannot allocate spectral node densities")?;
        node_densities.extend(plan.nodes.iter().map(|&(nm, _)| plan.source_density(nm)));
        if let Some(base) = &plan.fixed_caustics {
            let max_density = node_densities.iter().copied().fold(0.0, f64::max);
            if base.iter().any(|&value| !(value * max_density).is_finite()) {
                return Err("spectral irradiance overflow".into());
            }
        }
        SpectralAtlasStorage::ScaledSingle {
            base: plan.fixed_caustics,
            node_densities,
        }
    };
    Ok(Some(SpectralCausticsMap {
        width: plan.width,
        height: plan.height,
        receiver: job.photon.receiver,
        ground_id: ground,
        mode: settings.mode,
        samples: plan.nodes,
        storage,
    }))
}

fn scale_irradiance(values: &mut [f64], density: f64) -> Result<(), PhotonError> {
    for value in values {
        *value *= density;
        if !value.is_finite() {
            return Err("spectral irradiance overflow".into());
        }
    }
    Ok(())
}

//! One bounded camera walk for RGB and hero-wavelength radiance.
use super::{
    lighting::area::AreaLightSampler,
    lighting::environment::environment_lighting,
    lighting::visible_to_light,
    offset_ray_origin,
    transport::{CameraTransport, Caustics},
    types::{CameraPathRngs, CameraPathState, ScatterRay},
};
use crate::{
    config::RenderJob,
    geometry::{hit::Hit, ray::Ray},
    material::{Material, unit_vector},
    optics::dispersion::IorMode,
};
use nalgebra::Vector3;
use rand::Rng;
use std::error::Error;
#[cfg_attr(feature = "diagnostics", derive(serde::Serialize))]
#[cfg_attr(feature = "diagnostics", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]

pub enum StopReason {
    EnvironmentEscape,
    BackgroundEscape,
    NoScatteringComponents,
    Backface,
    SampleRejected,
    RussianRoulette,
    NonDeltaBudgetExhausted,
    DeltaBudgetExhausted,
    AllBudgetsExhausted,
    ZeroThroughput,
}

pub const MAX_DELTA_EVENTS: usize = 32;

pub fn russian_roulette<T: CameraTransport>(
    state: &mut CameraPathState<T>,
    rng: &mut dyn rand::RngCore,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    if state.non_delta_bounces < 3 || state.non_delta_bounces >= state.max_non_delta_bounces {
        return Ok(true);
    }
    if !state.transport.valid_weight(state.throughput) {
        return Err("invalid RR throughput".into());
    }
    if state.transport.importance(state.throughput) == 0.0 {
        return Ok(false);
    }
    let p = state
        .transport
        .importance(state.throughput)
        .clamp(0.05, 0.95);
    if rng.random::<f64>() >= p {
        return Ok(false);
    }
    state.throughput = state.throughput / p;
    if !state.transport.finite(state.throughput) {
        return Err("RR compensation overflow".into());
    }
    Ok(true)
}

pub fn sample_scatter_ray<T: CameraTransport>(
    job: &RenderJob,
    hit: &Hit,
    m: &dyn Material,
    wo: Vector3<f64>,
    transport: T,
    ior: IorMode,
    rng: &mut dyn rand::RngCore,
) -> Result<Option<ScatterRay<T::Value>>, Box<dyn Error + Send + Sync>> {
    let normal = unit_vector(hit.normal).ok_or("invalid surface normal")?;
    let Some(sample) = transport.sample(m, wo, normal, ior, rng) else {
        return Ok(None);
    };
    if sample.event.is_delta() != sample.pdf.is_none()
        || sample.pdf.is_some_and(|p| !p.is_finite() || p <= 0.0)
        || !transport.valid_weight(sample.weight)
    {
        return Err("invalid BSDF event, density or weight".into());
    }
    if sample.event.is_delta() && !m.has_delta() || !sample.event.is_delta() && !m.has_non_delta() {
        return Err("BSDF event contradicts declared components".into());
    }
    let wi = unit_vector(sample.wi).ok_or("invalid BSDF direction")?;
    if !sample.event.is_delta() && (normal.dot(&wi) <= 0.0 || normal.dot(&wo) <= 0.0) {
        return Ok(None);
    }
    let weight = sample.weight
        * job
            .ground_reflectance_scale(hit)
            .ok_or("invalid checkerboard coordinate")?;
    if weight == T::Value::default() && !sample.event.is_delta() {
        return Ok(None);
    }
    let side = if normal.dot(&wi) >= 0.0 { 1.0 } else { -1.0 };
    let ray =
        Ray::new(offset_ray_origin(hit.coords, normal, side), wi).ok_or("surface ray overflow")?;
    Ok(Some(ScatterRay {
        ray,
        throughput: weight,
        event: sample.event,
    }))
}

#[allow(clippy::too_many_arguments)]
pub fn trace_camera_path<T: CameraTransport>(
    job: &RenderJob,
    state: &mut CameraPathState<T>,
    rngs: &mut CameraPathRngs<'_>,
    light_wi: Vector3<f64>,
    light: f64,
    map: Caustics<'_>,
    ior: IorMode,
    #[cfg(feature = "diagnostics")] mut observer: Option<&mut crate::diagnostics::PathDebugSample>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    state.validate()?;
    let transport = state.transport;
    let (sun_wi, sun) = transport.sunlight(light_wi, light, map)?;
    let mut area = AreaLightSampler::new(job, rngs.camera);
    let _termination = loop {
        if state.throughput == T::Value::default() {
            break StopReason::ZeroThroughput;
        }
        let Some(hit) = job.scene.intersect(&state.ray) else {
            // Local environment sampling already owns non-delta escape. Only a subsequent
            // delta chain sees the environment again; constant background is camera-only.
            let emitted = if !state.has_non_delta_vertex && state.non_delta_bounces == 0 {
                Some(
                    job.scene
                        .background_radiance(state.ray.direction)
                        .ok_or("invalid background")?,
                )
            } else if state.last_scatter.is_some_and(|e| e.is_delta()) {
                job.scene
                    .environment
                    .as_ref()
                    .map(|e| {
                        e.radiance(state.ray.direction)
                            .ok_or("environment overflow")
                    })
                    .transpose()?
            } else {
                None
            };
            if let Some(rgb) = emitted {
                state.radiance.direct += transport.multiply(
                    state.throughput,
                    transport
                        .emission(rgb)
                        .ok_or("emission reconstruction overflow")?,
                );
            }
            #[cfg(feature = "diagnostics")]
            if let Some(o) = observer.as_deref_mut() {
                o.escaped_environment = job.scene.environment.is_some();
            }
            break if job.scene.environment.is_some() {
                StopReason::EnvironmentEscape
            } else {
                StopReason::BackgroundEscape
            };
        };
        #[cfg(feature = "diagnostics")]
        if let Some(o) = observer.as_deref_mut() {
            o.hit(&hit, job.photon.ground_id);
        }
        let material = job.scene.material(hit.material_id);
        let wo = -state.ray.direction;
        if material.has_non_delta() {
            let normal = unit_vector(hit.normal).ok_or("invalid normal")?;
            if normal.dot(&wo) <= 0.0 && !material.has_delta() {
                break StopReason::Backface;
            }
            let scale = job
                .ground_reflectance_scale(&hit)
                .ok_or("invalid checkerboard coordinate")?;
            let f = transport.evaluate(material, sun_wi, wo, normal);
            if !transport.valid_weight(f) {
                return Err("invalid direct BSDF".into());
            }
            let cosine = normal.dot(&sun_wi).max(0.0);
            #[cfg(feature = "diagnostics")]
            let disable =
                state.debug_disable_ground_sun && Some(hit.object_id) == job.photon.ground_id;
            #[cfg(not(feature = "diagnostics"))]
            let disable = false;
            let mut direct = T::Value::default();
            if !disable
                && cosine > 0.0
                && f != T::Value::default()
                && sun != T::Value::default()
                && visible_to_light(job, hit.coords, normal, sun_wi)?
            {
                direct = transport.multiply(f, sun) * cosine;
            }
            let env_rng = if state.has_non_delta_vertex || state.non_delta_bounces > 0 {
                &mut *rngs.diffuse
            } else {
                &mut *rngs.environment
            };
            direct += environment_lighting(job, &hit, material, wo, env_rng, transport, ior)?;
            direct += area.lighting(job, &hit, material, wo, transport)?;
            state.radiance.direct += transport.multiply(state.throughput, direct) * scale;
            if !state.has_non_delta_vertex && state.non_delta_bounces == 0 {
                state.radiance.caustics += transport.multiply(
                    state.throughput,
                    transport.caustics(&hit, material, wo, map)?,
                ) * scale;
            }
        }
        state.has_non_delta_vertex |= material.has_non_delta();
        let continuous_budget = state.non_delta_bounces < state.max_non_delta_bounces;
        let delta_budget = state.delta_events() < MAX_DELTA_EVENTS;
        if (!continuous_budget || !material.has_non_delta())
            && (!delta_budget || !material.has_delta())
        {
            break if !material.has_non_delta() && !material.has_delta() {
                StopReason::NoScatteringComponents
            } else if material.has_non_delta() && material.has_delta() {
                StopReason::AllBudgetsExhausted
            } else if material.has_non_delta() {
                StopReason::NonDeltaBudgetExhausted
            } else {
                StopReason::DeltaBudgetExhausted
            };
        }
        if material.has_non_delta() && continuous_budget && !russian_roulette(state, rngs.diffuse)?
        {
            break StopReason::RussianRoulette;
        }
        let rng = if !state.has_non_delta_vertex && state.non_delta_bounces == 0 {
            &mut *rngs.camera
        } else {
            &mut *rngs.diffuse
        };
        let Some(next) = sample_scatter_ray(job, &hit, material, wo, transport, ior, rng)? else {
            break StopReason::SampleRejected;
        };
        if next.event.is_delta() && !delta_budget || !next.event.is_delta() && !continuous_budget {
            break if next.event.is_delta() {
                StopReason::DeltaBudgetExhausted
            } else {
                StopReason::NonDeltaBudgetExhausted
            };
        }
        let continues = state.apply_scatter(next.event, next.throughput)?;
        #[cfg(feature = "diagnostics")]
        if let Some(o) = observer.as_deref_mut() {
            o.scatter(next.event, material, wo, hit.normal);
        }
        if !continues {
            break StopReason::ZeroThroughput;
        }
        state.ray = next.ray;
    };
    #[cfg(feature = "diagnostics")]
    if let Some(o) = observer {
        o.termination = Some(_termination);
    }
    if !transport.finite(state.radiance.direct)
        || !transport.finite(state.radiance.caustics)
        || !transport.finite(state.combined_radiance())
    {
        return Err("path radiance overflow".into());
    }
    Ok(())
}

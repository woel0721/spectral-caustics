//! 光子发射分块、首交缓存、静态透射/单遍预计算与标量沉积。

use super::{
    PhotonError, PhotonLanding, PhotonTracer,
    path::{PhotonPathState, PhotonSegment},
    photon_tracer,
};
use crate::{
    caustics::{GroundReceiver, map::ground_splat_weights},
    config::{RenderJob, render::RenderSetting},
    geometry::{hit::Hit, ray::Ray},
    medium::ACHROMATIC_QUERY_NM,
    scene::MediumRegion,
    spectrum::d65_spd,
};
use nalgebra::Vector3;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;
use rayon::prelude::*;
use std::ops::Range;

pub(super) struct EmissionHit {
    pub(super) index: usize,
    pub(super) segment: PhotonSegment,
    pub(super) coords: Vector3<f64>,
    pub(super) normal: Vector3<f64>,
    // Only achromatic first segments have a cached transmittance.
    pub(super) tr: Option<f64>,
}

pub(super) struct EmissionBlock {
    pub(super) hits: Vec<EmissionHit>,
    pub(super) paths: CachedPaths,
    pub(super) spectral_direct: Vec<(PhotonSegment, Vector3<f64>)>,
}

#[derive(Default)]
pub(super) struct CachedPaths {
    pub(super) landings: Vec<(PhotonLanding, Range<usize>)>,
    pub(super) segments: Vec<PhotonSegment>,
}

impl CachedPaths {
    fn trace(
        &mut self,
        job: &RenderJob,
        tracer: &PhotonTracer,
        segment: PhotonSegment,
        hit: Hit,
        rng: &mut ChaCha12Rng,
    ) -> Result<(), &'static str> {
        let start = self.segments.len();
        let mut record = |segment: PhotonSegment| {
            if let Some(tr) = fixed_transmittance(job, segment)? {
                return Ok(tr);
            }
            self.segments
                .try_reserve(1)
                .map_err(|_| "cannot allocate spectral photon segments")?;
            self.segments.push(segment);
            Ok(1.0)
        };
        let tr = record(segment)?;
        let state = PhotonPathState::new(segment.ray, segment.region, tracer.object_id, tr);
        if let Some(landing) = tracer
            .trace_state(job, state, Some(hit), rng, record)?
            .landing()
        {
            self.landings
                .try_reserve(1)
                .map_err(|_| "cannot allocate cached photon landings")?;
            self.landings.push((landing, start..self.segments.len()));
        } else {
            self.segments.truncate(start);
        }
        Ok(())
    }
}

pub(super) struct PhotonPlan {
    pub(super) blocks: Vec<EmissionBlock>,
    pub(super) fixed_direct: Option<Vec<f64>>,
    pub(super) fixed_caustics: Option<Vec<f64>>,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) weight: f64,
    pub(super) nodes: Vec<(f64, f64)>,
    pub(super) normalization: f64,
    pub(super) spectral_extinction: bool,
    pub(super) spectral_caustics: bool,
    pub(super) uncached_target: Option<usize>,
    pub(super) ground: usize,
    pub(super) max_volume_scatters: usize,
    pub(super) base_events: ChaCha12Rng,
}

struct PreparedPhotonPlan {
    target: Option<usize>,
    ground: usize,
    nx: usize,
    ny: usize,
    pixels: usize,
    weight: f64,
    nodes: Vec<(f64, f64)>,
    normalization: f64,
    spectral_extinction: bool,
    spectral_caustics: bool,
    cache_spectral_segments: bool,
    single_pass_caustics: bool,
}

impl PhotonPlan {
    pub(super) fn source_density(&self, nm: f64) -> f64 {
        self.normalization * d65_spd(nm).expect("embedded spectrum")
    }

    fn validate_and_prepare(
        job: &RenderJob,
        settings: &RenderSetting,
        target: Option<usize>,
        ground: usize,
    ) -> Result<PreparedPhotonPlan, PhotonError> {
        settings.validate()?;
        if ground >= job.scene.objects.len()
            || target.is_some_and(|id| id == ground || id >= job.scene.objects.len())
        {
            return Err("invalid photon target or receiver".into());
        }
        let target = target.filter(|&id| job.supports_ideal_caustics(id));
        job.scene.light.validate_emission()?;
        let (_, area) = job.photon.receiver.checked_area()?;
        let nx = settings
            .width
            .checked_mul(settings.samples_per_pixel_axis)
            .ok_or("emission grid overflow")?;
        let ny = settings
            .height
            .checked_mul(settings.samples_per_pixel_axis)
            .ok_or("emission grid overflow")?;
        let count = nx.checked_mul(ny).ok_or("photon count overflow")?;
        let pixels = settings
            .width
            .checked_mul(settings.height)
            .ok_or("map size overflow")?;
        let weight = job.scene.light.total_flux() / count as f64 / (area / pixels as f64);
        if !weight.is_finite() {
            return Err("photon normalization overflow".into());
        }
        let nodes = settings.wavelength_samples()?;
        let normalization = job
            .scene
            .light
            .spectral_normalization(&nodes)
            .ok_or("invalid source spectrum")?;
        let spectral_extinction = job.scene.has_wavelength_dependent_medium_coefficients();
        let volume_enabled = settings.photon_max_volume_scatters > 0;
        let dispersive_target =
            target.is_some_and(|id| job.has_wavelength_dependent_ior(id, settings.mode));
        let spectral_caustics = target.is_some()
            && (dispersive_target
                || spectral_extinction
                || (volume_enabled && job.scene.has_wavelength_dependent_medium()));
        // Extinction alone can be replayed on fixed geometry. With volume
        // sampling, spectral coefficients or g can change the path itself.
        let cache_spectral_segments =
            target.is_some() && spectral_extinction && !dispersive_target && !volume_enabled;
        let single_pass_caustics = target.is_some() && !spectral_caustics;
        Ok(PreparedPhotonPlan {
            target,
            ground,
            nx,
            ny,
            pixels,
            weight,
            nodes,
            normalization,
            spectral_extinction,
            spectral_caustics,
            cache_spectral_segments,
            single_pass_caustics,
        })
    }

    pub(super) fn new(
        job: &RenderJob,
        settings: &RenderSetting,
        target: Option<usize>,
        ground: usize,
        direct: bool,
    ) -> Result<Self, PhotonError> {
        let prepared = Self::validate_and_prepare(job, settings, target, ground)?;
        let fixed_tracer = if prepared.cache_spectral_segments || prepared.single_pass_caustics {
            photon_tracer(
                job,
                settings,
                prepared.target,
                prepared.ground,
                prepared.nodes[0].0,
            )?
        } else {
            None
        };
        Self::emit_blocks(job, settings, prepared, direct, fixed_tracer)
    }

    fn emit_blocks(
        job: &RenderJob,
        settings: &RenderSetting,
        prepared: PreparedPhotonPlan,
        direct: bool,
        fixed_tracer: Option<PhotonTracer>,
    ) -> Result<Self, PhotonError> {
        let PreparedPhotonPlan {
            target,
            ground,
            nx,
            ny,
            pixels,
            weight,
            nodes,
            normalization,
            spectral_extinction,
            spectral_caustics,
            cache_spectral_segments,
            single_pass_caustics,
        } = prepared;
        let dimensions = [settings.width, settings.height];
        let receiver = &job.photon.receiver;
        let base_events = ChaCha12Rng::seed_from_u64(settings.seed);
        let raw_blocks: Result<Vec<_>, PhotonError> = spectral_sample_blocks(
            nx,
            ny,
            settings.seed,
            settings.max_spectral_blocks,
            job.scene.light.emission_dimensions(),
        )
        .into_par_iter()
        .map(|(rows, mut rng)| {
            let mut block = EmissionBlock {
                hits: Vec::new(),
                paths: CachedPaths::default(),
                spectral_direct: Vec::new(),
            };
            let mut direct_fixed: Option<Vec<f64>> = None;
            let mut caustics_fixed: Option<Vec<f64>> = None;
            for_each_emitted_hit(job, nx, ny, rows, &mut rng, |index, ray, hit, region| {
                let segment = PhotonSegment {
                    ray,
                    region,
                    distance: hit.distance,
                };
                if hit.object_id == ground {
                    if direct && hit.is_front_face(ray.direction) {
                        if let Some(tr) = fixed_transmittance(job, segment)? {
                            let buf = direct_fixed.get_or_insert_with(|| vec![0.0; pixels]);
                            deposit(buf, hit.coords, weight * tr, dimensions, receiver)?;
                        } else {
                            block
                                .spectral_direct
                                .try_reserve(1)
                                .map_err(|_| "cannot allocate spectral direct hits")?;
                            block.spectral_direct.push((segment, hit.coords));
                        }
                    }
                } else if Some(hit.object_id) == target {
                    if cache_spectral_segments {
                        let tracer = fixed_tracer.as_ref().expect("cached tracer");
                        let mut events = event_rng_from_base(&base_events, index);
                        block.paths.trace(job, tracer, segment, hit, &mut events)?;
                    } else if single_pass_caustics {
                        let tracer = fixed_tracer.as_ref().expect("single-pass tracer");
                        let tr = fixed_transmittance(job, segment)?.expect("achromatic segment");
                        let state =
                            PhotonPathState::new(segment.ray, segment.region, tracer.object_id, tr);
                        let mut events = event_rng_from_base(&base_events, index);
                        let path = if settings.photon_max_volume_scatters == 0 {
                            tracer.trace_state(job, state, Some(hit), &mut events, |seg| {
                                seg.transmittance(&job.scene, ACHROMATIC_QUERY_NM)
                            })?
                        } else {
                            tracer
                                .trace_state_with_volume(
                                    job,
                                    state,
                                    Some(hit),
                                    ACHROMATIC_QUERY_NM,
                                    settings.photon_max_volume_scatters,
                                    &mut events,
                                )
                                .map_err(|e| -> PhotonError { e.into() })?
                        };
                        if let Some(landing) = path.landing() {
                            let buf = caustics_fixed.get_or_insert_with(|| vec![0.0; pixels]);
                            deposit(
                                buf,
                                landing.position,
                                weight * landing.weight,
                                dimensions,
                                receiver,
                            )?;
                        }
                    } else {
                        block
                            .hits
                            .try_reserve(1)
                            .map_err(|_| "cannot allocate photon hits")?;
                        block.hits.push(EmissionHit {
                            index,
                            segment,
                            coords: hit.coords,
                            normal: hit.normal,
                            tr: fixed_transmittance(job, segment)?,
                        });
                    }
                }
                Ok(())
            })?;
            Ok((block, direct_fixed, caustics_fixed))
        })
        .collect();
        let raw_blocks = raw_blocks?;
        let mut blocks = Vec::with_capacity(raw_blocks.len());
        let mut fixed_direct = direct.then(|| vec![0.0; pixels]);
        let mut fixed_caustics = (!spectral_caustics).then(|| vec![0.0; pixels]);
        for (block, block_direct, block_caustics) in raw_blocks {
            if let (Some(dst), Some(src)) = (&mut fixed_direct, block_direct) {
                for (a, b) in dst.iter_mut().zip(src) {
                    *a += b;
                }
            }
            if let (Some(dst), Some(src)) = (&mut fixed_caustics, block_caustics) {
                for (a, b) in dst.iter_mut().zip(src) {
                    *a += b;
                }
            }
            blocks.push(block);
        }
        Ok(Self {
            width: settings.width,
            height: settings.height,
            weight,
            nodes,
            normalization,
            spectral_extinction,
            spectral_caustics,
            uncached_target: if cache_spectral_segments || single_pass_caustics {
                None
            } else {
                target
            },
            ground,
            max_volume_scatters: settings.photon_max_volume_scatters,
            base_events,
            blocks,
            fixed_direct,
            fixed_caustics,
        })
    }
}

fn fixed_transmittance(
    job: &RenderJob,
    segment: PhotonSegment,
) -> Result<Option<f64>, &'static str> {
    match segment.medium(&job.scene)? {
        None => Ok(Some(1.0)),
        Some(medium) if medium.has_wavelength_dependent_coefficients() => Ok(None),
        Some(medium) => segment
            .transmittance_in(medium, ACHROMATIC_QUERY_NM)
            .map(Some),
    }
}

pub(super) fn event_rng_from_base(base: &ChaCha12Rng, index: usize) -> ChaCha12Rng {
    let mut rng = base.clone();
    rng.set_stream((index as u64) ^ 0x7068_6f74_6f6e_0001);
    rng
}

pub(super) fn deposit(
    values: &mut [f64],
    position: Vector3<f64>,
    irradiance: f64,
    [width, height]: [usize; 2],
    receiver: &GroundReceiver,
) -> Result<(), &'static str> {
    if !irradiance.is_finite() {
        return Err("irradiance accumulation overflow");
    }
    let Some(weights) = ground_splat_weights(position, width, height, receiver)? else {
        return Ok(());
    };
    for (x, y, w) in weights {
        if w == 0.0 {
            continue;
        }
        let value = &mut values[y * width + x];
        *value += irradiance * w;
        if !value.is_finite() {
            return Err("irradiance accumulation overflow");
        }
    }
    Ok(())
}

pub(super) fn spectral_sample_blocks(
    samples_x: usize,
    samples_y: usize,
    seed: u64,
    max_blocks: usize,
    random_dimensions: usize,
) -> Vec<(Range<usize>, ChaCha12Rng)> {
    let rows_per_block = samples_y.div_ceil(max_blocks);
    let mut blocks = Vec::new();
    for start in (0..samples_y).step_by(rows_per_block) {
        let end = start.saturating_add(rows_per_block).min(samples_y);
        let mut rng = ChaCha12Rng::seed_from_u64(seed);
        // 一个 f64 消耗两个 u32 word；平行光两维、圆盘位置/方向共四维。
        // u128 避免大采样数在 usize 中溢出。
        let first_sample = start as u128 * samples_x as u128;
        rng.set_word_pos(first_sample * random_dimensions as u128 * 2);
        blocks.push((start..end, rng));
    }
    blocks
}

fn for_each_emitted_hit(
    scene: &RenderJob,
    samples_x: usize,
    samples_y: usize,
    rows: Range<usize>,
    rng: &mut ChaCha12Rng,
    mut visit: impl FnMut(usize, Ray, Hit, MediumRegion) -> Result<(), PhotonError>,
) -> Result<(), PhotonError> {
    let has_medium_interface = scene.scene.has_medium_interface();
    match &scene.scene.light {
        crate::light::Light::Parallel(light) => {
            let span = light.aperture_max_m - light.aperture_min_m;
            let direction =
                crate::material::unit_vector(light.direction).ok_or("光源发射采样失败")?;
            let min = light.aperture_min_m;
            let z = light.height_m;
            for sample_y in rows {
                for sample_x in 0..samples_x {
                    let ux = (sample_x as f64 + rng.random::<f64>()) / samples_x as f64;
                    let uy = (sample_y as f64 + rng.random::<f64>()) / samples_y as f64;
                    let origin = Vector3::new(min.x + ux * span.x, min.y + uy * span.y, z);
                    if !origin.iter().all(|v| v.is_finite()) {
                        return Err("光源发射采样失败".into());
                    }
                    let ray = Ray { origin, direction };
                    let region = if has_medium_interface {
                        scene.scene.initial_medium_region(ray.origin, None)?
                    } else {
                        MediumRegion::Exterior
                    };
                    if let Some(hit) = scene.scene.intersect(&ray) {
                        visit(sample_y * samples_x + sample_x, ray, hit, region)?;
                    }
                }
            }
        }
        crate::light::Light::DiskArea(disk) => {
            for sample_y in rows {
                for sample_x in 0..samples_x {
                    let u = [
                        (sample_x as f64 + rng.random::<f64>()) / samples_x as f64,
                        (sample_y as f64 + rng.random::<f64>()) / samples_y as f64,
                    ];
                    let v = [rng.random::<f64>(), rng.random::<f64>()];
                    let ray = disk.sample_emission(u, v).ok_or("光源发射采样失败")?.ray;
                    let region = if has_medium_interface {
                        scene.scene.initial_medium_region(ray.origin, None)?
                    } else {
                        MediumRegion::Exterior
                    };
                    if let Some(hit) = scene.scene.intersect(&ray) {
                        visit(sample_y * samples_x + sample_x, ray, hit, region)?;
                    }
                }
            }
        }
    }
    Ok(())
}

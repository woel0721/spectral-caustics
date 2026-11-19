use crate::caustics::map::ground_splat_weights;
use crate::geometry::hit::Hit;
// Shared forward emission and scalar irradiance kernel for XYZ and wavelength atlases.
use super::{
    PhotonTracer,
    path::{PhotonPathState, segment_transmittance},
};
use crate::{
    caustics::{SpectralCausticsMap, SpectralLighting},
    config::{RenderJob, render::RenderSetting},
    film::Framebuffer,
    geometry::ray::Ray,
    optics::dispersion::IorMode,
    spectrum::{cie_xyz, d65_spd},
};
use nalgebra::Vector3;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;
use rayon::prelude::*;
use std::ops::Range;
type PhotonError = Box<dyn std::error::Error + Send + Sync>;
struct EmissionBlock {
    // Sample index, emission ray, first hit, and its wavelength-independent Tr.
    hits: Vec<(usize, Ray, Hit, f64)>,
    direct: Option<Vec<f64>>,
}
struct PhotonPlan {
    blocks: Vec<EmissionBlock>,
    width: usize,
    height: usize,
    weight: f64,
    nodes: Vec<(f64, f64)>,
    normalization: f64,
}
impl PhotonPlan {
    fn new(
        job: &RenderJob,
        settings: &RenderSetting,
        target: Option<usize>,
        ground: usize,
        direct: bool,
    ) -> Result<Self, PhotonError> {
        settings.validate()?;
        if ground >= job.scene.objects.len()
            || target.is_some_and(|id| id == ground || id >= job.scene.objects.len())
        {
            return Err("invalid photon target or receiver".into());
        }
        let target = target.filter(|&id| job.supports_ideal_caustics(id));
        job.scene.light.validate_emission()?;
        let area = job.photon.receiver.area();
        let span = job.photon.receiver.max_m - job.photon.receiver.min_m;
        if !area.is_finite()
            || area <= 0.0
            || span.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || job
                .photon
                .receiver
                .min_m
                .iter()
                .chain(job.photon.receiver.max_m.iter())
                .any(|v| !v.is_finite())
        {
            return Err("invalid photon receiver bounds".into());
        }
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
        let medium = job.scene.medium.as_deref();
        let blocks: Result<Vec<_>, &'static str> = spectral_sample_blocks(
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
                direct: direct.then(|| vec![0.0; pixels]),
            };
            for_each_emitted_hit(job, nx, ny, rows, &mut rng, |index, ray, hit| {
                if hit.object_id == ground {
                    if let Some(values) = &mut block.direct {
                        let irradiance =
                            weight * segment_transmittance(medium, &ray, hit.distance)?;
                        for (x, y, w) in ground_splat_weights(
                            hit.coords,
                            settings.width,
                            settings.height,
                            &job.photon.receiver,
                        )? {
                            values[y * settings.width + x] += irradiance * w;
                        }
                    }
                } else if Some(hit.object_id) == target {
                    let tr = segment_transmittance(medium, &ray, hit.distance)?;
                    block.hits.push((index, ray, hit, tr));
                }
                Ok(())
            })?;
            Ok(block)
        })
        .collect();
        Ok(Self {
            width: settings.width,
            height: settings.height,
            weight,
            nodes,
            normalization,
            blocks: blocks?,
        })
    }
}
struct GeometryLayer {
    direct: Option<Vec<f64>>,
    caustics: Vec<f64>,
}
#[allow(clippy::too_many_arguments)]
fn geometry_layer(
    job: &RenderJob,
    settings: &RenderSetting,
    plan: &PhotonPlan,
    target: Option<usize>,
    ground: usize,
    nm: f64,
    direct: bool,
    parallel: bool,
) -> Result<GeometryLayer, PhotonError> {
    let tracer = target
        .map(|id| {
            let ior = job
                .photon_ior(id, Some(nm), settings.mode)
                .ok_or("invalid dielectric IOR")?;
            PhotonTracer::new(job, id, ground, ior, settings.photon_max_events)
        })
        .transpose()?;
    let block = |source: &EmissionBlock| -> Result<GeometryLayer, &'static str> {
        let count = plan.width * plan.height;
        let mut layer = GeometryLayer {
            direct: if direct { source.direct.clone() } else { None },
            caustics: vec![0.0; count],
        };
        if let Some(tracer) = &tracer {
            for &(index, ray, hit, tr) in &source.hits {
                let mut events = ChaCha12Rng::seed_from_u64(settings.seed);
                events.set_stream((index as u64) ^ 0x7068_6f74_6f6e_0001);
                let mut state = PhotonPathState::new(ray);
                state.flux_weight = tr;
                let Some(landing) = tracer
                    .trace_state(job, state, Some(hit), &mut events)?
                    .landing()
                else {
                    continue;
                };
                for (x, y, w) in ground_splat_weights(
                    landing.position,
                    plan.width,
                    plan.height,
                    &job.photon.receiver,
                )? {
                    let p = &mut layer.caustics[y * plan.width + x];
                    *p += plan.weight * landing.weight * w;
                    if !p.is_finite() {
                        return Err("irradiance accumulation overflow");
                    }
                }
            }
        }
        Ok(layer)
    };
    // Indexed collection fixes the reduction order independently of worker count.
    let mut result = GeometryLayer {
        direct: direct.then(|| vec![0.0; plan.width * plan.height]),
        caustics: vec![0.0; plan.width * plan.height],
    };
    let mut merge = |part: Result<GeometryLayer, &'static str>| -> Result<(), PhotonError> {
        let part = part?;
        for (a, b) in result.caustics.iter_mut().zip(part.caustics) {
            *a += b;
        }
        if let (Some(a), Some(b)) = (&mut result.direct, part.direct) {
            for (a, b) in a.iter_mut().zip(b) {
                *a += b;
            }
        }
        Ok(())
    };
    if parallel {
        for part in plan.blocks.par_iter().map(block).collect::<Vec<_>>() {
            merge(part)?;
        }
    } else {
        for item in &plan.blocks {
            merge(block(item))?;
        }
    }
    Ok(result)
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
    let target = target.filter(|&id| job.supports_ideal_caustics(id));
    let mut caustics = Framebuffer::new(plan.width, plan.height)?;
    let base = geometry_layer(
        job,
        settings,
        &plan,
        target,
        ground,
        plan.nodes[0].0,
        direct,
        true,
    )?;
    let white = job
        .scene
        .light
        .white_xyz(&plan.nodes)
        .ok_or("source XYZ overflow")?;
    let direct = if let Some(values) = base.direct {
        Some({
            let mut frame = Framebuffer::new(plan.width, plan.height)?;
            for (p, v) in frame.pixels.iter_mut().zip(values) {
                *p = white * v;
            }
            frame
        })
    } else {
        None
    };
    if settings.mode == IorMode::Constant || target.is_none() {
        for (p, v) in caustics.pixels.iter_mut().zip(base.caustics) {
            *p = white * v;
        }
    } else {
        for (index, &(nm, width)) in plan.nodes.iter().enumerate() {
            let layer = if index == 0 {
                None
            } else {
                Some(geometry_layer(
                    job, settings, &plan, target, ground, nm, false, true,
                )?)
            };
            let values = layer.as_ref().map_or(&base.caustics, |l| &l.caustics);
            let xyz = cie_xyz(nm).expect("validated wavelength")
                * (plan.normalization * d65_spd(nm).expect("embedded spectrum") * width);
            for (p, v) in caustics.pixels.iter_mut().zip(values) {
                *p += xyz * *v;
            }
        }
    }
    let (light_wi, strength) = job
        .scene
        .light
        .analytic_sun()?
        .unwrap_or((Vector3::z(), 0.0));
    Ok(SpectralLighting {
        direct,
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
    let plan = PhotonPlan::new(job, settings, Some(target), ground, false)?;
    if !job.supports_ideal_caustics(target) {
        return Ok(None);
    }
    let material = job.scene.material(job.scene.objects[ground].material_id());
    if material.irradiance_reflectance().is_none() {
        return Err("caustics require a Lambertian receiver".into());
    }
    plan.width
        .checked_mul(plan.height)
        .and_then(|n| n.checked_mul(plan.nodes.len()))
        .and_then(|n| n.checked_mul(size_of::<f64>()))
        .filter(|&n| n <= isize::MAX as usize)
        .ok_or("atlas size overflow")?;
    let constant = if settings.mode == IorMode::Constant {
        Some(
            geometry_layer(
                job,
                settings,
                &plan,
                Some(target),
                ground,
                plan.nodes[0].0,
                false,
                true,
            )?
            .caustics,
        )
    } else {
        None
    };
    let layers: Result<Vec<_>, String> = plan
        .nodes
        .par_iter()
        .map(|&(nm, _)| {
            let mut layer = if let Some(g) = &constant {
                g.clone()
            } else {
                geometry_layer(job, settings, &plan, Some(target), ground, nm, false, false)
                    .map_err(|e| e.to_string())?
                    .caustics
            };
            let density = plan.normalization * d65_spd(nm).expect("embedded spectrum");
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

fn spectral_sample_blocks(
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
    mut visit: impl FnMut(usize, Ray, crate::geometry::hit::Hit) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let area = scene.scene.light.as_disk().is_some();
    for sample_y in rows {
        for sample_x in 0..samples_x {
            let u = [
                (sample_x as f64 + rng.random::<f64>()) / samples_x as f64,
                (sample_y as f64 + rng.random::<f64>()) / samples_y as f64,
            ];
            let v = if area {
                [rng.random::<f64>(), rng.random::<f64>()]
            } else {
                [0.0; 2]
            };
            let ray = scene
                .scene
                .light
                .emission_ray(u, v)
                .ok_or("光源发射采样失败")?;
            if let Some(hit) = scene.scene.intersect(&ray) {
                visit(sample_y * samples_x + sample_x, ray, hit)?;
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use crate::caustics::map::ground_to_pixel;
    use std::error::Error;

    const SAMPLING_SEED: u64 = 42;
    const MAX_SPECTRAL_BLOCKS: usize = 16;

    // 这些能量/落点参考值使用垂直光源，独立于展示场景的默认斜射光。
    fn vertical_light_scene() -> RenderJob {
        let mut scene = RenderJob::new();
        scene.scene.light.as_parallel_mut().unwrap().direction = -Vector3::z();
        scene
    }

    fn settings_for_size(width: usize, height: usize) -> RenderSetting {
        RenderSetting {
            width,
            height,
            ..RenderSetting::default()
        }
    }

    fn sample_settings(base: &RenderSetting, samples: usize, mode: IorMode) -> RenderSetting {
        RenderSetting {
            samples_per_pixel_axis: samples,
            mode,
            ..base.clone()
        }
    }

    use super::*;
    use crate::geometry::ground::Ground;
    use crate::geometry::prism::Prism;
    use crate::output::save_png_with_exposure;
    use rand::rngs::StdRng;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    #[test]
    fn emission_blocks_seek_both_parallel_and_disk_random_dimensions_exactly() {
        for dimensions in [2, 4] {
            for max_blocks in [1, 2, 7] {
                let mut reference = StdRng::seed_from_u64(SAMPLING_SEED);
                for (rows, mut rng) in
                    spectral_sample_blocks(3, 9, SAMPLING_SEED, max_blocks, dimensions)
                {
                    for _ in rows {
                        for _ in 0..3 {
                            for _ in 0..dimensions {
                                assert_eq!(reference.random::<f64>(), rng.random::<f64>());
                            }
                        }
                    }
                }
            }
        }
    }

    struct CountedGround {
        ground: Ground,
        calls: Arc<AtomicUsize>,
    }

    impl crate::geometry::object::Hittable for CountedGround {
        fn intersect(&self, ray: &Ray) -> Option<crate::geometry::hit::GeometryHit> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.ground.intersect(ray)
        }
    }

    #[test]
    fn oblique_sun_and_photon_flux_share_the_horizontal_window_scale() {
        let mut scene = RenderJob::new();
        scene.scene.light.as_parallel_mut().unwrap().direction = Vector3::new(-3.0, 0.0, -4.0);
        // 整个发射窗口的落点平移 -3*3/4=-2.25 米；接收窗口一起平移。
        scene.photon.receiver.min_m.x -= 2.25;
        scene.photon.receiver.max_m.x -= 2.25;
        let ground_id = scene.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        let settings = settings_for_size(12, 8);
        let lighting = integrate_xyz(&scene, None, ground_id, &settings).unwrap();
        let photon_flux: f64 = lighting
            .direct
            .as_ref()
            .unwrap()
            .pixels
            .iter()
            .map(|p| p.y)
            .sum::<f64>()
            * scene.photon.receiver.area()
            / (settings.width * settings.height) as f64;
        let analytic_e = lighting.sun_irradiance_xyz.y * lighting.light_wi.z;
        assert!((photon_flux - scene.scene.light.as_parallel().unwrap().total_flux).abs() < 1e-12);
        assert!(
            (analytic_e
                - scene.scene.light.as_parallel().unwrap().total_flux
                    / scene.photon.receiver.area())
            .abs()
                < 1e-12
        );
        assert!((lighting.sun_irradiance_xyz.y - 0.625).abs() < 1e-12);
    }

    #[test]
    fn split_lighting_rejects_invalid_sun_receiver_and_merge_data() {
        let mut scene = RenderJob::new();
        let ground = scene.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        let settings = settings_for_size(2, 2);
        for direction in [
            Vector3::x(),
            Vector3::z(),
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
        ] {
            scene.scene.light.as_parallel_mut().unwrap().direction = direction;
            assert!(integrate_xyz(&scene, None, ground, &settings).is_err());
        }
        scene.scene.light.as_parallel_mut().unwrap().direction = -Vector3::z();
        let mut lighting = integrate_xyz(&scene, None, ground, &settings).unwrap();
        lighting.caustics.pixels[0].x = f64::NAN;
        assert!(lighting.combined().is_err());
        lighting.caustics = Framebuffer::new(1, 1).unwrap();
        assert!(lighting.combined().is_err());
        scene.photon.receiver.max_m = scene.photon.receiver.min_m;
        assert!(integrate_xyz(&scene, None, ground, &settings).is_err());
    }

    #[test]
    fn spectral_blocks_cover_every_row_and_replay_the_original_random_sequence() {
        for (samples_x, samples_y) in [(1, 1), (7, 5), (3, 17), (5, 33)] {
            let blocks =
                spectral_sample_blocks(samples_x, samples_y, SAMPLING_SEED, MAX_SPECTRAL_BLOCKS, 2);
            assert!(blocks.len() <= MAX_SPECTRAL_BLOCKS);
            let mut serial_rng = StdRng::seed_from_u64(SAMPLING_SEED);
            let mut next_row = 0;
            for (rows, mut rng) in blocks {
                assert_eq!(rows.start, next_row);
                assert!(rows.end > rows.start);
                next_row = rows.end;
                for _ in rows {
                    for _ in 0..samples_x {
                        assert_eq!(rng.random::<f64>(), serial_rng.random::<f64>());
                        assert_eq!(rng.random::<f64>(), serial_rng.random::<f64>());
                    }
                }
            }
            assert_eq!(next_row, samples_y);
        }
    }

    #[test]
    fn spectral_xyz_is_identical_across_rayon_thread_counts_and_repeated_runs() {
        let settings = settings_for_size(19, 17);
        let mut scene = RenderJob::new();
        let caustics_object_id = scene.scene.add(
            Box::new(Prism::new().unwrap()),
            Box::new(
                crate::material::dielectric::Dielectric::with_dispersion(
                    1.52,
                    Some(crate::optics::dispersion::Cauchy::default()),
                )
                .unwrap(),
            ),
        );
        let ground_id = scene.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        let serial_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            let reference = serial_pool.install(|| {
                crate::integrator::photon::integrate_xyz(
                    &scene,
                    Some(caustics_object_id),
                    ground_id,
                    &sample_settings(&settings, 2, mode),
                )
                .and_then(|images| images.combined())
                .unwrap()
            });
            for threads in [2, 4, 2] {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap();
                let image = pool.install(|| {
                    crate::integrator::photon::integrate_xyz(
                        &scene,
                        Some(caustics_object_id),
                        ground_id,
                        &sample_settings(&settings, 2, mode),
                    )
                    .and_then(|images| images.combined())
                    .unwrap()
                });
                assert_eq!(image.pixels, reference.pixels);
            }
        }
    }

    #[test]
    fn direct_blocked_and_missed_rays_query_the_scene_once_per_spatial_sample() {
        // 三种场景分别覆盖直接落地、其他物体遮挡、所有物体均未命中。
        for case in 0..3 {
            let settings = settings_for_size(6, 4);
            let mut scene = RenderJob::new();
            let caustics_object_id = if case == 2 {
                scene.scene.add(
                    Box::new(Ground::new(4.0).unwrap()),
                    Box::new(
                        crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(
                            0.8,
                        ))
                        .unwrap(),
                    ),
                )
            } else {
                scene.scene.add(
                    Box::new(Prism::new().unwrap()),
                    Box::new(
                        crate::material::dielectric::Dielectric::with_dispersion(
                            1.52,
                            Some(crate::optics::dispersion::Cauchy::default()),
                        )
                        .unwrap(),
                    ),
                )
            };
            let calls = Arc::new(AtomicUsize::new(0));
            let ground_id = scene.scene.add(
                Box::new(CountedGround {
                    ground: Ground::new(if case == 2 { 5.0 } else { 2.0 }).unwrap(),
                    calls: calls.clone(),
                }),
                Box::new(
                    crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                        .unwrap(),
                ),
            );
            if case == 1 {
                scene.scene.add(
                    Box::new(Ground::new(2.5).unwrap()),
                    Box::new(
                        crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(
                            0.8,
                        ))
                        .unwrap(),
                    ),
                );
            }
            let samples_per_pixel_axis = 2;
            let spatial_ray_count =
                settings.width * settings.height * samples_per_pixel_axis * samples_per_pixel_axis;
            for mode in [IorMode::Constant, IorMode::Dispersion] {
                calls.store(0, Ordering::Relaxed);
                let image = crate::integrator::photon::integrate_xyz(
                    &scene,
                    Some(caustics_object_id),
                    ground_id,
                    &sample_settings(&settings, samples_per_pixel_axis, mode),
                )
                .and_then(|images| images.combined())
                .unwrap();
                assert_eq!(calls.load(Ordering::Relaxed), spatial_ray_count);
                assert_eq!(image.pixels.iter().any(|xyz| xyz.y > 0.0), case == 0);
            }
        }
    }

    #[test]
    fn spectral_unblocked_white_preserves_xyz_flux_across_spatial_sample_counts() {
        let white_xyz = Vector3::new(0.9501416822353593, 1.0, 1.087976622255359);
        for (width, height) in [(1, 1), (1, 4), (4, 1), (12, 8)] {
            let mut scene = vertical_light_scene();
            let ground = scene.scene.add(
                Box::new(Ground::new(0.0).unwrap()),
                Box::new(
                    crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                        .unwrap(),
                ),
            );
            for samples in [1, 3] {
                for mode in [IorMode::Constant, IorMode::Dispersion] {
                    let settings =
                        sample_settings(&settings_for_size(width, height), samples, mode);
                    let image =
                        crate::integrator::photon::integrate_xyz(&scene, None, ground, &settings)
                            .and_then(|images| images.combined())
                            .unwrap();
                    let flux: Vector3<f64> = image.pixels.iter().copied().sum::<Vector3<f64>>()
                        * scene.photon.receiver.area()
                        / (width * height) as f64;
                    assert!(
                        (flux - white_xyz * scene.scene.light.as_parallel().unwrap().total_flux)
                            .norm()
                            < 2e-12
                    );
                }
            }
        }
    }

    #[test]
    fn spectral_constant_stays_white_and_dispersion_separates_colors_reproducibly() {
        let settings = settings_for_size(128, 16);
        let mut scene = RenderJob::new();
        let caustics_object_id = scene.scene.add(
            Box::new(Prism::new().unwrap()),
            Box::new(
                crate::material::dielectric::Dielectric::with_dispersion(
                    1.52,
                    Some(crate::optics::dispersion::Cauchy::default()),
                )
                .unwrap(),
            ),
        );
        let ground_id = scene.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        let constant = crate::integrator::photon::integrate_xyz(
            &scene,
            Some(caustics_object_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
        )
        .and_then(|images| images.combined())
        .unwrap();
        let dispersion = crate::integrator::photon::integrate_xyz(
            &scene,
            Some(caustics_object_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Dispersion),
        )
        .and_then(|images| images.combined())
        .unwrap();
        let repeated = crate::integrator::photon::integrate_xyz(
            &scene,
            Some(caustics_object_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Dispersion),
        )
        .and_then(|images| images.combined())
        .unwrap();
        assert_eq!(dispersion.pixels, repeated.pixels);
        assert_ne!(constant.pixels, dispersion.pixels);
        assert_eq!(constant.get(0, 0), dispersion.get(0, 0));
        let white_xyz = Vector3::new(0.9501416822353593, 1.0, 1.087976622255359);
        assert!(
            constant
                .pixels
                .iter()
                .all(|pixel| { (pixel - white_xyz * pixel.y).norm() < 1e-11 })
        );
        assert!(
            dispersion
                .pixels
                .iter()
                .any(|pixel| { (pixel - white_xyz * pixel.y).norm() > 1e-3 })
        );
        let emitted_y = 0.5 * (settings.width * settings.height) as f64;
        for image in [&constant, &dispersion] {
            assert!(
                image
                    .pixels
                    .iter()
                    .all(|pixel| { pixel.iter().all(|value| value.is_finite() && *value >= 0.0) })
            );
            let total_y: f64 = image.pixels.iter().map(|pixel| pixel.y).sum();
            assert!(total_y > 0.0 && total_y <= emitted_y + 1e-9);
        }
    }

    #[test]
    fn spectral_png_roundtrip_has_neutral_constant_mode_and_colored_dispersion()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let settings = settings_for_size(128, 16);
        let mut scene = RenderJob::new();
        let caustics_object_id = scene.scene.add(
            Box::new(Prism::new().unwrap()),
            Box::new(
                crate::material::dielectric::Dielectric::with_dispersion(
                    1.52,
                    Some(crate::optics::dispersion::Cauchy::default()),
                )
                .unwrap(),
            ),
        );
        let ground_id = scene.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("spectral-png-{}-{timestamp}", std::process::id()));
        for (mode, filename) in [
            (IorMode::Constant, "constant.png"),
            (IorMode::Dispersion, "dispersion.png"),
        ] {
            let image = crate::integrator::photon::integrate_xyz(
                &scene,
                Some(caustics_object_id),
                ground_id,
                &sample_settings(&settings, 2, mode),
            )
            .and_then(|images| images.combined())?;
            let path = directory.join(filename);
            save_png_with_exposure(&image, &path, 1.0)?;
            let decoded = image::open(&path)?;
            assert_eq!(decoded.color(), image::ColorType::Rgb8);
            let rgb = decoded.into_rgb8();
            assert_eq!(rgb.dimensions(), (128, 16));
            let channel_spread = |pixel: &image::Rgb<u8>| {
                pixel.0.iter().max().unwrap() - pixel.0.iter().min().unwrap()
            };
            match mode {
                // 有限波长范围和 10 nm 积分使 D65 白点存在微小误差，允许 1 字节差异。
                IorMode::Constant => assert!(rgb.pixels().all(|pixel| channel_spread(pixel) <= 1)),
                IorMode::Dispersion => assert!(rgb.pixels().any(|pixel| channel_spread(pixel) > 2)),
            }
        }
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn spectral_render_rejects_invalid_ids_dimensions_and_sampling_overflow() {
        let mut settings = settings_for_size(8, 4);
        let mut scene = RenderJob::new();
        let caustics_object_id = scene.scene.add(
            Box::new(Prism::new().unwrap()),
            Box::new(
                crate::material::dielectric::Dielectric::with_dispersion(
                    1.52,
                    Some(crate::optics::dispersion::Cauchy::default()),
                )
                .unwrap(),
            ),
        );
        let ground_id = scene.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            for (caustics_object_id, ground_id, samples) in [
                (usize::MAX, ground_id, 1),
                (ground_id, ground_id, 1),
                (caustics_object_id, usize::MAX, 1),
                (caustics_object_id, ground_id, 0),
                (caustics_object_id, ground_id, usize::MAX),
            ] {
                assert!(
                    crate::integrator::photon::integrate_xyz(
                        &scene,
                        Some(caustics_object_id),
                        ground_id,
                        &sample_settings(&settings, samples, mode)
                    )
                    .and_then(|images| images.combined())
                    .is_err()
                );
            }
        }
        for (width, height) in [
            (0, 4),
            (4, 0),
            (usize::MAX, 2),
            (usize::MAX, 1),
            (1, usize::MAX),
        ] {
            settings.width = width;
            settings.height = height;
            assert!(
                crate::integrator::photon::integrate_xyz(
                    &scene,
                    Some(caustics_object_id),
                    ground_id,
                    &sample_settings(&settings, 2, IorMode::Dispersion)
                )
                .and_then(|images| images.combined())
                .is_err()
            );
        }
    }

    #[test]
    fn world_coordinates_map_to_non_square_image_with_y_flipped() {
        for (position, expected) in [
            (Vector3::new(-2.0, -2.0, 0.0), (-0.5, 1.5)),
            (Vector3::new(-1.5, 1.5, 0.0), (-0.125, -0.25)),
            (Vector3::new(1.5, -1.5, 0.0), (2.125, 1.25)),
            (Vector3::new(1.99, 1.99, 0.0), (2.4925, -0.495)),
        ] {
            let actual =
                ground_to_pixel(position, 3, 2, &RenderJob::new().photon.receiver).unwrap();
            assert!((actual.0 - expected.0).abs() < 1e-12);
            assert!((actual.1 - expected.1).abs() < 1e-12);
        }
        assert_eq!(
            ground_to_pixel(
                Vector3::new(-1.5, 1.0, 0.0),
                4,
                2,
                &RenderJob::new().photon.receiver
            ),
            Some((0.0, 0.0))
        );
        assert_eq!(
            ground_to_pixel(
                Vector3::new(1.5, -1.0, 0.0),
                4,
                2,
                &RenderJob::new().photon.receiver
            ),
            Some((3.0, 1.0))
        );
    }

    #[test]
    fn mapping_rejects_outside_non_finite_and_empty_inputs() {
        for value in [-2.01, 2.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                ground_to_pixel(
                    Vector3::new(value, 0.0, 0.0),
                    3,
                    2,
                    &RenderJob::new().photon.receiver
                ),
                None
            );
            assert_eq!(
                ground_to_pixel(
                    Vector3::new(0.0, value, 0.0),
                    3,
                    2,
                    &RenderJob::new().photon.receiver
                ),
                None
            );
        }
        assert_eq!(
            ground_to_pixel(
                Vector3::new(0.0, 0.0, f64::NAN),
                3,
                2,
                &RenderJob::new().photon.receiver
            ),
            None
        );
        assert_eq!(
            ground_to_pixel(Vector3::zeros(), 0, 2, &RenderJob::new().photon.receiver),
            None
        );
        assert_eq!(
            ground_to_pixel(Vector3::zeros(), 3, 0, &RenderJob::new().photon.receiver),
            None
        );
    }
    #[test]
    fn deposition_weights_conserve_flux_at_interior_edges_and_single_pixel_receivers() {
        let receiver = RenderJob::new().photon.receiver;
        for (width, height) in [(4, 2), (1, 1), (1, 4), (4, 1)] {
            for position in [
                Vector3::zeros(),
                Vector3::new(-2.0, -2.0, 0.0),
                Vector3::new(2.0 - 1e-10, 2.0 - 1e-10, 0.0),
            ] {
                let weights = ground_splat_weights(position, width, height, &receiver).unwrap();
                assert!((weights.iter().map(|&(_, _, w)| w).sum::<f64>() - 1.0).abs() < 1e-14);
                assert!(weights.iter().all(|&(x, y, _)| x < width && y < height));
            }
        }
        let outside = ground_splat_weights(Vector3::new(2.0, 0.0, 0.0), 2, 2, &receiver).unwrap();
        assert_eq!(outside.iter().map(|&(_, _, w)| w).sum::<f64>(), 0.0);
    }
}

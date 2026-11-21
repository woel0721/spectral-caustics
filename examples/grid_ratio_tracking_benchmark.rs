//! Fixed-ray convergence and single-thread Release timing for grid transmittance.
use nalgebra::Vector3;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::Serialize;
use serde_json::json;
use spectral_caustics::{
    geometry::ray::Ray,
    medium::{
        GridMedium, GridMediumConfig, Medium, MediumCoefficient, ProceduralDensityConfig,
        ProceduralMediumConfig,
    },
};
use std::{error::Error, hint::black_box, time::Instant};

const RESOLUTION: usize = 64;
const MAX_DISTANCE: f64 = 3.0;
const CHECKPOINTS: [usize; 4] = [1_000, 10_000, 100_000, 1_000_000];
const BATCHES: usize = 12;
const FIRST_SAMPLE_SEED: u64 = 10_000;
const RAY_COUNT: usize = 4096;
const RAY_SEED: u64 = 42;
const TIMING_SAMPLES: usize = 100_000;
const TIMING_REPEATS: usize = 7;
const TIMING_SEED: u64 = 17;
const TARGET_RMSE: f64 = 0.01;

#[derive(Serialize)]
struct Convergence {
    dda: f64,
    ratio_mean: f64,
    abs_error: f64,
    ratio_sample_variance: f64,
    standard_error: f64,
    samples: u64,
    checkpoints: [Checkpoint; CHECKPOINTS.len()],
}

#[derive(Serialize)]
struct Checkpoint {
    samples_per_batch: usize,
    rmse: f64,
}

fn convergence(grid: &GridMedium, ray: &Ray, nm: f64) -> Result<Convergence, String> {
    let reference = grid.transmittance_ray(ray, MAX_DISTANCE, nm);
    let mut squared_errors = [0.0; CHECKPOINTS.len()];
    let (mut count, mut mean, mut m2) = (0_u64, 0.0, 0.0);
    for _ in 0..BATCHES {
        let mut sum = 0.0;
        let mut previous = 0;
        for (&n, error) in CHECKPOINTS.iter().zip(&mut squared_errors) {
            for _ in previous..n {
                let mut rng = ChaCha8Rng::seed_from_u64(FIRST_SAMPLE_SEED + count);
                let weight = grid.estimate_transmittance_ratio(ray, MAX_DISTANCE, nm, &mut rng)?;
                sum += weight;
                // Online sample variance over all seeds, independent of the batch size.
                count += 1;
                let delta = weight - mean;
                mean += delta / count as f64;
                m2 += delta * (weight - mean);
            }
            *error += (sum / n as f64 - reference).powi(2);
            previous = n;
        }
    }
    let variance = m2 / (count - 1) as f64;
    Ok(Convergence {
        dda: reference,
        ratio_mean: mean,
        abs_error: (mean - reference).abs(),
        ratio_sample_variance: variance,
        standard_error: (variance / count as f64).sqrt(),
        samples: count,
        checkpoints: std::array::from_fn(|i| Checkpoint {
            samples_per_batch: CHECKPOINTS[i],
            rmse: (squared_errors[i] / BATCHES as f64).sqrt(),
        }),
    })
}

fn timing(grid: &GridMedium, rays: &[Ray], nm: f64) -> Result<(f64, f64), String> {
    let mut warmup = ChaCha8Rng::seed_from_u64(7);
    for ray in rays {
        black_box(grid.transmittance_ray(ray, MAX_DISTANCE, nm));
        black_box(grid.estimate_transmittance_ratio(ray, MAX_DISTANCE, nm, &mut warmup)?);
    }
    let mut dda = [0.0; TIMING_REPEATS];
    let mut ratio = [0.0; TIMING_REPEATS];
    for (repeat, (dda_ns, ratio_ns)) in dda.iter_mut().zip(&mut ratio).enumerate() {
        let mut rng = ChaCha8Rng::seed_from_u64(TIMING_SEED + repeat as u64);
        let start = Instant::now();
        for ray in rays.iter().cycle().take(TIMING_SAMPLES) {
            black_box(grid.transmittance_ray(black_box(ray), MAX_DISTANCE, nm));
        }
        *dda_ns = start.elapsed().as_secs_f64() * 1e9 / TIMING_SAMPLES as f64;

        let start = Instant::now();
        for ray in rays.iter().cycle().take(TIMING_SAMPLES) {
            black_box(grid.estimate_transmittance_ratio(
                black_box(ray),
                MAX_DISTANCE,
                nm,
                &mut rng,
            )?);
        }
        *ratio_ns = start.elapsed().as_secs_f64() * 1e9 / TIMING_SAMPLES as f64;
    }
    dda.sort_by(f64::total_cmp);
    ratio.sort_by(f64::total_cmp);
    Ok((dda[TIMING_REPEATS / 2], ratio[TIMING_REPEATS / 2]))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut ray_rng = ChaCha8Rng::seed_from_u64(RAY_SEED);
    let rays: Vec<_> = (0..RAY_COUNT)
        .map(|i| {
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            Ray::new(
                Vector3::new(
                    -1.5 * sign,
                    ray_rng.random_range(-0.75..0.75),
                    ray_rng.random_range(-0.75..0.75),
                ),
                Vector3::new(
                    sign,
                    ray_rng.random_range(-0.1..0.1),
                    ray_rng.random_range(-0.1..0.1),
                ),
            )
        })
        .collect::<Option<_>>()
        .ok_or("invalid generated benchmark ray")?;
    let probe =
        Ray::new(Vector3::new(-1.5, 0.05, -0.1), Vector3::x()).ok_or("invalid probe ray")?;
    let mut config = GridMediumConfig {
        min_m: [-1.0; 3],
        max_m: [1.0; 3],
        resolution: [RESOLUTION; 3],
        densities: vec![1.0; RESOLUTION.pow(3)],
        sigma_a_base: 0.15.into(),
        sigma_s_base: 0.35.into(),
        g: 0.0.into(),
    };
    let uniform = GridMedium::try_from(&config)?;
    for (i, density) in config.densities.iter_mut().enumerate() {
        let n = RESOLUTION;
        let position = [i % n, (i / n) % n, i / n.pow(2)]
            .map(|axis| 2.0 * (axis as f64 + 0.5) / n as f64 - 1.0);
        let radius_squared = position.iter().map(|v| v * v).sum::<f64>();
        *density = 5.0 * (1.0 - radius_squared / 0.25_f64.powi(2)).max(0.0);
    }
    let sparse = GridMedium::try_from(&config)?;
    let fog_seed = 31;
    let fog = GridMedium::try_from(&ProceduralMediumConfig {
        min_m: config.min_m,
        max_m: config.max_m,
        resolution: config.resolution,
        procedural: ProceduralDensityConfig { seed: fog_seed },
        sigma_a_base: MediumCoefficient::Curve {
            samples: vec![[380.0, 0.25], [550.0, 0.12], [780.0, 0.03]],
        },
        sigma_s_base: MediumCoefficient::Curve {
            samples: vec![[380.0, 1.7], [550.0, 1.1], [780.0, 0.48]],
        },
        g: 0.0.into(),
    })?;

    let mut cases = Vec::new();
    for (name, grid, wavelengths) in [
        ("uniform", &uniform, &[550.0][..]),
        ("sparse", &sparse, &[550.0][..]),
        ("procedural_fog", &fog, &[450.0, 550.0, 650.0][..]),
    ] {
        for &nm in wavelengths {
            let stats = convergence(grid, &probe, nm)?;
            let (dda_ns, ratio_ns) = timing(grid, &rays, nm)?;
            let (probe_dda_ns, probe_ratio_ns) = timing(grid, std::slice::from_ref(&probe), nm)?;
            let target_samples = (stats.ratio_sample_variance / TARGET_RMSE.powi(2))
                .ceil()
                .max(1.0);
            cases.push(json!({
                "case": name, "wavelength_nm": nm, "convergence": stats,
                "dda_ns_per_ray": dda_ns, "ratio_ns_per_ray": ratio_ns,
                "dda_ns_per_probe": probe_dda_ns, "ratio_ns_per_probe": probe_ratio_ns,
                "ratio_samples_for_target_rmse": target_samples as u64,
                "ratio_ns_for_target_rmse": target_samples * probe_ratio_ns,
            }));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "resolution": config.resolution, "max_distance_m": MAX_DISTANCE,
            "probe_origin_m": probe.origin.as_slice(), "probe_direction": probe.direction.as_slice(),
            "batches": BATCHES, "first_sample_seed": FIRST_SAMPLE_SEED, "rng": "ChaCha8Rng",
            "timing_samples_per_repeat": TIMING_SAMPLES, "timing_repeats": TIMING_REPEATS,
            "timing_seed": TIMING_SEED, "ray_count": RAY_COUNT, "ray_seed": RAY_SEED,
            "fog_seed": fog_seed, "target_absolute_rmse": TARGET_RMSE, "cases": cases,
        }))?,
    );
    Ok(())
}

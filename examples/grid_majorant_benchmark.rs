//! Fixed-seed public-API sampling and whole-render benchmarks. No timing assertions.
use nalgebra::Vector3;
use rand::{Rng, RngCore, SeedableRng, rngs::StdRng};
use serde_json::json;
use spectral_caustics::{
    config::Config,
    geometry::ray::Ray,
    medium::{GridMedium, GridMediumConfig, Medium, MediumConfig},
    output,
    renderer::Renderer,
};
use std::{error::Error, hint::black_box, path::PathBuf, time::Instant};

struct CountedRng {
    inner: StdRng,
    draws: u64,
}

impl RngCore for CountedRng {
    fn next_u32(&mut self) -> u32 {
        self.inner.next_u32()
    }
    fn next_u64(&mut self) -> u64 {
        self.draws += 1;
        self.inner.next_u64()
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.inner.fill_bytes(dest);
    }
}

fn field(n: usize, name: &str) -> GridMediumConfig {
    let mut densities = Vec::with_capacity(n * n * n);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let p = [x, y, z].map(|i| 2.0 * (i as f64 + 0.5) / n as f64 - 1.0);
                let density = match name {
                    "uniform" => 1.0,
                    "sparse_clumps" => {
                        let r2 = p.iter().map(|v| v * v).sum::<f64>();
                        if r2 < 0.09 {
                            30.0 * (1.0 - r2 / 0.09)
                        } else {
                            0.0
                        }
                    }
                    "spike" => {
                        if [x, y, z] == [n / 2; 3] {
                            128.0
                        } else {
                            0.05
                        }
                    }
                    _ => unreachable!(),
                };
                densities.push(density);
            }
        }
    }
    GridMediumConfig {
        min_m: [-1.0; 3],
        max_m: [1.0; 3],
        resolution: [n; 3],
        densities,
        sigma_a_base: 0.05.into(),
        sigma_s_base: 0.95.into(),
        g: 0.7.into(),
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() > 6 {
        return Err("usage: grid_majorant_benchmark [rays=100000] [resolution=64] [repeats=5] [output_dir] [render_spp=0] [render_width=96]".into());
    }
    let number = |index: usize, default: usize| -> Result<usize, Box<dyn Error + Send + Sync>> {
        Ok(args
            .get(index)
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or(default))
    };
    let (samples, n, repeats) = (number(0, 100_000)?, number(1, 64)?, number(2, 5)?);
    let directory = args
        .get(3)
        .map_or_else(|| PathBuf::from("output/grid_benchmark"), PathBuf::from);
    let (spp, width) = (number(4, 0)?, number(5, 96)?);
    if samples == 0 || repeats == 0 || !(2..=256).contains(&n) || width == 0 {
        return Err("rays/repeats/width must be positive and resolution must be 2..256".into());
    }
    std::fs::create_dir_all(&directory)?;
    let mut ray_rng = StdRng::seed_from_u64(42);
    let rays: Vec<_> = (0..4096)
        .map(|i| {
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            Ray::new(
                Vector3::new(
                    -1.5 * sign,
                    ray_rng.random_range(-0.9..0.9),
                    ray_rng.random_range(-0.9..0.9),
                ),
                Vector3::new(
                    sign,
                    ray_rng.random_range(-0.1..0.1),
                    ray_rng.random_range(-0.1..0.1),
                ),
            )
            .unwrap()
        })
        .collect();
    let mut cases = Vec::new();
    for name in ["uniform", "sparse_clumps", "spike"] {
        let grid_config = field(n, name);
        let start = Instant::now();
        let grid = GridMedium::try_from(&grid_config)?;
        let construction_seconds = start.elapsed().as_secs_f64();
        // Warm code/data without affecting measured stream states.
        let mut warmup = StdRng::seed_from_u64(7);
        for ray in &rays {
            black_box(grid.sample_segment(ray, 3.0, 550.0, true, &mut warmup)?);
        }
        let mut seconds = Vec::new();
        let (mut candidates, mut draws, mut events, mut weight) = (0_u64, 0_u64, 0_usize, 0.0);
        for repeat in 0..repeats {
            let mut rng = CountedRng {
                inner: StdRng::seed_from_u64(17 + repeat as u64),
                draws: 0,
            };
            let start = Instant::now();
            for i in 0..samples {
                let before = rng.draws;
                let sample =
                    grid.sample_segment(&rays[i % rays.len()], 3.0, 550.0, true, &mut rng)?;
                // Both samplers use one u64 exponential draw and one u64 accept
                // draw per candidate; a final unused exponential is the odd tail.
                candidates += (rng.draws - before) / 2;
                events += usize::from(sample.position.is_some());
                weight += sample.weight;
                black_box(sample);
            }
            seconds.push(start.elapsed().as_secs_f64());
            draws += rng.draws;
        }
        let total = samples as f64 * repeats as f64;
        let mut report = json!({
            "name": name, "construction_seconds": construction_seconds,
            "sampling_median_seconds": median(&mut seconds), "sampling_seconds": seconds,
            "candidates_per_ray": candidates as f64 / total,
            "rng_u64_per_ray": draws as f64 / total,
            "event_fraction": events as f64 / total, "mean_weight": weight / total,
        });
        if spp > 0 {
            let mut config = Config::load("configs/grid_fog.json")?;
            let Some(MediumConfig::Grid(original)) = config.scene.medium.take() else {
                unreachable!()
            };
            config.scene.medium = Some(MediumConfig::Grid(GridMediumConfig {
                min_m: original.min_m,
                max_m: original.max_m,
                sigma_a_base: original.sigma_a_base,
                sigma_s_base: original.sigma_s_base,
                ..grid_config
            }));
            config.render.width = width;
            config.render.height = (width * 2 / 3).max(1);
            config.render.camera_spp = spp;
            config.render.thread_count = Some(1);
            config.render.output_path = directory.join(name).join("camera_combined.png");
            let job = config.build()?;
            let renderer = Renderer::new(config.render.clone())?;
            let mut times = Vec::new();
            let mut mean_y = 0.0;
            for repeat in 0..repeats {
                let start = Instant::now();
                let images = renderer.render(&job)?;
                times.push(start.elapsed().as_secs_f64());
                mean_y = images.combined.pixels().iter().map(|v| v.y).sum::<f64>()
                    / (width * config.render.height) as f64;
                if repeat == 0 {
                    output::save_components(
                        &images,
                        &config.render.output_path,
                        config.render.exposure,
                    )?;
                    config.save(directory.join(name).join("config.json"))?;
                }
            }
            report["render_median_seconds"] = json!(median(&mut times));
            report["render_seconds"] = json!(times);
            report["render_mean_y"] = json!(mean_y);
        }
        println!("{}", serde_json::to_string(&report)?);
        cases.push(report);
    }
    let report = json!({"samples": samples, "resolution": n, "repeats": repeats,
        "sampling_seed": 17, "ray_seed": 42, "render_seed": 42, "render_threads": 1,
        "render_spp": spp, "render_width": width, "cases": cases});
    std::fs::write(
        directory.join("benchmark.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}

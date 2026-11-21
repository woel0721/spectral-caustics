use super::dda::MajorantDda as GridDda;
use super::*;
use crate::medium::{MediumCoefficient, bounds::box_interval};
use rand::{SeedableRng, rngs::StdRng};

fn config(resolution: [usize; 3], densities: Vec<f64>) -> GridMediumConfig {
    GridMediumConfig {
        min_m: [0.0; 3],
        max_m: resolution.map(|v| v as f64),
        resolution,
        densities,
        sigma_a_base: 0.25.into(),
        sigma_s_base: 0.75.into(),
        g: 0.0.into(),
    }
}

fn ray(origin: [f64; 3], direction: [f64; 3]) -> Ray {
    Ray::new(origin.into(), direction.into()).unwrap()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 2e-13, "{a} != {b}");
}

#[test]
fn local_bounds_cover_every_fine_voxel_including_partial_blocks() {
    let mut c = config([9, 7, 5], (0..315).map(|i| i as f64 / 7.0).collect());
    c.sigma_a_base = MediumCoefficient::Curve {
        samples: vec![[380.0, 0.18], [530.0, 0.02], [780.0, 0.3]],
    };
    c.sigma_s_base = MediumCoefficient::Curve {
        samples: vec![[380.0, 1.2], [480.0, 0.7], [780.0, 0.0]],
    };
    let grid = GridMedium::try_from(&c).unwrap();
    for stride in [2, 4, 8] {
        let bounds = MajorantGrid::new(c.resolution, &c.densities, stride).unwrap();
        assert_eq!(bounds.resolution, c.resolution.map(|n| n.div_ceil(stride)));
        for nm in [380.0, 450.0, 480.0, 550.0, 690.0, 780.0] {
            let base = grid.base.at(nm);
            let extinction = |density| base.scaled_by_density(density).sigma_t();
            for z in 0..5 {
                for y in 0..7 {
                    for x in 0..9 {
                        let p = Vector3::new(x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5);
                        let max_density =
                            bounds.max_density_at([x / stride, y / stride, z / stride]);
                        assert!(grid.coefficients_at(p, nm).sigma_t() <= extinction(max_density));
                    }
                }
            }
            let max_density = bounds.max_densities.iter().copied().fold(0.0, f64::max);
            assert_eq!(extinction(max_density), grid.sigma_t_majorant(nm));
        }
    }
    let c = config([9, 7, 5], vec![1.7; 315]);
    let uniform = GridMedium::try_from(&c).unwrap();
    assert_eq!(uniform.majorants.max_densities.len(), 1);
    assert_eq!(uniform.majorants.resolution, [1; 3]);
    assert_eq!(uniform.majorants.max_density_at([0; 3]), 1.7);
    assert_eq!(uniform.sigma_t_majorant(550.0), 1.7 * 0.25 + 1.7 * 0.75);
}

#[test]
fn coarse_dda_uses_fine_faces_for_partial_blocks_and_negative_face_starts() {
    let grid = GridMedium::try_from(&config([9, 7, 5], vec![1.0; 315])).unwrap();
    for (r, expected) in [
        (
            ray([-1.0, 3.0, 2.0], [1.0, 0.0, 0.0]),
            vec![(1.0, 5.0, 0), (5.0, 9.0, 1), (9.0, 10.0, 2)],
        ),
        (
            ray([10.0, 3.0, 2.0], [-1.0, 0.0, 0.0]),
            vec![(1.0, 2.0, 2), (2.0, 6.0, 1), (6.0, 10.0, 0)],
        ),
        (ray([4.0, 3.0, 2.0], [-1.0, 0.0, 0.0]), vec![(0.0, 4.0, 0)]),
    ] {
        let (enter, exit) = grid.interval(&r, 20.0).unwrap();
        let actual: Vec<_> = GridDda::new(&grid, &r, enter, exit, 4)
            .map(|s| (s.start, s.end, s.cell[0]))
            .collect();
        assert_eq!(actual, expected);
    }
    let grid = GridMedium::try_from(&config([9; 3], vec![1.0; 729])).unwrap();
    let r = ray([-1.0; 3], [1.0; 3]);
    let (enter, exit) = grid.interval(&r, 30.0).unwrap();
    let segments: Vec<_> = GridDda::new(&grid, &r, enter, exit, 4).collect();
    assert_eq!(segments.len(), 3);
    for (i, segment) in segments.iter().enumerate() {
        assert_eq!(segment.cell, [i; 3]);
        close(
            segment.end - segment.start,
            if i < 2 {
                4.0 * 3.0_f64.sqrt()
            } else {
                3.0_f64.sqrt()
            },
        );
    }
}

#[test]
fn fine_and_coarse_dda_match_independent_block_slab_intersections() {
    let mut c = config([9, 7, 5], vec![1.0; 315]);
    c.min_m = [-1.3, -0.7, 0.1];
    c.max_m = [2.2, 3.1, 4.8];
    let grid = GridMedium::try_from(&c).unwrap();
    let mut rng = StdRng::seed_from_u64(912);
    for _ in 0..300 {
        let r = Ray::new(
            Vector3::from_fn(|_, _| rng.random_range(-4.0..6.0)),
            Vector3::from_fn(|_, _| rng.random_range(-1.0..1.0)),
        )
        .unwrap();
        let limit = rng.random_range(0.0..15.0);
        let Some((enter, exit)) = grid.interval(&r, limit) else {
            continue;
        };
        for stride in [1, 2, 4, 8] {
            let shape = c.resolution.map(|n| n.div_ceil(stride));
            let mut expected = Vec::new();
            for z in 0..shape[2] {
                for y in 0..shape[1] {
                    for x in 0..shape[0] {
                        let cell = [x, y, z];
                        let bound = |offset: usize| {
                            Vector3::from(std::array::from_fn(|axis| {
                                grid.face(
                                    axis,
                                    ((cell[axis] + offset) * stride).min(c.resolution[axis]),
                                )
                            }))
                        };
                        if let Some((a, b)) = box_interval(bound(0), bound(1), &r, limit) {
                            expected.push((a, b, cell));
                        }
                    }
                }
            }
            expected.sort_by(|a, b| a.0.total_cmp(&b.0));
            let actual: Vec<_> = GridDda::new(&grid, &r, enter, exit, stride).collect();
            assert_eq!(actual.len(), expected.len());
            for (actual, (a, b, cell)) in actual.iter().zip(expected) {
                close(actual.start, a);
                close(actual.end, b);
                assert_eq!(actual.cell, cell);
            }
        }
    }
}

fn layered() -> GridMediumConfig {
    config([9, 1, 1], vec![0.1, 0.1, 0.1, 0.1, 0.0, 0.0, 0.0, 0.0, 3.0])
}

#[test]
fn spectral_optical_depth_crosses_nonzero_and_vacuum_blocks_without_redrawing() {
    let mut c = layered();
    c.sigma_a_base = MediumCoefficient::Curve {
        samples: vec![[380.0, 0.2], [580.0, 0.6], [780.0, 0.8]],
    };
    c.sigma_s_base = MediumCoefficient::Curve {
        samples: vec![[380.0, 1.4], [480.0, 0.9], [580.0, 0.4], [780.0, 0.0]],
    };
    let mut grid = GridMedium::try_from(&c).unwrap();
    // Each block has uniform density, so the sampled optical depth can be
    // inverted independently without any null collisions.
    grid.majorants = MajorantGrid::new(grid.resolution, &grid.densities, 4).unwrap();
    for (nm, sigma_t, albedo) in [(480.0, 1.3, 0.9 / 1.3), (580.0, 1.0, 0.4)] {
        let mut crossed = false;
        for reverse in [false, true] {
            let r = if reverse {
                ray([10.0, 0.5, 0.5], [-1.0, 0.0, 0.0])
            } else {
                ray([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0])
            };
            for seed in 0..256 {
                let mut rng = StdRng::seed_from_u64(seed);
                let mut reference = rng.clone();
                let u: f64 = reference.sample(Open01);
                let tau = -(-u).ln_1p() / sigma_t;
                let expected = if reverse {
                    if tau < 3.0 {
                        Some(9.0 - tau / 3.0)
                    } else if tau < 3.4 {
                        Some(4.0 - (tau - 3.0) / 0.1)
                    } else {
                        None
                    }
                } else if tau < 0.4 {
                    Some(tau / 0.1)
                } else if tau < 3.4 {
                    crossed = true;
                    Some(8.0 + (tau - 0.4) / 3.0)
                } else {
                    None
                };
                let sample = grid.sample_segment(&r, 20.0, nm, true, &mut rng).unwrap();
                if let Some(x) = expected {
                    close(sample.position.unwrap().x, x);
                    close(sample.weight, albedo);
                    let _: f64 = reference.random(); // One acceptance draw, no boundary draws.
                } else {
                    assert!(sample.position.is_none());
                    assert_eq!(sample.weight, 1.0);
                }
                assert_eq!(rng.next_u64(), reference.next_u64());
            }
        }
        assert!(crossed);
    }
    // The red end of the same spectrum is pure absorption.
    let mut rng = StdRng::seed_from_u64(37);
    let mut untouched = rng.clone();
    let r = ray([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0]);
    let sample = grid
        .sample_segment(&r, 20.0, 780.0, true, &mut rng)
        .unwrap();
    assert!(sample.position.is_none());
    close(sample.weight, (-0.8_f64 * 3.4).exp());
    assert_eq!(rng.next_u64(), untouched.next_u64());
}

#[test]
fn local_collision_cdf_matches_analytic_layers_across_multiple_blocks_and_clips() {
    let base = GridMedium::try_from(&layered()).unwrap();
    for stride in [2, 4, 8] {
        let mut grid = base.clone();
        grid.majorants = MajorantGrid::new(grid.resolution, &grid.densities, stride).unwrap();
        for (r, limit, references) in [
            (
                ray([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0]),
                20.0,
                vec![
                    (0.5, 0.05_f64),
                    (4.0, 0.4),
                    (8.0, 0.4),
                    (8.5, 1.9),
                    (9.0, 3.4),
                ],
            ),
            (
                ray([10.0, 0.5, 0.5], [-1.0, 0.0, 0.0]),
                20.0,
                vec![(0.5, 1.5), (1.0, 3.0), (5.0, 3.0), (7.0, 3.2), (9.0, 3.4)],
            ),
            (
                ray([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0]),
                9.5,
                vec![(4.0, 0.4), (8.0, 0.4), (8.5, 1.9)],
            ),
        ] {
            let mut rng = StdRng::seed_from_u64(37);
            let mut counts = vec![0; references.len()];
            let n = 40_000;
            let (enter, exit) = grid.interval(&r, limit).unwrap();
            for _ in 0..n {
                let sample = grid
                    .sample_segment(&r, limit, 550.0, true, &mut rng)
                    .unwrap();
                if let Some(p) = sample.position {
                    let t = (p - r.origin).dot(&r.direction);
                    assert!(t >= enter && t < exit);
                    close(sample.weight, 0.75);
                    for (count, &(distance, _)) in counts.iter_mut().zip(&references) {
                        *count += usize::from(t - enter < distance);
                    }
                } else {
                    assert_eq!(sample.weight, 1.0);
                }
            }
            for (count, &(_, tau)) in counts.iter().zip(&references) {
                let expected = 1.0 - (-tau).exp();
                let observed = *count as f64 / n as f64;
                let tolerance = 6.0 * (expected * (1.0 - expected) / n as f64).sqrt() + 1e-4;
                assert!(
                    (observed - expected).abs() < tolerance,
                    "{observed} vs {expected}"
                );
            }
        }
    }
}

#[test]
fn ray_crossing_only_empty_blocks_does_not_consume_rng_despite_nonzero_global_bound() {
    let mut c = config([24, 16, 16], vec![0.0; 6144]);
    c.densities[12 + 24 * (12 + 16 * 12)] = 100.0;
    let grid = GridMedium::try_from(&c).unwrap();
    assert_eq!(grid.sigma_t_majorant(550.0), 100.0);
    let mut rng = StdRng::seed_from_u64(2);
    let mut untouched = rng.clone();
    for r in [
        ray([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0]),
        ray([25.0, 0.5, 0.5], [-1.0, 0.0, 0.0]),
    ] {
        let sample = grid
            .sample_segment(&r, 40.0, 550.0, true, &mut rng)
            .unwrap();
        assert!(sample.position.is_none());
        assert_eq!(sample.weight, 1.0);
        assert_eq!(
            grid.estimate_transmittance_ratio(&r, 40.0, 550.0, &mut rng)
                .unwrap(),
            1.0
        );
    }
    assert_eq!(rng.next_u64(), untouched.next_u64());
}

#[test]
fn ratio_tracking_matches_analytic_spectral_layers_across_vacuum_and_partial_blocks() {
    let mut densities = vec![0.15; 8];
    densities[0] = 0.6;
    densities.extend([0.0; 8]);
    densities.push(0.8);
    let mut c = config([17, 1, 1], densities);
    c.sigma_s_base = MediumCoefficient::Curve {
        samples: vec![[380.0, 0.0], [780.0, 0.25]],
    };
    let grid = GridMedium::try_from(&c).unwrap();
    // Both rays include x in [0.5, 17], with clipping at different ends.
    let rays = [
        (ray([0.5, 0.5, 0.5], [1.0, 0.0, 0.0]), 20.0),
        (ray([18.0, 0.5, 0.5], [-1.0, 0.0, 0.0]), 17.5),
    ];
    let samples = 20_000;
    for (nm, base_extinction) in [(380.0, 0.25_f64), (780.0, 0.5)] {
        let depth = base_extinction * (0.5 * 0.6 + 7.0 * 0.15 + 0.8);
        let expected = (-depth).exp();
        // For Poisson candidates, Var[Tr_hat] = Tr² expm1(∫ σt²/M ds).
        let squared_depth = base_extinction * (0.5 * 0.6 + 7.0 * 0.15 * 0.15 / 0.6 + 0.8);
        let variance = expected.powi(2) * squared_depth.exp_m1();
        for (ray, limit) in &rays {
            close(grid.transmittance_ray(ray, *limit, nm), expected);
            let mut sum = 0.0;
            let mut saw_fraction = false;
            for seed in 0..samples {
                let mut rng = StdRng::seed_from_u64(seed);
                let weight = grid
                    .estimate_transmittance_ratio(ray, *limit, nm, &mut rng)
                    .unwrap();
                assert!((0.0..=1.0).contains(&weight));
                saw_fraction |= weight > 0.0 && weight < 1.0;
                sum += weight;
            }
            assert!(saw_fraction);
            let mean = sum / samples as f64;
            let tolerance = 6.0 * (variance / samples as f64).sqrt();
            assert!((mean - expected).abs() < tolerance, "{mean} vs {expected}");
        }
    }
}

#[test]
fn ratio_tracking_returns_one_without_rng_for_empty_segments_and_vacuum() {
    let solid = GridMedium::try_from(&config([1; 3], vec![1.0])).unwrap();
    let vacuum = GridMedium::try_from(&config([1; 3], vec![0.0])).unwrap();
    let ray = ray([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0]);
    let mut rng = StdRng::seed_from_u64(2);
    let mut untouched = rng.clone();
    for (grid, distance) in [(&solid, 1.0), (&vacuum, 3.0)] {
        assert_eq!(
            grid.estimate_transmittance_ratio(&ray, distance, 550.0, &mut rng)
                .unwrap(),
            1.0
        );
    }
    assert_eq!(rng.next_u64(), untouched.next_u64());
}

#[test]
#[ignore = "manual Release benchmark; requires GRID_BENCHMARK_CONFIG from the benchmark example"]
fn benchmark_block_sizes() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::{config::Config, medium::MediumConfig};
    use std::{hint::black_box, time::Instant};
    let c = Config::load(std::env::var("GRID_BENCHMARK_CONFIG")?)?;
    let Some(MediumConfig::Grid(mut c)) = c.scene.medium else {
        return Err("expected a grid configuration".into());
    };
    // Same unit-box coefficients as the public sampling benchmark, independent
    // of the fog scene's world scale used for whole-render timing.
    c.min_m = [-1.0; 3];
    c.max_m = [1.0; 3];
    c.sigma_a_base = 0.05.into();
    c.sigma_s_base = 0.95.into();
    let mut rays_rng = StdRng::seed_from_u64(42);
    let rays: Vec<_> = (0..4096)
        .map(|i| {
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            Ray::new(
                Vector3::new(
                    -1.5 * sign,
                    rays_rng.random_range(-0.9..0.9),
                    rays_rng.random_range(-0.9..0.9),
                ),
                Vector3::new(
                    sign,
                    rays_rng.random_range(-0.1..0.1),
                    rays_rng.random_range(-0.1..0.1),
                ),
            )
            .unwrap()
        })
        .collect();
    for stride in [2, 4, 8] {
        let mut grid = GridMedium::try_from(&c)?;
        let start = Instant::now();
        grid.majorants = MajorantGrid::new(c.resolution, &c.densities, stride)?;
        let cache_build_seconds = start.elapsed().as_secs_f64();
        let mut warmup = StdRng::seed_from_u64(7);
        for ray in &rays {
            black_box(grid.sample_segment(ray, 3.0, 550.0, true, &mut warmup)?);
        }
        let mut seconds = Vec::new();
        for repeat in 0..5 {
            let mut rng = StdRng::seed_from_u64(17 + repeat);
            let start = Instant::now();
            for i in 0..100_000 {
                black_box(grid.sample_segment(
                    &rays[i % rays.len()],
                    3.0,
                    550.0,
                    true,
                    &mut rng,
                )?);
            }
            seconds.push(start.elapsed().as_secs_f64());
        }
        seconds.sort_by(f64::total_cmp);
        println!(
            "{}",
            serde_json::json!({"resolution": c.resolution, "requested_block_edge": stride,
            "effective_block_edge": grid.majorants.stride, "cache_bytes": grid.majorants.max_densities.capacity() * size_of::<f64>(),
            "cache_build_seconds": cache_build_seconds, "sampling_median_seconds": seconds[2],
            "sampling_seconds": seconds, "rays": 100_000, "repeats": 5, "ray_seed": 42, "sampling_seed": 17})
        );
    }
    Ok(())
}

//! Bake a seeded fog clump into a piecewise constant density grid.
use nalgebra::Vector3;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

pub(super) fn generate(
    resolution: [usize; 3],
    count: usize,
    seed: u64,
) -> Result<Vec<f64>, String> {
    let mut densities = Vec::new();
    densities
        .try_reserve_exact(count)
        .map_err(|_| "cannot allocate procedural grid densities")?;
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let clumps: [_; 6] = std::array::from_fn(|_| {
        let center = Vector3::from_fn(|_, _| rng.random_range(-0.55..0.55));
        let widths = Vector3::from_fn(|_, _| rng.random_range(0.18..0.36));
        let amplitude = rng.random_range(0.55..1.2);
        (center, widths, amplitude)
    });

    let [nx, ny, nz] = resolution;
    let center = |i: usize, n: usize| 2.0 * ((i as f64 + 0.5) / n as f64) - 1.0;
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let p = Vector3::new(center(x, nx), center(y, ny), center(z, nz));
                let radius_squared = p.norm_squared();
                let density = if radius_squared < 1.0 {
                    // Smooth envelope with compact support in the box's inscribed ellipsoid.
                    let edge = (2.0 * (1.0 - radius_squared)).min(1.0);
                    let envelope = edge * edge * (3.0 - 2.0 * edge);
                    let detail: f64 = clumps
                        .iter()
                        .map(|(center, widths, amplitude)| {
                            let q = (p - center).component_div(widths);
                            amplitude * (-0.5 * q.norm_squared()).exp()
                        })
                        .sum();
                    envelope * (0.12 + detail)
                } else {
                    0.0
                };
                densities.push(density);
            }
        }
    }
    Ok(densities)
}

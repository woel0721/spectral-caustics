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
        let inv_widths = Vector3::new(1.0 / widths.x, 1.0 / widths.y, 1.0 / widths.z);
        let amplitude = rng.random_range(0.55..1.2);
        (center, inv_widths, amplitude)
    });

    let [nx, ny, nz] = resolution;
    let inv_nx = 2.0 / nx as f64;
    let inv_ny = 2.0 / ny as f64;
    let inv_nz = 2.0 / nz as f64;
    for z in 0..nz {
        let pz = (z as f64 + 0.5) * inv_nz - 1.0;
        let pz2 = pz * pz;
        for y in 0..ny {
            let py = (y as f64 + 0.5) * inv_ny - 1.0;
            let pyz2 = py * py + pz2;
            if pyz2 >= 1.0 {
                densities.resize(densities.len() + nx, 0.0);
                continue;
            }
            let row_clumps: [_; 6] = std::array::from_fn(|i| {
                let (center, inv_widths, amplitude) = clumps[i];
                let qy = (py - center.y) * inv_widths.y;
                let qz = (pz - center.z) * inv_widths.z;
                (center.x, inv_widths.x, qy * qy + qz * qz, amplitude)
            });
            for x in 0..nx {
                let px = (x as f64 + 0.5) * inv_nx - 1.0;
                let radius_squared = px * px + pyz2;
                let density = if radius_squared < 1.0 {
                    // Smooth envelope with compact support in the box's inscribed ellipsoid.
                    let edge = (2.0 * (1.0 - radius_squared)).min(1.0);
                    let envelope = edge * edge * (3.0 - 2.0 * edge);
                    let mut detail = 0.0;
                    for &(cx, inv_wx, qyz2, amplitude) in &row_clumps {
                        let qx = (px - cx) * inv_wx;
                        detail += amplitude * (-0.5 * (qx * qx + qyz2)).exp();
                    }
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

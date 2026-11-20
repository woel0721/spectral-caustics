//! Piecewise constant density on a world-space axis-aligned grid. No interpolation.
use super::{
    GridMediumConfig, Medium, MediumCoefficients, MediumSample, ProceduralMediumConfig,
    bounds::{box_interval, contains, valid_bounds},
    phase::HenyeyGreenstein,
    procedural,
    spectral::SpectralCoefficients,
};
use crate::geometry::ray::Ray;
use nalgebra::Vector3;
use rand::{Rng, RngCore, distr::Open01};
use std::borrow::Cow;

mod dda;
#[cfg(test)]
mod tests;
use dda::{MajorantDda, VoxelDda};

// Internal tuning only; no transport/configuration API depends on this layout.
const MAJORANT_BLOCK_EDGE: usize = 8;

#[derive(Debug, Clone)]
struct MajorantGrid {
    stride: usize,
    resolution: [usize; 3],
    max_densities: Vec<f64>,
}

impl MajorantGrid {
    fn new(fine: [usize; 3], densities: &[f64], stride: usize) -> Result<Self, String> {
        let resolution = fine.map(|n| n.div_ceil(stride));
        // Each coarse axis is <= its already validated fine axis.
        let count = resolution.iter().product();
        let mut max_densities = Vec::new();
        max_densities
            .try_reserve_exact(count)
            .map_err(|_| "cannot allocate local majorants")?;
        max_densities.resize(count, 0.0_f64);
        for (i, &density) in densities.iter().enumerate() {
            let cell = [
                i % fine[0],
                (i / fine[0]) % fine[1],
                i / (fine[0] * fine[1]),
            ]
            .map(|v| v / stride);
            let index = cell[0] + resolution[0] * (cell[1] + resolution[1] * cell[2]);
            max_densities[index] = max_densities[index].max(density);
        }
        // Equal block maxima give identical bounds at every wavelength.
        if max_densities.iter().all(|&m| m == max_densities[0]) {
            max_densities.truncate(1);
            max_densities.shrink_to_fit();
            return Ok(Self {
                stride: *fine.iter().max().unwrap(),
                resolution: [1; 3],
                max_densities,
            });
        }
        Ok(Self {
            stride,
            resolution,
            max_densities,
        })
    }

    fn max_density_at(&self, cell: [usize; 3]) -> f64 {
        self.max_densities[cell[0] + self.resolution[0] * (cell[1] + self.resolution[1] * cell[2])]
    }
}

#[derive(Debug, Clone)]
pub struct GridMedium {
    min: Vector3<f64>,
    max: Vector3<f64>,
    resolution: [usize; 3],
    densities: Vec<f64>,
    base: SpectralCoefficients,
    max_density: f64,
    majorants: MajorantGrid,
    phase: HenyeyGreenstein,
}

impl TryFrom<&GridMediumConfig> for GridMedium {
    type Error = String;
    fn try_from(c: &GridMediumConfig) -> Result<Self, String> {
        Self::build(
            c.min_m,
            c.max_m,
            c.resolution,
            SpectralCoefficients::new(&c.sigma_a_base, &c.sigma_s_base)?,
            c.g,
            |_| Ok(Cow::Borrowed(&c.densities)),
        )
    }
}

impl TryFrom<&ProceduralMediumConfig> for GridMedium {
    type Error = String;
    fn try_from(c: &ProceduralMediumConfig) -> Result<Self, String> {
        Self::build(
            c.min_m,
            c.max_m,
            c.resolution,
            SpectralCoefficients::new(&c.sigma_a_base, &c.sigma_s_base)?,
            c.g,
            |count| procedural::generate(c.resolution, count, c.procedural.seed).map(Cow::Owned),
        )
    }
}

impl GridMedium {
    fn build<'a>(
        min: [f64; 3],
        max: [f64; 3],
        resolution: [usize; 3],
        base: SpectralCoefficients,
        g: f64,
        density_source: impl FnOnce(usize) -> Result<Cow<'a, [f64]>, String>,
    ) -> Result<Self, String> {
        if !valid_bounds(min, max) {
            return Err("grid medium requires finite bounds min < max".into());
        }
        let count = resolution
            .iter()
            .try_fold(1usize, |n, &r| if r == 0 { None } else { n.checked_mul(r) })
            .ok_or("grid resolution must be positive and not overflow")?;
        let phase = HenyeyGreenstein::new(g)?;
        let densities = density_source(count)?;
        if count != densities.len() {
            return Err("grid resolution must match densities length".into());
        }
        if densities.iter().any(|d| !d.is_finite() || *d < 0.0) {
            return Err("grid densities must be nonnegative and finite".into());
        }
        let max_density = densities.iter().copied().fold(0.0, f64::max);
        if !base.has_finite_extinction(max_density) {
            return Err("grid density-scaled extinction overflows".into());
        }
        let densities = match densities {
            Cow::Owned(densities) => densities,
            Cow::Borrowed(source) => {
                let mut densities = Vec::new();
                densities
                    .try_reserve_exact(count)
                    .map_err(|_| "cannot allocate grid densities")?;
                densities.extend_from_slice(source);
                densities
            }
        };
        let majorants = MajorantGrid::new(resolution, &densities, MAJORANT_BLOCK_EDGE)?;
        let m = Self {
            min: min.into(),
            max: max.into(),
            resolution,
            densities,
            base,
            max_density,
            majorants,
            phase,
        };
        // Reject cells collapsed by world-coordinate rounding. DDA must always
        // advance a voxel index, and each configured cell must have positive width.
        for axis in 0..3 {
            let mut previous = m.min[axis];
            for i in 1..=m.resolution[axis] {
                let face = m.face(axis, i);
                if face <= previous {
                    return Err("grid voxel width is below world-coordinate precision".into());
                }
                previous = face;
            }
        }
        Ok(m)
    }
    fn face(&self, axis: usize, i: usize) -> f64 {
        if i == self.resolution[axis] {
            self.max[axis]
        } else {
            self.min[axis]
                + (self.max[axis] - self.min[axis]) * (i as f64 / self.resolution[axis] as f64)
        }
    }
    fn index(&self, cell: [usize; 3]) -> usize {
        cell[0] + self.resolution[0] * (cell[1] + self.resolution[1] * cell[2])
    }
    fn cell_at(&self, position: Vector3<f64>) -> [usize; 3] {
        std::array::from_fn(|axis| {
            let q = (position[axis] - self.min[axis]) / (self.max[axis] - self.min[axis]);
            let mut i = ((q * self.resolution[axis] as f64).floor() as usize)
                .min(self.resolution[axis] - 1);
            // Use the same rounded world-space faces as DDA, including for
            // non-binary cell sizes where division can round to the other side.
            while i + 1 < self.resolution[axis] && position[axis] >= self.face(axis, i + 1) {
                i += 1;
            }
            while i > 0 && position[axis] < self.face(axis, i) {
                i -= 1;
            }
            i
        })
    }
    pub fn interval(&self, ray: &Ray, max_distance: f64) -> Option<(f64, f64)> {
        box_interval(self.min, self.max, ray, max_distance)
    }

    /// Exact Beer–Lambert for piecewise constant cells, up to floating-point rounding.
    fn transmittance_interval(
        &self,
        ray: &Ray,
        enter: f64,
        exit: f64,
        base: MediumCoefficients,
    ) -> f64 {
        let mut tau = 0.0;
        for segment in VoxelDda::new(self, ray, enter, exit, 1) {
            let density = self.densities[self.index(segment.cell)];
            let sigma_t = base.scaled_by_density(density).sigma_t();
            // Vacuum contributes zero even if the exit distance overflows.
            if sigma_t > 0.0 {
                tau += sigma_t * (segment.end - segment.start);
            }
        }
        (-tau).exp()
    }
}

impl Medium for GridMedium {
    fn is_wavelength_dependent(&self) -> bool {
        self.base.is_wavelength_dependent()
    }

    /// Outside the closed AABB is vacuum. Internal faces belong to the higher
    /// index, and the outer max face belongs to the last voxel (as in homogeneous).
    fn density_at(&self, position: Vector3<f64>) -> f64 {
        if !contains(self.min, self.max, position) {
            return 0.0;
        }
        self.densities[self.index(self.cell_at(position))]
    }
    fn coefficients_at(&self, position: Vector3<f64>, wavelength_nm: f64) -> MediumCoefficients {
        self.base
            .at(wavelength_nm)
            .scaled_by_density(self.density_at(position))
    }
    fn sigma_t_majorant(&self, wavelength_nm: f64) -> f64 {
        self.base
            .at(wavelength_nm)
            .scaled_by_density(self.max_density)
            .sigma_t()
    }
    fn phase(&self) -> HenyeyGreenstein {
        self.phase
    }
    /// Piecewise-majorant tracking samples real extinction with density σt(x) Tr(x).
    /// Null collisions only advance this internal walk. Implicit absorption gives
    /// σs(x)/σt(x) at a real event; surviving the segment has compensated weight 1.
    fn sample_segment(
        &self,
        ray: &Ray,
        max_distance: f64,
        wavelength_nm: f64,
        allow_scattering: bool,
        rng: &mut dyn RngCore,
    ) -> Result<MediumSample, String> {
        let Some((enter, exit)) = self.interval(ray, max_distance) else {
            return Ok(MediumSample {
                position: None,
                weight: 1.0,
            });
        };
        let base = self.base.at(wavelength_nm);
        let maximum = base.scaled_by_density(self.max_density);
        if !maximum.sigma_t().is_finite() {
            return Err("grid density-scaled extinction overflows".into());
        }
        if !allow_scattering || maximum.sigma_s == 0.0 {
            return Ok(MediumSample {
                position: None,
                weight: self.transmittance_interval(ray, enter, exit, base),
            });
        }
        if !enter.is_finite() || !exit.is_finite() {
            return Err("grid collision interval exceeds finite ray distances".into());
        }
        // Carry unit-exponential optical depth, not metres, across rate changes.
        // Draw lazily: even a nonempty ray may only cross zero-majorant blocks.
        let mut remaining = None;
        for segment in MajorantDda::new(self, ray, enter, exit, self.majorants.stride) {
            // Use identical arithmetic for block and voxel extinction so rounding
            // preserves the bound. Only the density differs.
            let majorant = base
                .scaled_by_density(self.majorants.max_density_at(segment.cell))
                .sigma_t();
            if majorant == 0.0 {
                continue;
            }
            let mut t = segment.start;
            loop {
                let tau = remaining.get_or_insert_with(|| {
                    let u: f64 = rng.sample(Open01);
                    -(-u).ln_1p()
                });
                let depth = majorant * (segment.end - t);
                if *tau >= depth {
                    *tau -= depth;
                    break;
                }
                let next = t + *tau / majorant;
                if next <= t || next >= segment.end {
                    return Err(
                        "grid collision distance cannot advance inside segment at f64 precision"
                            .into(),
                    );
                }
                t = next;
                let position = ray.at(t);
                if position.iter().any(|v| !v.is_finite()) {
                    return Err("grid collision position overflows".into());
                }
                let coefficients = base.scaled_by_density(self.density_at(position));
                let sigma_t = coefficients.sigma_t();
                if sigma_t > majorant {
                    return Err("rounded grid collision lies outside its local majorant".into());
                }
                if rng.random::<f64>() < sigma_t / majorant {
                    return Ok(MediumSample {
                        position: Some(position),
                        weight: coefficients.sigma_s / sigma_t,
                    });
                }
                // Null events stay inside this method, without a candidate cap.
                remaining = None;
            }
        }
        Ok(MediumSample {
            position: None,
            weight: 1.0,
        })
    }
    fn transmittance_ray(&self, ray: &Ray, max_distance: f64, wavelength_nm: f64) -> f64 {
        self.interval(ray, max_distance)
            .map_or(1.0, |(enter, exit)| {
                self.transmittance_interval(ray, enter, exit, self.base.at(wavelength_nm))
            })
    }
}

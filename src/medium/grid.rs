//! Piecewise constant density on a world-space axis-aligned grid. No interpolation.
use super::{
    GridMediumConfig, HgAnisotropy, Medium, MediumCoefficients, MediumSample,
    ProceduralMediumConfig,
    bounds::{box_interval, contains, valid_bounds},
    phase::{HenyeyGreenstein, SpectralHg},
    procedural,
    spectral::SpectralCoefficients,
};
use crate::geometry::ray::Ray;
use nalgebra::Vector3;
use rand::{Rng, RngCore, distr::Open01};
use std::{borrow::Cow, ops::ControlFlow};

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
    has_empty_blocks: bool,
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
        let [fx, fy, fz] = fine;
        let [rx, ry, _] = resolution;
        let mut offset = 0;
        for z in 0..fz {
            let cz_base = (z / stride) * ry;
            for y in 0..fy {
                let cy_base = (cz_base + y / stride) * rx;
                let row = &densities[offset..offset + fx];
                offset += fx;
                for (cx, chunk) in row.chunks(stride).enumerate() {
                    let mut m = max_densities[cy_base + cx];
                    for &density in chunk {
                        m = m.max(density);
                    }
                    max_densities[cy_base + cx] = m;
                }
            }
        }
        // Equal block maxima give identical bounds at every wavelength.
        if max_densities.iter().all(|&m| m == max_densities[0]) {
            let has_empty_blocks = max_densities[0] == 0.0;
            max_densities.truncate(1);
            max_densities.shrink_to_fit();
            return Ok(Self {
                stride: *fine.iter().max().unwrap(),
                resolution: [1; 3],
                max_densities,
                has_empty_blocks,
            });
        }
        let has_empty_blocks = max_densities.contains(&0.0);
        Ok(Self {
            stride,
            resolution,
            max_densities,
            has_empty_blocks,
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
    inv_extent: Vector3<f64>,
    resolution: [usize; 3],
    faces: [Vec<f64>; 3],
    densities: Vec<f64>,
    base: SpectralCoefficients,
    max_density: f64,
    majorants: MajorantGrid,
    phase: SpectralHg,
}

impl TryFrom<&GridMediumConfig> for GridMedium {
    type Error = String;
    fn try_from(c: &GridMediumConfig) -> Result<Self, String> {
        Self::build(
            c.min_m,
            c.max_m,
            c.resolution,
            SpectralCoefficients::new(&c.sigma_a_base, &c.sigma_s_base)?,
            &c.g,
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
            &c.g,
            |count| procedural::generate(c.resolution, count, c.procedural.seed).map(Cow::Owned),
        )
    }
}

pub(super) fn validate_grid_layout(
    min: [f64; 3],
    max: [f64; 3],
    resolution: [usize; 3],
) -> Result<usize, String> {
    if !valid_bounds(min, max) {
        return Err("grid medium requires finite bounds min < max".into());
    }
    let count = resolution
        .iter()
        .try_fold(1usize, |n, &r| if r == 0 { None } else { n.checked_mul(r) })
        .filter(|&n| {
            n.checked_mul(std::mem::size_of::<f64>())
                .is_some_and(|bytes| bytes <= isize::MAX as usize)
        })
        .ok_or("grid resolution must be positive and not overflow")?;
    for axis in 0..3 {
        let n = resolution[axis];
        n.checked_add(1)
            .ok_or("grid resolution must be positive and not overflow")?;
        let extent = max[axis] - min[axis];
        let mut previous = min[axis];
        for i in 1..=n {
            let face = if i == n {
                max[axis]
            } else {
                min[axis] + extent * (i as f64 / n as f64)
            };
            if face <= previous {
                return Err("grid voxel width is below world-coordinate precision".into());
            }
            previous = face;
        }
    }
    Ok(count)
}

pub(super) fn validate_densities(
    densities: &[f64],
    count: usize,
    base: &SpectralCoefficients,
) -> Result<f64, String> {
    if count != densities.len() {
        return Err("grid resolution must match densities length".into());
    }
    let mut max_density = 0.0_f64;
    for &d in densities {
        if !d.is_finite() || d < 0.0 {
            return Err("grid densities must be nonnegative and finite".into());
        }
        max_density = max_density.max(d);
    }
    if !base.has_finite_extinction(max_density) {
        return Err("grid density-scaled extinction overflows".into());
    }
    Ok(max_density)
}

impl GridMedium {
    fn build<'a>(
        min: [f64; 3],
        max: [f64; 3],
        resolution: [usize; 3],
        base: SpectralCoefficients,
        g: &HgAnisotropy,
        density_source: impl FnOnce(usize) -> Result<Cow<'a, [f64]>, String>,
    ) -> Result<Self, String> {
        let count = validate_grid_layout(min, max, resolution)?;
        let phase = SpectralHg::new(g)?;
        let min_vec = Vector3::from(min);
        let max_vec = Vector3::from(max);
        let extent = max_vec - min_vec;
        let inv_extent = Vector3::new(1.0 / extent.x, 1.0 / extent.y, 1.0 / extent.z);
        let mut faces: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for axis in 0..3 {
            let n = resolution[axis];
            let mut axis_faces = Vec::new();
            axis_faces
                .try_reserve_exact(n + 1)
                .map_err(|_| "cannot allocate grid voxel faces")?;
            axis_faces.push(min_vec[axis]);
            for i in 1..n {
                axis_faces.push(min_vec[axis] + extent[axis] * (i as f64 / n as f64));
            }
            axis_faces.push(max_vec[axis]);
            faces[axis] = axis_faces;
        }
        let densities = density_source(count)?;
        let max_density = validate_densities(&densities, count, &base)?;
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
        Ok(Self {
            min: min_vec,
            max: max_vec,
            inv_extent,
            resolution,
            faces,
            densities,
            base,
            max_density,
            majorants,
            phase,
        })
    }
    #[inline]
    fn face(&self, axis: usize, i: usize) -> f64 {
        self.faces[axis][i]
    }
    #[inline]
    fn index(&self, cell: [usize; 3]) -> usize {
        cell[0] + self.resolution[0] * (cell[1] + self.resolution[1] * cell[2])
    }
    #[inline]
    fn cell_at(&self, position: Vector3<f64>) -> [usize; 3] {
        std::array::from_fn(|axis| {
            let n = self.resolution[axis];
            let faces = &self.faces[axis];
            let q = (position[axis] - self.min[axis]) * self.inv_extent[axis];
            let mut i = ((q * n as f64).floor() as usize).min(n - 1);
            // Use the same rounded world-space faces as DDA, including for
            // non-binary cell sizes where division can round to the other side.
            while i + 1 < n && position[axis] >= faces[i + 1] {
                i += 1;
            }
            while i > 0 && position[axis] < faces[i] {
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
        if self.max_density == 0.0 || base.sigma_t() == 0.0 {
            return 1.0;
        }
        let mut tau = 0.0;
        let accumulate = |start: f64, end: f64, tau: &mut f64| -> bool {
            for segment in VoxelDda::new(self, ray, start, end, 1) {
                let density = self.densities[self.index(segment.cell)];
                // Vacuum contributes zero even if the exit distance overflows.
                if density > 0.0 {
                    let sigma_t = base.scaled_by_density(density).sigma_t();
                    if sigma_t > 0.0 {
                        *tau += sigma_t * (segment.end - segment.start);
                        // exp(-746) underflows to exact 0.0 in IEEE-754 f64.
                        if *tau >= 746.0 {
                            return true;
                        }
                    }
                }
            }
            false
        };
        if !self.majorants.has_empty_blocks {
            if accumulate(enter, exit, &mut tau) {
                return 0.0;
            }
        } else {
            let mut active: Option<(f64, f64)> = None;
            for coarse in MajorantDda::with_resolution(
                self,
                ray,
                enter,
                exit,
                self.majorants.stride,
                self.majorants.resolution,
            ) {
                if self.majorants.max_density_at(coarse.cell) == 0.0 {
                    if let Some((start, end)) = active.take()
                        && accumulate(start, end, &mut tau)
                    {
                        return 0.0;
                    }
                    continue;
                }
                match &mut active {
                    Some((_, end)) => *end = coarse.end,
                    None => active = Some((coarse.start, coarse.end)),
                }
            }
            if let Some((start, end)) = active
                && accumulate(start, end, &mut tau)
            {
                return 0.0;
            }
        }
        (-tau).exp()
    }

    /// Estimate beam transmittance with one Ratio Tracking sample in [0, 1].
    /// Uses total extinction at a wavelength in 380–780 nm and clips the ray
    /// to the medium and the given maximum distance in metres.
    /// Use [Medium::transmittance_ray] for deterministic integration.
    pub fn estimate_transmittance_ratio(
        &self,
        ray: &Ray,
        max_distance: f64,
        wavelength_nm: f64,
        rng: &mut dyn RngCore,
    ) -> Result<f64, String> {
        let Some(interval) = self.interval(ray, max_distance) else {
            return Ok(1.0);
        };
        let base = self.base.at(wavelength_nm);
        let maximum = base.scaled_by_density(self.max_density).sigma_t();
        if !maximum.is_finite() {
            return Err("grid density-scaled extinction overflows".into());
        }
        if maximum == 0.0 {
            return Ok(1.0);
        }

        let mut weight = 1.0;
        let stopped = self.visit_tracking_candidates(ray, interval, base, rng, |_, c, m, _| {
            // Subtract before dividing to retain precision near the majorant.
            weight *= (m - c.sigma_t()) / m;
            if weight == 0.0 {
                return ControlFlow::Break(0.0);
            }
            ControlFlow::Continue(())
        })?;
        Ok(stopped.unwrap_or(weight))
    }

    fn visit_tracking_candidates<T>(
        &self,
        ray: &Ray,
        (enter, exit): (f64, f64),
        base: MediumCoefficients,
        rng: &mut dyn RngCore,
        mut visit: impl FnMut(Vector3<f64>, MediumCoefficients, f64, &mut dyn RngCore) -> ControlFlow<T>,
    ) -> Result<Option<T>, String> {
        if !enter.is_finite() || !exit.is_finite() {
            return Err("grid collision interval exceeds finite ray distances".into());
        }
        // Carry unit-exponential optical depth, not metres, across rate changes.
        // Draw lazily: even a nonempty ray may only cross zero-majorant blocks.
        let mut remaining = None;
        for segment in MajorantDda::with_resolution(
            self,
            ray,
            enter,
            exit,
            self.majorants.stride,
            self.majorants.resolution,
        ) {
            // Use identical arithmetic for block and voxel extinction so rounding
            // preserves the bound. Only the density differs.
            let majorant = base
                .scaled_by_density(self.majorants.max_density_at(segment.cell))
                .sigma_t();
            if majorant == 0.0 {
                continue;
            }
            let inv_majorant = 1.0 / majorant;
            let use_inv = inv_majorant.is_finite();
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
                let next = if use_inv {
                    t + *tau * inv_majorant
                } else {
                    t + *tau / majorant
                };
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
                if coefficients.sigma_t() > majorant {
                    return Err("rounded grid collision lies outside its local majorant".into());
                }
                if let ControlFlow::Break(result) = visit(position, coefficients, majorant, rng) {
                    return Ok(Some(result));
                }
                remaining = None;
            }
        }
        Ok(None)
    }
}

impl Medium for GridMedium {
    fn is_wavelength_dependent(&self) -> bool {
        self.has_wavelength_dependent_coefficients() || self.phase.is_wavelength_dependent()
    }

    fn has_wavelength_dependent_coefficients(&self) -> bool {
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
    fn phase(&self, wavelength_nm: f64) -> HenyeyGreenstein {
        self.phase.at(wavelength_nm)
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
        let collision = self.visit_tracking_candidates(
            ray,
            (enter, exit),
            base,
            rng,
            |position, coefficients, majorant, rng| {
                let sigma_t = coefficients.sigma_t();
                if rng.random::<f64>() < sigma_t / majorant {
                    return ControlFlow::Break(MediumSample {
                        position: Some(position),
                        weight: coefficients.sigma_s / sigma_t,
                    });
                }
                ControlFlow::Continue(())
            },
        )?;
        Ok(collision.unwrap_or(MediumSample {
            position: None,
            weight: 1.0,
        }))
    }
    fn transmittance_ray(&self, ray: &Ray, max_distance: f64, wavelength_nm: f64) -> f64 {
        self.interval(ray, max_distance)
            .map_or(1.0, |(enter, exit)| {
                self.transmittance_interval(ray, enter, exit, self.base.at(wavelength_nm))
            })
    }
}

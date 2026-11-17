//! Piecewise constant density on a world-space axis-aligned grid. No interpolation.
use super::{
    GridMediumConfig, Medium, MediumCoefficients, MediumSample,
    bounds::{box_interval, contains, valid_bounds},
    phase::HenyeyGreenstein,
};
use crate::geometry::ray::Ray;
use nalgebra::Vector3;
use rand::{Rng, RngCore, distr::Open01};

#[derive(Debug, Clone)]
pub struct GridMedium {
    min: Vector3<f64>,
    max: Vector3<f64>,
    resolution: [usize; 3],
    densities: Vec<f64>,
    base: MediumCoefficients,
    max_density: f64,
    sigma_t_majorant: f64,
    phase: HenyeyGreenstein,
}

impl TryFrom<&GridMediumConfig> for GridMedium {
    type Error = String;
    fn try_from(c: &GridMediumConfig) -> Result<Self, String> {
        if !valid_bounds(c.min_m, c.max_m) {
            return Err("grid medium requires finite bounds min < max".into());
        }
        let count = c
            .resolution
            .iter()
            .try_fold(1usize, |n, &r| if r == 0 { None } else { n.checked_mul(r) });
        if count != Some(c.densities.len()) {
            return Err(
                "grid resolution must be positive, not overflow, and match densities length".into(),
            );
        }
        if c.densities.iter().any(|d| !d.is_finite() || *d < 0.0)
            || [
                c.sigma_a_base,
                c.sigma_s_base,
                c.sigma_a_base + c.sigma_s_base,
            ]
            .iter()
            .any(|s| !s.is_finite() || *s < 0.0)
        {
            return Err(
                "grid densities and base coefficients must be nonnegative and finite".into(),
            );
        }
        let max_density = c.densities.iter().copied().fold(0.0, f64::max);
        // Same operations as coefficients_at().sigma_t(): monotonic products/sum
        // at the maximum density cover every voxel, including roundoff.
        let sigma_t_majorant = max_density * c.sigma_a_base + max_density * c.sigma_s_base;
        if !sigma_t_majorant.is_finite() {
            return Err("grid density-scaled extinction overflows".into());
        }
        let mut densities = Vec::new();
        densities
            .try_reserve_exact(c.densities.len())
            .map_err(|_| "cannot allocate grid densities")?;
        densities.extend_from_slice(&c.densities);
        let m = Self {
            min: c.min_m.into(),
            max: c.max_m.into(),
            resolution: c.resolution,
            densities,
            base: MediumCoefficients {
                sigma_a: c.sigma_a_base,
                sigma_s: c.sigma_s_base,
            },
            max_density,
            sigma_t_majorant,
            phase: HenyeyGreenstein::new(c.g)?,
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
}

impl GridMedium {
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
    fn scaled_coefficients(&self, density: f64) -> MediumCoefficients {
        MediumCoefficients {
            sigma_a: density * self.base.sigma_a,
            sigma_s: density * self.base.sigma_s,
        }
    }
    pub fn interval(&self, ray: &Ray, max_distance: f64) -> Option<(f64, f64)> {
        box_interval(self.min, self.max, ray, max_distance)
    }
}

impl Medium for GridMedium {
    /// Outside the closed AABB is vacuum. Internal faces belong to the higher
    /// index, and the outer max face belongs to the last voxel (as in homogeneous).
    fn density_at(&self, position: Vector3<f64>) -> f64 {
        if !contains(self.min, self.max, position) {
            return 0.0;
        }
        self.densities[self.index(self.cell_at(position))]
    }
    fn coefficients_at(&self, position: Vector3<f64>) -> MediumCoefficients {
        self.scaled_coefficients(self.density_at(position))
    }
    fn sigma_t_majorant(&self) -> f64 {
        self.sigma_t_majorant
    }
    fn phase(&self) -> HenyeyGreenstein {
        self.phase
    }
    /// Delta tracking samples the first real extinction with density σt(x) Tr(x).
    /// Null collisions only advance this internal walk. Implicit absorption gives
    /// σs(x)/σt(x) at a real event; surviving the segment has compensated weight 1.
    fn sample_segment(
        &self,
        ray: &Ray,
        max_distance: f64,
        allow_scattering: bool,
        rng: &mut dyn RngCore,
    ) -> Result<MediumSample, String> {
        if !allow_scattering || self.max_density * self.base.sigma_s == 0.0 {
            return Ok(MediumSample {
                position: None,
                weight: self.transmittance_ray(ray, max_distance),
            });
        }
        let Some((mut t, exit)) = self.interval(ray, max_distance) else {
            return Ok(MediumSample {
                position: None,
                weight: 1.0,
            });
        };
        if !t.is_finite() || !exit.is_finite() {
            return Err("grid collision interval exceeds finite ray distances".into());
        }
        loop {
            // Open01 excludes a zero exponential step. Do not impose a candidate
            // count cap: dropping paths after many null events would bias sampling.
            let u: f64 = rng.sample(Open01);
            let next = t - (-u).ln_1p() / self.sigma_t_majorant;
            if next >= exit {
                return Ok(MediumSample {
                    position: None,
                    weight: 1.0,
                });
            }
            if next <= t {
                return Err("grid collision distance cannot advance at f64 precision".into());
            }
            t = next;
            let position = ray.at(t);
            if position.iter().any(|v| !v.is_finite()) {
                return Err("grid collision position overflows".into());
            }
            let coefficients = self.coefficients_at(position);
            let sigma_t = coefficients.sigma_t();
            if rng.random::<f64>() < sigma_t / self.sigma_t_majorant {
                return Ok(MediumSample {
                    position: Some(position),
                    weight: coefficients.sigma_s / sigma_t,
                });
            }
        }
    }
    /// Exact Beer–Lambert for piecewise constant cells, up to floating-point
    /// rounding: tau = sum_i (sigma_a_i + sigma_s_i) * length_i, Tr = exp(-tau).
    /// DDA visits only crossed voxels; no fixed step, RNG, or collision sampling.
    fn transmittance_ray(&self, ray: &Ray, max_distance: f64) -> f64 {
        let Some((mut t, exit)) = self.interval(ray, max_distance) else {
            return 1.0;
        };
        let mut cell = self.cell_at(ray.at(t));
        let crossing = |axis: usize, cell: [usize; 3]| {
            let d = ray.direction[axis];
            if d == 0.0 {
                f64::INFINITY
            } else {
                let face = cell[axis] + usize::from(d > 0.0);
                (self.face(axis, face) - ray.origin[axis]) / d
            }
        };
        let mut next: [f64; 3] = std::array::from_fn(|axis| crossing(axis, cell));
        let mut tau = 0.0;
        loop {
            let end = next.iter().copied().fold(exit, f64::min).max(t);
            let sigma_t = self
                .scaled_coefficients(self.densities[self.index(cell)])
                .sigma_t();
            // A vacuum cell contributes zero even if a very distant box exit
            // overflows the representable ray distance (avoid 0 * infinity).
            if sigma_t > 0.0 {
                tau += sigma_t * (end - t);
            }
            if end >= exit {
                break;
            }
            // Advance every tied axis at edges/corners. Zero-length crossings
            // handle rays starting on a face in the negative direction, without epsilon.
            for axis in 0..3 {
                if next[axis] <= end {
                    if ray.direction[axis] > 0.0 {
                        cell[axis] += 1;
                        if cell[axis] == self.resolution[axis] {
                            return (-tau).exp();
                        }
                    } else {
                        let Some(previous) = cell[axis].checked_sub(1) else {
                            return (-tau).exp();
                        };
                        cell[axis] = previous;
                    }
                    next[axis] = crossing(axis, cell);
                }
            }
            t = end;
        }
        (-tau).exp()
    }
}

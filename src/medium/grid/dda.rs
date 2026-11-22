//! Private front-to-back grid traversal, shared by fine extinction and coarse majorants.
use super::GridMedium;
use crate::geometry::ray::Ray;

pub(super) struct Segment {
    pub start: f64,
    pub end: f64,
    pub cell: [usize; 3],
}

pub(super) type VoxelDda<'a> = GridDda<'a, true>;
pub(super) type MajorantDda<'a> = GridDda<'a, false>;

// Specialize fine traversal so its hot loop has no runtime stride divisions or
// partial-block multiplication/clamping. Both layouts share the same DDA logic.
pub(super) struct GridDda<'a, const FINE: bool> {
    grid: &'a GridMedium,
    ray: &'a Ray,
    inv_d: [f64; 3],
    stride: usize,
    resolution: [usize; 3],
    cell: [usize; 3],
    next: [f64; 3],
    t: f64,
    exit: f64,
    finished: bool,
}

impl<'a, const FINE: bool> GridDda<'a, FINE> {
    /// stride=1 visits fine voxels; larger strides group adjacent fine cells.
    /// Faces use fine-grid indices, so partial edge blocks are not stretched.
    #[inline]
    pub fn new(grid: &'a GridMedium, ray: &'a Ray, enter: f64, exit: f64, stride: usize) -> Self {
        let resolution = if FINE {
            grid.resolution
        } else {
            grid.resolution.map(|n| n.div_ceil(stride))
        };
        Self::with_resolution(grid, ray, enter, exit, stride, resolution)
    }

    #[inline]
    pub fn with_resolution(
        grid: &'a GridMedium,
        ray: &'a Ray,
        enter: f64,
        exit: f64,
        stride: usize,
        resolution: [usize; 3],
    ) -> Self {
        if grid.resolution.iter().all(|&n| n <= stride) {
            return Self {
                grid,
                ray,
                inv_d: [0.0; 3],
                stride,
                resolution: [1; 3],
                cell: [0; 3],
                next: [f64::INFINITY; 3],
                t: enter,
                exit,
                finished: false,
            };
        }
        let inv_d = std::array::from_fn(|axis| {
            let d = ray.direction[axis];
            if d == 0.0 {
                0.0
            } else {
                let inv = 1.0 / d;
                if inv.is_finite() { inv } else { 0.0 }
            }
        });
        let mut dda = Self {
            grid,
            ray,
            inv_d,
            stride,
            resolution,
            cell: if FINE {
                grid.cell_at(ray.at(enter))
            } else {
                grid.cell_at(ray.at(enter)).map(|i| i / stride)
            },
            next: [0.0; 3],
            t: enter,
            exit,
            finished: false,
        };
        dda.next = std::array::from_fn(|axis| dda.crossing(axis));
        dda
    }

    #[inline]
    fn crossing(&self, axis: usize) -> f64 {
        let d = self.ray.direction[axis];
        if d == 0.0 {
            return f64::INFINITY;
        }
        let mut face = self.cell[axis] + usize::from(d > 0.0);
        if !FINE {
            face = face
                .saturating_mul(self.stride)
                .min(self.grid.resolution[axis]);
        }
        let delta = self.grid.face(axis, face) - self.ray.origin[axis];
        let inv = self.inv_d[axis];
        if inv != 0.0 { delta * inv } else { delta / d }
    }
}

impl<const FINE: bool> Iterator for GridDda<'_, FINE> {
    type Item = Segment;

    #[inline]
    fn next(&mut self) -> Option<Segment> {
        while !self.finished {
            let end = self
                .next
                .iter()
                .copied()
                .fold(self.exit, f64::min)
                .max(self.t);
            let segment = Segment {
                start: self.t,
                end,
                cell: self.cell,
            };
            if end >= self.exit {
                self.finished = true;
            } else {
                // Every tied axis advances. Face starts in the negative direction
                // may have zero length; skip those after advancing the index.
                for axis in 0..3 {
                    if self.next[axis] <= end {
                        if self.ray.direction[axis] > 0.0 {
                            self.cell[axis] += 1;
                            if self.cell[axis] == self.resolution[axis] {
                                self.finished = true;
                                break;
                            }
                        } else {
                            let Some(previous) = self.cell[axis].checked_sub(1) else {
                                self.finished = true;
                                break;
                            };
                            self.cell[axis] = previous;
                        }
                        self.next[axis] = self.crossing(axis);
                    }
                }
            }
            self.t = end;
            if segment.end > segment.start {
                return Some(segment);
            }
        }
        None
    }
}

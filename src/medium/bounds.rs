//! Shared world-space box validation, containment and ray clipping.
use crate::geometry::ray::Ray;
use nalgebra::Vector3;

pub(super) fn valid_bounds(min: [f64; 3], max: [f64; 3]) -> bool {
    (0..3).all(|i| {
        min[i].is_finite() && max[i].is_finite() && min[i] < max[i] && (max[i] - min[i]).is_finite()
    })
}

pub(super) fn contains(min: Vector3<f64>, max: Vector3<f64>, p: Vector3<f64>) -> bool {
    (0..3).all(|i| p[i] >= min[i] && p[i] <= max[i])
}

/// Shared closed-box slab clipping. Ray directions must be unit length;
/// max_distance is a nonnegative world distance, possibly infinity.
pub(super) fn box_interval(
    min: Vector3<f64>,
    max: Vector3<f64>,
    ray: &Ray,
    max_distance: f64,
) -> Option<(f64, f64)> {
    if max_distance.is_nan() || max_distance <= 0.0 {
        return None;
    }
    let (mut enter, mut exit) = (0.0_f64, max_distance);
    for axis in 0..3 {
        let d = ray.direction[axis];
        let o = ray.origin[axis];
        if d == 0.0 {
            if o < min[axis] || o > max[axis] {
                return None;
            }
        } else {
            let a = (min[axis] - o) / d;
            let b = (max[axis] - o) / d;
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
        }
        if exit <= enter {
            return None;
        }
    }
    Some((enter, exit))
}

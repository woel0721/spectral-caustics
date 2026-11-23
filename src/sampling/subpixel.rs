//! Per-pixel Cranley–Patterson shifts of the 2D R2 sequence.
use super::rng::{CAMERA_STREAM, pixel_seed, unit_f64_from_u64};
use nalgebra::Vector2;

// Rounded 2^64 / phi_2 and 2^64 / phi_2^2, where phi_2^3 = phi_2 + 1.
const R2_X: u64 = 0xc13f_a9a9_02a6_328f;
const R2_Y: u64 = 0x91e1_0da5_c79e_7b1d;
const SHIFT_Y_STREAM: u64 = 0x3c79_ac49_2ba7_b653;
const PERMUTATION_STREAM: u64 = 0x76cf_49e1_2d89_43bb;

pub(crate) struct SubpixelSampler {
    shift_x: u64,
    shift_y: u64,
}

impl SubpixelSampler {
    pub(crate) fn new(seed: u64, pixel: usize) -> Self {
        Self {
            shift_x: pixel_seed(seed, pixel, CAMERA_STREAM),
            shift_y: pixel_seed(seed, pixel, CAMERA_STREAM ^ SHIFT_Y_STREAM),
        }
    }

    /// Return a pixel-local offset in [0, 1)^2. Wrapping implements modulo one in fixed point.
    #[inline]
    pub(crate) fn offset(&self, index: usize) -> Vector2<f64> {
        let k = index as u64;
        Vector2::new(
            unit_f64_from_u64(self.shift_x.wrapping_add(k.wrapping_mul(R2_X))),
            unit_f64_from_u64(self.shift_y.wrapping_add(k.wrapping_mul(R2_Y))),
        )
    }
}

pub(crate) fn permutation_salt(seed: u64, pixel: usize) -> u32 {
    (pixel_seed(seed, pixel, PERMUTATION_STREAM) >> 32) as u32
}

/// Kensler's cycle-walking permutation, extended to the full usize domain.
/// The input must be in 0..len when len > 1.
pub(crate) fn permute(mut index: usize, len: usize, salt: u32) -> usize {
    if len <= 1 {
        return 0;
    }
    debug_assert!(index < len);
    let mask = usize::MAX >> (len - 1).leading_zeros();
    let salt = salt as usize;
    // Odd multipliers and xor-shifts are invertible on the masked domain.
    // Each walk starts inside 0..len, so its cycle must return to that subset.
    loop {
        index ^= salt;
        index = index.wrapping_mul(0xe170_893d);
        index ^= salt >> 16;
        index ^= (index & mask) >> 4;
        index ^= salt >> 8;
        index = index.wrapping_mul(0x0929_eb3f);
        index ^= salt >> 23;
        index ^= (index & mask) >> 1;
        index = index.wrapping_mul(1 | salt >> 27);
        index = index.wrapping_mul(0x6935_fa69);
        index ^= (index & mask) >> 11;
        index = index.wrapping_mul(0x74dc_b303);
        index ^= (index & mask) >> 2;
        index = index.wrapping_mul(0x9e50_1cc3);
        index ^= (index & mask) >> 2;
        index = index.wrapping_mul(0xc860_a3df);
        index &= mask;
        index ^= index >> 5;
        if index < len {
            break;
        }
    }
    // Compute (index + salt) % len without overflowing even for len near usize::MAX.
    let offset = salt % len;
    if index >= len - offset {
        index - (len - offset)
    } else {
        index + offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampling::rng::RenderRng;
    use rand::{Rng, SeedableRng};
    use std::collections::HashSet;

    #[test]
    fn permutation_is_bijective_for_arbitrary_lengths() {
        for len in [1, 2, 3, 4, 7, 8, 15, 16, 17, 64, 100] {
            for salt in [0, 1, 42, 0x8ed5_3aa7, u32::MAX] {
                let mut indices: Vec<_> = (0..len).map(|i| permute(i, len, salt)).collect();
                indices.sort_unstable();
                assert!(indices.into_iter().eq(0..len), "{len}, {salt}");
            }
        }
        for len in [
            1 << (usize::BITS - 1),
            (1 << (usize::BITS - 1)) + 1,
            usize::MAX,
        ] {
            let mut indices = HashSet::new();
            for i in [0, 1, 17, len / 2, len - 2, len - 1] {
                let j = permute(i, len, u32::MAX);
                assert!(j < len);
                assert!(indices.insert(j));
            }
        }
    }

    #[test]
    fn subpixel_coordinates_stay_half_open_after_wraparound() {
        for shift_x in [0, 1, u64::MAX] {
            for shift_y in [0, 1, u64::MAX] {
                let sampler = SubpixelSampler { shift_x, shift_y };
                for index in [0, 1, 97, usize::MAX / 2, usize::MAX - 1, usize::MAX] {
                    assert!(sampler.offset(index).iter().all(|v| (0.0..1.0).contains(v)));
                }
            }
        }
    }

    #[test]
    fn shifted_r2_reduces_error_on_a_diagonal_step_edge() {
        const SPP: usize = 32;
        const PIXELS: usize = 256;
        // x + y < 0.75 covers a triangle of area 0.75² / 2.
        let edge = |x: f64, y: f64| u32::from(x + y < 0.75);
        let squared_error = |hits| (f64::from(hits) / SPP as f64 - 0.28125).powi(2);
        let mut r2_error = 0.0;
        let mut uniform_error = 0.0;
        for pixel in 0..PIXELS {
            let r2 = SubpixelSampler::new(42, pixel);
            let mut rng = RenderRng::seed_from_u64(pixel_seed(42, pixel, 0x42));
            let r2_hits = (0..SPP)
                .map(|k| {
                    let p = r2.offset(k);
                    edge(p.x, p.y)
                })
                .sum::<u32>();
            let uniform_hits = (0..SPP)
                .map(|_| edge(rng.random(), rng.random()))
                .sum::<u32>();
            r2_error += squared_error(r2_hits);
            uniform_error += squared_error(uniform_hits);
        }
        assert!(
            r2_error < uniform_error,
            "R2 {r2_error}, uniform {uniform_error}"
        );
    }
}

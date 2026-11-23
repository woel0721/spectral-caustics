//! 确定性渲染随机流与种子派生；不依赖 Rayon 调度。

use rand::{RngCore, SeedableRng};

// 环境 stream_id 与像素混合步长不同，避免相邻像素的相机/环境流使用同一种子。
pub(crate) const CAMERA_STREAM: u64 = 0;
pub(crate) const PREFIX_STREAM: u64 = 0x8ebc_6af0_9c88_c6e3;
const SPLITMIX_STEP: u64 = 0x9e37_79b9_7f4a_7c15;

pub(crate) const ENVIRONMENT_STREAM: u64 = 0xd1b5_4a32_d192_ed03;

// 漫反射续追（含玻璃）、次表面环境采样和 RR 使用独立流。
pub(crate) const DIFFUSE_STREAM: u64 = 0xa076_1d64_78bd_642f;
pub(crate) const MEDIUM_STREAM: u64 = 0x6a09_e667_f3bc_c909;

/// 为某个像素的某类采样派生固定种子，保证并行调度不改变结果。
///
/// 输入：基础 seed、行优先像素编号 index=y·width+x、随机流标识 stream_id。
///
/// 输出：经过 SplitMix64 风格位混合的 u64；同一组输入总返回同一种子，相机/环境/续追使用不同标识。
pub(crate) fn pixel_seed(base_seed: u64, pixel_index: usize, stream_id: u64) -> u64 {
    mix64(base_seed ^ (pixel_index as u64).wrapping_mul(SPLITMIX_STEP) ^ stream_id)
}

fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Xoshiro256++ with SplitMix64 seed expansion and portable little-endian byte output.
#[derive(Clone, Copy)]
pub(crate) struct RenderRng([u64; 4]);

impl SeedableRng for RenderRng {
    type Seed = [u8; 32];

    fn from_seed(seed: Self::Seed) -> Self {
        if seed == [0; 32] {
            return Self::seed_from_u64(0);
        }
        Self(std::array::from_fn(|i| {
            u64::from_le_bytes(seed[i * 8..][..8].try_into().expect("eight seed bytes"))
        }))
    }

    #[inline]
    fn seed_from_u64(mut seed: u64) -> Self {
        Self(std::array::from_fn(|_| {
            seed = seed.wrapping_add(SPLITMIX_STEP);
            mix64(seed)
        }))
    }
}

impl RngCore for RenderRng {
    #[inline]
    fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        let [s0, s1, s2, s3] = &mut self.0;
        let result = s0.wrapping_add(*s3).rotate_left(23).wrapping_add(*s0);
        let t = *s1 << 17;
        *s2 ^= *s0;
        *s3 ^= *s1;
        *s1 ^= *s2;
        *s0 ^= *s3;
        *s2 ^= t;
        *s3 = s3.rotate_left(45);
        result
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes()[..chunk.len()]);
        }
    }
}

/// Convert the high 53 bits to a uniform grid in [0, 1).
#[inline]
pub(crate) fn unit_f64_from_u64(v: u64) -> f64 {
    (v >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_rng_replays_a_portable_xoshiro_sequence() {
        let mut rng = RenderRng::from_seed(std::array::from_fn(|i| {
            if i % 8 == 0 { (i / 8 + 1) as u8 } else { 0 }
        }));
        let mut copy = rng;
        // Reference Xoshiro256++ outputs for the four words [1, 2, 3, 4].
        assert_eq!(rng.next_u64(), 0x0000_0000_0280_0001);
        assert_eq!(rng.next_u64(), 0x0000_0000_0380_0067);
        assert_eq!(copy.next_u32(), 0);
        assert_eq!(copy.next_u64(), 0x0000_0000_0380_0067);
        assert_eq!(copy.next_u64(), rng.next_u64());
    }

    #[test]
    fn seed_expansion_and_byte_filling_are_deterministic() {
        assert_eq!(size_of::<RenderRng>(), 32);
        assert_ne!(RenderRng::from_seed([0; 32]).0, [0; 4]);
        for seed in [0, 1, 42, u64::MAX] {
            let mut integers = RenderRng::seed_from_u64(seed);
            assert_ne!(integers.0, [0; 4]);
            let mut bytes = RenderRng::seed_from_u64(seed);
            bytes.fill_bytes(&mut []);
            let mut out = [0; 19];
            bytes.fill_bytes(&mut out);
            assert_eq!(out[..8], integers.next_u64().to_le_bytes());
            assert_eq!(out[8..16], integers.next_u64().to_le_bytes());
            assert_eq!(out[16..], integers.next_u64().to_le_bytes()[..3]);
            assert_eq!(bytes.next_u64(), integers.next_u64());
        }
    }

    #[test]
    fn unit_float_uses_the_upper_53_bits_with_an_open_right_edge() {
        assert_eq!(unit_f64_from_u64(0), 0.0);
        assert_eq!(unit_f64_from_u64(2047), 0.0);
        assert_eq!(unit_f64_from_u64(2048), 2.0_f64.powi(-53));
        assert_eq!(unit_f64_from_u64(u64::MAX), 1.0_f64.next_down());
    }
}

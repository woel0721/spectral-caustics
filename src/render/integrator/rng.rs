//! 与像素编号绑定的确定性流种子；不依赖 Rayon 调度。

// 环境 stream_id 与像素混合步长不同，避免相邻像素的相机/环境流使用同一种子。
pub(super) const CAMERA_STREAM: u64 = 0;

pub(super) const ENVIRONMENT_STREAM: u64 = 0xd1b5_4a32_d192_ed03;

// 漫反射续追、次表面环境采样和 RR 使用独立流。
pub(super) const DIFFUSE_STREAM: u64 = 0xa076_1d64_78bd_642f;

/// SplitMix64 风格混合；只依赖基础种子、行优先像素编号和随机流，与线程调度无关。
pub(super) fn pixel_seed(base_seed: u64, pixel_index: usize, stream_id: u64) -> u64 {
    let mut z = base_seed ^ (pixel_index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ stream_id;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

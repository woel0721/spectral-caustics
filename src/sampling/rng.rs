//! 与像素编号绑定的确定性流种子；不依赖 Rayon 调度。

// 环境 stream_id 与像素混合步长不同，避免相邻像素的相机/环境流使用同一种子。
pub(crate) const CAMERA_STREAM: u64 = 0;

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
    let mut z = base_seed ^ (pixel_index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ stream_id;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

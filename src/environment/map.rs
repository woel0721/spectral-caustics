//! Z-up 经纬度环境贴图；U 水平周期采样，V 在图片上下边界夹紧。

use super::EnvironmentSample;
use image::DynamicImage;
use image::ImageDecoder;
use image::codecs::hdr::HdrDecoder;
use nalgebra::Vector3;
use rand::Rng;
use rand::RngCore;
use std::error::Error;
use std::f64::consts::PI;
use std::f64::consts::TAU;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::Path;

/// 非黑贴图的采样分布：95% 亮度重要性 + 5% 均匀球面，确保黑像素也有支持。
const IMPORTANCE_FRACTION: f64 = 0.95;
pub(super) const UNIFORM_SPHERE_PDF: f64 = 1.0 / (4.0 * PI);

/// 按行保存相对线性 RGB 辐亮度，第一行为图片顶部（世界 +Z）。
/// 像素在加载时验证，之后只读；不进行曝光、色调映射或 sRGB 转换。
#[derive(Debug)]
pub struct EnvironmentMap {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 3]>,
    /// 行优先的重要性 CDF，含起点 0 和终点 1；全黑贴图为空。
    importance_cdf: Vec<f64>,
    /// 每行起点的累积概率（共 height + 1 项），与 importance_cdf 两级二分等价。
    row_cdf: Vec<f64>,
    /// 每行上下边界的 cos(theta)，共 height + 1 项，由 +1 递减到 -1。
    row_cos_theta: Vec<f64>,
    /// 每行单像素立体角的倒数 1/Ω_row，供 PDF 查询复用。
    row_inv_solid_angle: Vec<f64>,
}

impl EnvironmentMap {
    /// 读取一张 Radiance RGBE HDR，保留浮点亮度并预建采样分布。
    ///
    /// 输入：文件路径；输出：EnvironmentMap，文件、解码或像素校验失败时 Err。
    /// 只支持标准 -Y +X（从上到下、从左到右）排列，不经过 8 位或 sRGB 转换。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Self::decode(BufReader::new(File::open(path)?))
    }

    /// 沿贴图局部方向查询线性 RGB 辐亮度，供环境背景与直接光求值。
    ///
    /// 输入：有限非零 direction，内部归一化；输出：双线性插值的 HDR RGB，方向无效返回 None。
    /// +Z/-Z 对应图像顶部/底部；+X、+Y、-Y 对应 U=0.5、0.75、0.25，-X 为 U=0/1 接缝，极点取 U=0.5。
    pub fn radiance(&self, direction: Vector3<f64>) -> Option<Vector3<f64>> {
        let (u, v) = direction_to_uv(direction)?;
        Some(self.sample_uv(u, v))
    }

    /// 一次方向映射同时查询贴图辐亮度与采样密度，避免重复三角函数计算。
    pub(super) fn radiance_and_pdf(&self, direction: Vector3<f64>) -> Option<(Vector3<f64>, f64)> {
        let (u, v) = direction_to_uv(direction)?;
        Some((self.sample_uv(u, v), self.pdf_from_uv(u, v)))
    }

    /// 只抽取贴图局部单位方向（不重复查询 PDF），供外层统一做一次 radiance_and_pdf。
    pub(super) fn sample_direction_unit(&self, rng: &mut dyn RngCore) -> Vector3<f64> {
        if self.importance_cdf.is_empty() {
            return sample_uniform_sphere(rng).direction;
        }
        if rng.random::<f64>() < IMPORTANCE_FRACTION {
            let u = rng.random::<f64>();
            // 两级上界查找与在整张 importance_cdf 上二分逐位等价，跳过零概率行与像素。
            let y = self.row_cdf.partition_point(|&p| p <= u) - 1;
            let row_start = y * self.width;
            let row_slice = &self.importance_cdf[row_start..=row_start + self.width];
            let x = row_slice.partition_point(|&p| p <= u) - 1;
            let phi = ((x as f64 + rng.random::<f64>()) / self.width as f64 - 0.5) * TAU;
            let top = self.row_cos_theta[y];
            let bottom = self.row_cos_theta[y + 1];
            // 均匀采样 cos(theta)，而非 theta；dω = dφ d(cos(theta))。
            let cos_theta = top + (bottom - top) * rng.random::<f64>();
            spherical_direction(phi, cos_theta)
        } else {
            sample_uniform_sphere(rng).direction
        }
    }

    /// 优先朝环境亮处抽方向，减少 HDRI 直接光积分的噪声。
    ///
    /// 输入：可变 RNG；输出：贴图局部单位方向及完整混合 PDF（sr⁻¹），分布查询失败返回 None。
    /// 分布为 p=0.95·p_importance+0.05/(4π)，保证黑像素方向也有非零概率；全黑图退化为均匀球面。
    /// 像素按亮度乘立体角选取，像素内部均匀抽 φ 和 cosθ，因此对立体角均匀。
    pub fn sample_direction(&self, rng: &mut dyn RngCore) -> Option<EnvironmentSample> {
        if self.importance_cdf.is_empty() {
            return Some(sample_uniform_sphere(rng));
        }
        let direction = self.sample_direction_unit(rng);
        Some(EnvironmentSample {
            direction,
            // 统一处理恰好落在接缝、像素边界或极点上的浮点样本。
            pdf: self.pdf_direction(direction)?,
        })
    }

    fn pdf_from_uv(&self, u: f64, v: f64) -> f64 {
        if self.importance_cdf.is_empty() {
            return UNIFORM_SPHERE_PDF;
        }
        let x = ((u.rem_euclid(1.0) * self.width as f64) as usize).min(self.width - 1);
        let y = ((v * self.height as f64) as usize).min(self.height - 1);
        let index = y * self.width + x;
        // 使用实际 CDF 区间宽度，包含预计算舍入，保证查询与抽样的概率相同。
        let probability = self.importance_cdf[index + 1] - self.importance_cdf[index];
        IMPORTANCE_FRACTION * probability * self.row_inv_solid_angle[y]
            + (1.0 - IMPORTANCE_FRACTION) * UNIFORM_SPHERE_PDF
    }

    /// 查询 sample_direction 的完整方向密度，供环境 MIS 计算权重。
    ///
    /// 输入：贴图局部的有限非零方向；输出：p（sr⁻¹），非法方向返回 None。
    ///
    /// 公式：p=0.95·P_pixel/Ω_pixel+0.05/(4π)，全黑图为 1/(4π)。
    /// P_pixel 采用实际 CDF 区间宽度；U=1 绕回首列，极点取 U=0.5，与抽样约定一致。
    pub fn pdf_direction(&self, direction: Vector3<f64>) -> Option<f64> {
        let (u, v) = direction_to_uv(direction)?;
        Some(self.pdf_from_uv(u, v))
    }

    /// 从字节流解码 HDR 为浮点像素，再建立可查询的贴图。
    ///
    /// 输入：实现 Read 的 HDR 数据流；输出：EnvironmentMap，格式、尺寸或像素无效返回 Err。
    /// 解码前检查像素缓冲大小，解码后不做 sRGB 传递函数变换。
    fn decode(reader: impl Read) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let decoder = HdrDecoder::new(reader)?;
        let (width, height) = decoder.dimensions();
        let (width, height) = (usize::try_from(width)?, usize::try_from(height)?);
        // 在解码分配像素缓冲之前拒绝零尺寸和地址空间溢出。
        let count = pixel_count(width, height)?;
        // HdrDecoder 输出 Rgb32F，into_rgb32f 直接取出浮点缓冲，不施加传递函数。
        let raw = DynamicImage::from_decoder(decoder)?
            .into_rgb32f()
            .into_raw();
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(count)
            .map_err(|_| "环境贴图像素缓冲内存分配失败")?;
        pixels.extend_from_slice(raw.as_chunks::<3>().0);
        drop(raw);
        Ok(Self::from_pixels(width, height, pixels)?)
    }

    /// 把已有 HDR 像素构建为环境贴图，并缓存纬度边界和亮度采样分布。
    ///
    /// 输入：正宽高与行优先 `Vec<[f32;3]>`；输出：EnvironmentMap，长度不符、负/非法像素或分辨率失效返回 Err。
    fn from_pixels(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> Result<Self, String> {
        if pixels.len() != pixel_count(width, height)? {
            return Err("环境贴图像素数量与宽高不匹配".into());
        }
        if pixels
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err("环境贴图辐亮度必须有限且非负".into());
        }
        let row_cos_theta: Vec<_> = (0..=height)
            .map(|row| (PI * row as f64 / height as f64).cos())
            .collect();
        if row_cos_theta.windows(2).any(|edges| edges[0] <= edges[1]) {
            return Err("环境贴图纬度分辨率超出浮点精度".into());
        }
        let width_scale = TAU / width as f64;
        let row_inv_solid_angle: Vec<_> = row_cos_theta
            .windows(2)
            .map(|edges| 1.0 / (width_scale * (edges[0] - edges[1])))
            .collect();
        let mut map = Self {
            width,
            height,
            pixels,
            importance_cdf: Vec::new(),
            row_cdf: Vec::new(),
            row_cos_theta,
            row_inv_solid_angle,
        };
        map.build_distribution();
        Ok(map)
    }

    /// 求一行中单个经纬图像素在球面上覆盖的立体角，供采样分布和 PDF 换算。
    ///
    /// 输入：有效行下标 row；输出：Ω=(2π/width)·(cosθ_top-cosθ_bottom)，单位 sr。
    fn pixel_solid_angle(&self, row: usize) -> f64 {
        (TAU / self.width as f64) * (self.row_cos_theta[row] - self.row_cos_theta[row + 1])
    }

    /// 根据缓存像素建立按亮度采样的累积分布，写入 self.importance_cdf，无返回值。
    ///
    /// 输入：已校验的 self 像素和尺寸；公式：w_i=Y_i·Ω_i，CDF_j=Σ_{i<j}w_i/Σw_i，
    /// 其中 Y=0.2126R+0.7152G+0.0722B；全黑图保留空分布，由采样入口退化为均匀球面。
    fn build_distribution(&mut self) {
        let mut cdf = Vec::with_capacity(self.pixels.len() + 1);
        cdf.push(0.0);
        let mut total = 0.0;
        for (row, pixels) in self.pixels.chunks_exact(self.width).enumerate() {
            let solid_angle = self.pixel_solid_angle(row);
            for &[r, g, b] in pixels {
                let luminance =
                    0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
                total += luminance * solid_angle;
                cdf.push(total);
            }
        }
        // f32 输入在 f64 中加权，避免 HDR 亮度求和溢出；全黑不做 0/0。
        if total > 0.0 {
            for value in &mut cdf {
                *value /= total;
            }
            let row_cdf = (0..=self.height).map(|row| cdf[row * self.width]).collect();
            self.importance_cdf = cdf;
            self.row_cdf = row_cdf;
        }
    }

    /// 在缓存 HDR 像素中做双线性插值，供方向查询取得辐亮度。
    ///
    /// 输入：有限 UV，顶部 V=0；输出：线性 RGB，不裁掉大于 1 的亮度。
    ///
    /// 公式：C=(1-dx)(1-dy)C00+dx(1-dy)C10+(1-dx)dy C01+dx dy C11。
    /// U 周期绕回、V 夹到上下边缘；连续像素坐标的整数位置表示像素中心。
    fn sample_uv(&self, u: f64, v: f64) -> Vector3<f64> {
        let x = u.rem_euclid(1.0) * self.width as f64 - 0.5;
        let y = (v * self.height as f64 - 0.5).clamp(0.0, (self.height - 1) as f64);
        let x_floor = x.floor();
        // x_floor 最小为 -1；先向下取整再做整数 wrap，保留接缝两侧的插值权重。
        let x0 = (x_floor as isize).rem_euclid(self.width as isize) as usize;
        let x1 = (x0 + 1) % self.width;
        let y0 = y.floor() as usize;
        let y1 = (y0 + 1).min(self.height - 1);
        let dx = x - x_floor;
        let dy = y - y0 as f64;

        let mut rgb = Vector3::zeros();
        for (px, py, weight) in [
            (x0, y0, (1.0 - dx) * (1.0 - dy)),
            (x1, y0, dx * (1.0 - dy)),
            (x0, y1, (1.0 - dx) * dy),
            (x1, y1, dx * dy),
        ] {
            // 在 f64 中插值，保留大于 1 的亮度，也避免 f32 加法溢出。
            rgb += Vector3::from(self.pixels[py * self.width + px]).cast::<f64>() * weight;
        }
        rgb
    }
}

/// 校验环境图像尺寸及地址空间，供解码前安全检查。
///
/// 输入：width/height；输出：width·height，零尺寸、乘法或缓冲字节数溢出返回 Err。
fn pixel_count(width: usize, height: usize) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("环境贴图宽高必须大于零".into());
    }
    let count = width.checked_mul(height).ok_or("环境贴图像素数量溢出")?;
    count
        .checked_mul(size_of::<[f32; 3]>())
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or("环境贴图像素缓冲大小溢出")?;
    Ok(count)
}

/// 把有限非零方向稳定归一化，供 HDR 方向和 UV 查询使用。
///
/// 输入：direction；输出：单位向量，无效时 None；先除最大绝对分量，再除模长，避免平方和上溢/下溢。
pub(super) fn normalized_direction(direction: Vector3<f64>) -> Option<Vector3<f64>> {
    if !direction.iter().all(|value| value.is_finite()) {
        return None;
    }
    let scale = direction.amax();
    if scale == 0.0 {
        return None;
    }
    // 先缩放，避免有限非零向量的 norm 在平方求和时上溢或下溢。
    let direction = direction / scale;
    Some(direction / direction.norm())
}

/// 把球面角转换成 Z-up 单位方向，供环境采样使用。
///
/// 输入：方位角 φ（弧度）和 cosθ∈[-1,1]；输出：(sinθ cosφ,sinθ sinφ,cosθ)，sinθ=√(1-cos²θ)。
fn spherical_direction(phi: f64, cos_theta: f64) -> Vector3<f64> {
    let sin_theta = ((1.0 - cos_theta) * (1.0 + cos_theta)).max(0.0).sqrt();
    Vector3::new(sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta)
}

/// 在整个球面上均匀抽取方向，供黑色 HDR 和渐变环境回退使用。
///
/// 输入：可变 RNG；输出：单位方向及 p=1/(4π)。
///
/// 公式：φ=2πu-π，cosθ=1-2v，u/v 为独立 [0,1) 样本。
pub(super) fn sample_uniform_sphere(rng: &mut dyn RngCore) -> EnvironmentSample {
    EnvironmentSample {
        direction: spherical_direction(
            TAU * rng.random::<f64>() - PI,
            1.0 - 2.0 * rng.random::<f64>(),
        ),
        pdf: UNIFORM_SPHERE_PDF,
    }
}

/// 把贴图局部方向映射成经纬 UV，供 HDR 颜色和 PDF 查询共用。
///
/// 输入：有限非零方向，内部归一化；输出：(U,V)，无效方向返回 None。
///
/// 公式：φ=atan2(d_y,d_x)，θ=acos(d_z)，U=φ/(2π)+1/2，V=θ/π；极点统一 U=0.5。
fn direction_to_uv(direction: Vector3<f64>) -> Option<(f64, f64)> {
    let direction = normalized_direction(direction)?;
    // 极点没有唯一经度，固定 U=0.5，避免 x/y 的有符号零改变查询列。
    let phi = if direction.x == 0.0 && direction.y == 0.0 {
        0.0
    } else {
        direction.y.atan2(direction.x)
    };
    let theta = direction.z.clamp(-1.0, 1.0).acos();
    Some((phi / TAU + 0.5, theta / PI))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;
    use image::codecs::hdr::HdrEncoder;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use std::fs;
    use std::io::Cursor;
    use std::io::Write;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    const PIXELS: [[f32; 3]; 4] = [
        [2.0, 4.0, 8.0],
        [6.0, 8.0, 12.0],
        [10.0, 12.0, 16.0],
        [14.0, 16.0, 20.0],
    ];

    fn test_map() -> EnvironmentMap {
        EnvironmentMap::from_pixels(2, 2, PIXELS.to_vec()).unwrap()
    }

    fn assert_rgb(actual: Vector3<f64>, expected: [f64; 3]) {
        assert!(
            (actual - Vector3::from(expected)).norm() < 1e-12,
            "actual={actual:?}, expected={expected:?}"
        );
    }

    fn direction_at(u: f64, v: f64) -> Vector3<f64> {
        let phi = (u - 0.5) * TAU;
        let theta = v * PI;
        Vector3::new(
            theta.sin() * phi.cos(),
            theta.sin() * phi.sin(),
            theta.cos(),
        )
    }

    // 手写 RGBE 字节，独立检查通道、行列顺序和线性 HDR 数值；不依赖编码器往返。
    fn synthetic_hdr() -> Vec<u8> {
        let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 2\n".to_vec();
        for exponent in 130..=133 {
            bytes.extend_from_slice(&[128, 64, 32, exponent]);
        }
        bytes
    }

    #[test]
    fn direction_uv_matches_z_up_axes_and_image_orientation() {
        for (direction, expected) in [
            (Vector3::z(), (0.5, 0.0)),
            (-Vector3::z(), (0.5, 1.0)),
            (Vector3::x(), (0.5, 0.5)),
            (Vector3::y(), (0.75, 0.5)),
            (-Vector3::y(), (0.25, 0.5)),
            (Vector3::new(-1.0, 0.0, 0.0), (1.0, 0.5)),
            (Vector3::new(-1.0, -0.0, 0.0), (0.0, 0.5)),
        ] {
            let (u, v) = direction_to_uv(direction).unwrap();
            assert!((u - expected.0).abs() < 1e-12);
            assert!((v - expected.1).abs() < 1e-12);
        }
    }

    #[test]
    fn direction_is_normalized_even_at_extreme_finite_scales() {
        let map = test_map();
        let direction = Vector3::new(1.0, -2.0, 3.0);
        let expected_uv = direction_to_uv(direction).unwrap();
        let expected_rgb = map.radiance(direction).unwrap();
        for scale in [0.25, 8.0, 1e300, 1e-300] {
            let (u, v) = direction_to_uv(direction * scale).unwrap();
            assert!((u - expected_uv.0).abs() < 1e-12);
            assert!((v - expected_uv.1).abs() < 1e-12);
            assert!((map.radiance(direction * scale).unwrap() - expected_rgb).norm() < 1e-12);
            assert_eq!(
                map.pdf_direction(direction * scale),
                map.pdf_direction(direction)
            );
        }
        for scale in [f64::MAX, f64::from_bits(1)] {
            assert_eq!(direction_to_uv(Vector3::z() * scale), Some((0.5, 0.0)));
        }
    }

    #[test]
    fn zero_and_nonfinite_directions_return_none() {
        let map = test_map();
        assert!(direction_to_uv(Vector3::zeros()).is_none());
        assert!(map.radiance(Vector3::zeros()).is_none());
        assert!(map.pdf_direction(Vector3::zeros()).is_none());
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for axis in 0..3 {
                let mut direction = Vector3::repeat(1.0);
                direction[axis] = value;
                assert!(direction_to_uv(direction).is_none());
                assert!(map.radiance(direction).is_none());
                assert!(map.pdf_direction(direction).is_none());
            }
        }
    }

    #[test]
    fn pixel_centers_recover_the_four_texels() {
        let map = test_map();
        for (u, v, pixel) in [
            (0.25, 0.25, PIXELS[0]),
            (0.75, 0.25, PIXELS[1]),
            (0.25, 0.75, PIXELS[2]),
            (0.75, 0.75, PIXELS[3]),
        ] {
            assert_rgb(
                map.radiance(direction_at(u, v)).unwrap(),
                pixel.map(f64::from),
            );
        }
    }

    #[test]
    fn bilinear_interpolation_matches_known_two_by_two_values() {
        let map = test_map();
        assert_rgb(map.radiance(Vector3::x()).unwrap(), [8.0, 10.0, 14.0]);
        // x=0.25, y=0.75，四个权重依次为 3/16、1/16、9/16、3/16。
        assert_rgb(
            map.radiance(direction_at(0.375, 0.625)).unwrap(),
            [9.0, 11.0, 15.0],
        );
    }

    #[test]
    fn horizontal_seam_wraps_last_and_first_columns_continuously() {
        let map = EnvironmentMap::from_pixels(
            4,
            1,
            vec![[2.0, 4.0, 8.0], [32.0; 3], [64.0; 3], [10.0, 12.0, 16.0]],
        )
        .unwrap();
        for u in [-1.0, 0.0, 1.0, 2.0] {
            assert_rgb(map.sample_uv(u, 0.5), [6.0, 8.0, 12.0]);
        }
        for u in [-0.01, 0.01, 0.99] {
            assert!((map.sample_uv(u, 0.5) - map.sample_uv(u + 1.0, 0.5)).norm() < 1e-12);
        }
        let seam = map.radiance(Vector3::new(-1.0, 0.0, 0.0)).unwrap();
        assert_rgb(seam, [6.0, 8.0, 12.0]);
        assert_eq!(seam, map.radiance(Vector3::new(-1.0, -0.0, 0.0)).unwrap());
        for y in [-1e-9, 1e-9] {
            let neighbor = map.radiance(Vector3::new(-1.0, y, 0.0)).unwrap();
            assert!((neighbor - seam).norm() < 1e-7);
        }
    }

    #[test]
    fn vertical_boundaries_clamp_without_wrapping_to_the_other_pole() {
        let map = test_map();
        for v in [-1.0, 0.0, 0.1, 0.25] {
            assert_rgb(map.sample_uv(0.25, v), [2.0, 4.0, 8.0]);
        }
        for v in [0.75, 0.9, 1.0, 2.0] {
            assert_rgb(map.sample_uv(0.25, v), [10.0, 12.0, 16.0]);
        }
        assert_rgb(map.radiance(Vector3::z()).unwrap(), [4.0, 6.0, 10.0]);
        assert_rgb(map.radiance(-Vector3::z()).unwrap(), [12.0, 14.0, 18.0]);
    }

    #[test]
    fn single_column_interpolates_only_vertically() {
        let map = EnvironmentMap::from_pixels(1, 2, vec![PIXELS[0], PIXELS[2]]).unwrap();
        for u in [-1.0, 0.0, 0.25, 0.5, 1.0, 2.0] {
            assert_rgb(map.sample_uv(u, 0.0), [2.0, 4.0, 8.0]);
            assert_rgb(map.sample_uv(u, 0.5), [6.0, 8.0, 12.0]);
            assert_rgb(map.sample_uv(u, 1.0), [10.0, 12.0, 16.0]);
        }
    }

    #[test]
    fn single_row_interpolates_only_horizontally() {
        let map = EnvironmentMap::from_pixels(2, 1, vec![PIXELS[0], PIXELS[1]]).unwrap();
        for v in [-1.0, 0.0, 0.25, 0.5, 1.0, 2.0] {
            assert_rgb(map.sample_uv(0.25, v), [2.0, 4.0, 8.0]);
            assert_rgb(map.sample_uv(0.5, v), [4.0, 6.0, 10.0]);
            assert_rgb(map.sample_uv(0.75, v), [6.0, 8.0, 12.0]);
        }
    }

    #[test]
    fn single_pixel_is_constant_and_preserves_linear_hdr_values() {
        let map = EnvironmentMap::from_pixels(1, 1, vec![[0.25, 2.0, 16.0]]).unwrap();
        for direction in [
            Vector3::x(),
            -Vector3::x(),
            Vector3::y(),
            -Vector3::y(),
            Vector3::z(),
            -Vector3::z(),
            Vector3::new(1.0, 2.0, 3.0),
        ] {
            assert_rgb(map.radiance(direction).unwrap(), [0.25, 2.0, 16.0]);
        }
    }

    #[test]
    fn interpolation_of_maximum_f32_radiance_stays_finite_and_unclipped() {
        let map = EnvironmentMap::from_pixels(2, 2, vec![[f32::MAX; 3]; 4]).unwrap();
        for direction in [Vector3::x(), Vector3::new(1.0, 2.0, 3.0)] {
            let rgb = map.radiance(direction).unwrap();
            for value in rgb.iter() {
                assert!(value.is_finite());
                assert!((value / f64::from(f32::MAX) - 1.0).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn invalid_pixels_are_rejected_in_every_channel() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01] {
            for channel in 0..3 {
                let mut pixel = [1.0; 3];
                pixel[channel] = value;
                assert!(EnvironmentMap::from_pixels(2, 1, vec![[1.0; 3], pixel]).is_err());
            }
        }
        assert!(EnvironmentMap::from_pixels(1, 1, vec![[0.0, 2.0, f32::MAX]]).is_ok());
    }

    #[test]
    fn invalid_dimensions_and_pixel_counts_are_rejected() {
        for (width, height) in [
            (0, 0),
            (0, 1),
            (1, 0),
            (usize::MAX, 2),
            (isize::MAX as usize / size_of::<[f32; 3]>() + 1, 1),
        ] {
            assert!(EnvironmentMap::from_pixels(width, height, vec![]).is_err());
        }
        assert!(EnvironmentMap::from_pixels(2, 2, vec![[1.0; 3]; 3]).is_err());
        assert!(EnvironmentMap::from_pixels(2, 2, vec![[1.0; 3]; 5]).is_err());
    }

    #[test]
    fn hdr_decoder_preserves_linear_values_and_top_down_pixel_order() {
        let map = EnvironmentMap::decode(Cursor::new(synthetic_hdr())).unwrap();
        assert_eq!((map.width, map.height), (2, 2));
        assert_eq!(
            map.pixels,
            [
                [2.0, 1.0, 0.5],
                [4.0, 2.0, 1.0],
                [8.0, 4.0, 2.0],
                [16.0, 8.0, 4.0]
            ]
        );
        assert_rgb(map.radiance(Vector3::z()).unwrap(), [3.0, 1.5, 0.75]);
        assert_rgb(map.radiance(-Vector3::z()).unwrap(), [12.0, 6.0, 3.0]);
    }

    #[test]
    fn rle_hdr_decodes_from_memory_without_clipping_or_gamma_conversion() {
        let pixels: Vec<_> = [[0.25, 2.0, 16.0], [4.0, 8.0, 32.0]]
            .into_iter()
            .flat_map(|pixel| [Rgb(pixel); 8])
            .collect();
        let mut bytes = Vec::new();
        HdrEncoder::new(&mut bytes).encode(&pixels, 8, 2).unwrap();
        let map = EnvironmentMap::decode(Cursor::new(bytes)).unwrap();
        assert_eq!(
            map.pixels,
            pixels.iter().map(|pixel| pixel.0).collect::<Vec<_>>()
        );
        assert_rgb(map.radiance(Vector3::z()).unwrap(), [0.25, 2.0, 16.0]);
        assert_rgb(map.radiance(-Vector3::z()).unwrap(), [4.0, 8.0, 32.0]);
    }

    #[test]
    fn malformed_truncated_and_unsupported_hdr_inputs_return_errors() {
        assert!(EnvironmentMap::decode(Cursor::new(b"not a Radiance HDR image")).is_err());
        let mut truncated = synthetic_hdr();
        truncated.pop();
        assert!(EnvironmentMap::decode(Cursor::new(truncated)).is_err());
        for dimensions in [
            "-Y 0 +X 1",
            "-Y 1 +X 0",
            "-Y 4294967295 +X 4294967295",
            "+Y 2 +X 2",
            "-Y 2 -X 2",
        ] {
            let bytes = format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n{dimensions}\n");
            assert!(EnvironmentMap::decode(Cursor::new(bytes)).is_err());
        }
    }

    #[test]
    fn load_caches_pixels_and_does_not_read_the_file_during_queries() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "environment-map-{}-{stamp}.hdr",
            std::process::id()
        ));
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(&synthetic_hdr()).unwrap();
        drop(file);
        let result = EnvironmentMap::load(&path);
        fs::remove_file(&path).unwrap();
        let map = result.unwrap();
        assert!(EnvironmentMap::load(&path).is_err());
        assert_rgb(map.radiance(Vector3::z()).unwrap(), [3.0, 1.5, 0.75]);
        assert_rgb(map.radiance(Vector3::x()).unwrap(), [7.5, 3.75, 1.875]);
        let sample = map
            .sample_direction(&mut StdRng::seed_from_u64(42))
            .unwrap();
        assert_eq!(map.pdf_direction(sample.direction), Some(sample.pdf));
    }

    fn pixel_index(map: &EnvironmentMap, direction: Vector3<f64>) -> usize {
        let (u, v) = direction_to_uv(direction).unwrap();
        let x = ((u.rem_euclid(1.0) * map.width as f64) as usize).min(map.width - 1);
        let y = ((v * map.height as f64) as usize).min(map.height - 1);
        y * map.width + x
    }

    #[test]
    fn cdf_weights_use_linear_luminance_and_exact_solid_angle() {
        let pixels: Vec<_> = (0..12)
            .map(|i| match i % 3 {
                0 => [1.0, 0.0, 0.0],
                1 => [0.0, 1.0, 0.0],
                _ => [0.0, 0.0, 2.0],
            })
            .collect();
        let map = EnvironmentMap::from_pixels(3, 4, pixels.clone()).unwrap();
        let weights: Vec<_> = pixels
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let row = (i / 3) as f64;
                let area = (TAU / 3.0) * ((row * PI / 4.0).cos() - ((row + 1.0) * PI / 4.0).cos());
                (0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
                    * area
            })
            .collect();
        let total: f64 = weights.iter().sum();
        assert_eq!(map.importance_cdf.first(), Some(&0.0));
        assert_eq!(map.importance_cdf.last(), Some(&1.0));
        for (i, weight) in weights.iter().enumerate() {
            let probability = map.importance_cdf[i + 1] - map.importance_cdf[i];
            assert!((probability - weight / total).abs() < 1e-14);
        }
        let area: f64 = (0..map.height)
            .map(|row| map.pixel_solid_angle(row) * map.width as f64)
            .sum();
        assert!((area - 4.0 * PI).abs() < 1e-14);
    }

    #[test]
    fn samples_are_unit_positive_match_pdf_and_reproduce_with_seed() {
        for (width, height) in [(1, 1), (1, 7), (7, 1), (7, 3), (3, 7)] {
            let pixels = (0..width * height).map(|i| [i as f32; 3]).collect();
            let map = EnvironmentMap::from_pixels(width, height, pixels).unwrap();
            let mut rng = StdRng::seed_from_u64(42);
            let mut replay = StdRng::seed_from_u64(42);
            let mut different = StdRng::seed_from_u64(43);
            for _ in 0..2048 {
                let sample = map.sample_direction(&mut rng).unwrap();
                assert!((sample.direction.norm() - 1.0).abs() < 1e-14);
                assert!(sample.pdf.is_finite() && sample.pdf > 0.0);
                assert_eq!(Some(sample.pdf), map.pdf_direction(sample.direction));
                assert_eq!(Some(sample), map.sample_direction(&mut replay));
                assert_ne!(
                    sample.direction,
                    map.sample_direction(&mut different).unwrap().direction
                );
            }
        }
    }

    #[test]
    fn directional_pdf_integrates_to_one_on_the_sphere() {
        let pixels = (0..35)
            .map(|i| [if i % 3 == 0 { 0.0 } else { (i + 1) as f32 }; 3])
            .collect();
        let map = EnvironmentMap::from_pixels(7, 5, pixels).unwrap();
        // 独立按经纬度像素的精确面积积分，查询点在像素中心。
        let mut integral = 0.0;
        for row in 0..5 {
            let top = (row as f64 * PI / 5.0).cos();
            let bottom = ((row + 1) as f64 * PI / 5.0).cos();
            for col in 0..7 {
                let direction = direction_at((col as f64 + 0.5) / 7.0, (row as f64 + 0.5) / 5.0);
                integral += map.pdf_direction(direction).unwrap() * (TAU / 7.0) * (top - bottom);
            }
        }
        assert!((integral - 1.0).abs() < 1e-12);
        // 再用与贴图网格无关的等立体角球面网格，检验方向到像素的分区。
        let mut quadrature = 0.0;
        for row in 0..256 {
            let z = 1.0 - 2.0 * (f64::from(row) + 0.5) / 256.0;
            for col in 0..512 {
                let phi = TAU * (f64::from(col) + 0.5) / 512.0 - PI;
                quadrature += map.pdf_direction(spherical_direction(phi, z)).unwrap();
            }
        }
        assert!((quadrature * (4.0 * PI / (256.0 * 512.0)) - 1.0).abs() < 0.01);
    }

    #[test]
    fn constant_luminance_is_uniform_in_solid_angle_including_polar_rows() {
        let map = EnvironmentMap::from_pixels(3, 4, vec![[4.0; 3]; 12]).unwrap();
        let mut counts = [0; 12];
        let mut mean = Vector3::zeros();
        let mut mean_z_squared = 0.0;
        let mut rng = StdRng::seed_from_u64(42);
        let count = 32768;
        for _ in 0..count {
            let sample = map.sample_direction(&mut rng).unwrap();
            assert!((sample.pdf - UNIFORM_SPHERE_PDF).abs() < 1e-14);
            counts[pixel_index(&map, sample.direction)] += 1;
            mean += sample.direction;
            mean_z_squared += sample.direction.z.powi(2);
        }
        for (i, observed) in counts.into_iter().enumerate() {
            let expected_probability = map.pixel_solid_angle(i / 3) / (4.0 * PI);
            assert!((f64::from(observed) / f64::from(count) - expected_probability).abs() < 0.008);
        }
        assert!((mean / f64::from(count)).norm() < 0.02);
        assert!((mean_z_squared / f64::from(count) - 1.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn bright_regions_are_preferred_and_black_regions_retain_uniform_support() {
        let mut pixels = vec![[1.0; 3]; 12];
        pixels[0] = [0.0; 3];
        pixels[5] = [100.0; 3];
        let map = EnvironmentMap::from_pixels(4, 3, pixels).unwrap();
        let mut counts = [0; 12];
        let mut rng = StdRng::seed_from_u64(7);
        let count = 65536;
        for _ in 0..count {
            let sample = map.sample_direction(&mut rng).unwrap();
            counts[pixel_index(&map, sample.direction)] += 1;
        }
        // 此图三行每像素的面积分别为 π/4、π/2、π/4，总亮度积分 4π-π/4+99π/2。
        let total = 4.0 * PI - PI / 4.0 + 99.0 * PI / 2.0;
        for (i, observed) in counts.iter().enumerate() {
            let row = i / 4;
            let area = if row == 1 { PI / 2.0 } else { PI / 4.0 };
            let luminance = if i == 5 {
                100.0
            } else if i == 0 {
                0.0
            } else {
                1.0
            };
            let p = 0.95 * luminance * area / total + 0.05 * area / (4.0 * PI);
            let tolerance = 6.0 * (f64::from(count) * p * (1.0 - p)).sqrt() + 3.0;
            assert!((f64::from(*observed) - f64::from(count) * p).abs() < tolerance);
        }
        assert!(counts[5] > count * 4 / 5);
        assert!(counts[0] > 0);
        let black = direction_at(0.125, 1.0 / 6.0);
        let pdf = map.pdf_direction(black).unwrap();
        assert!((pdf - 0.05 / (4.0 * PI)).abs() < 1e-16);
        // 黑像素中心附近的双线性辐亮度可能非零，保底也覆盖这些方向。
        assert!(map.radiance(direction_at(0.2, 0.2)).unwrap().x > 0.0);
    }

    #[test]
    fn all_black_maps_fall_back_to_uniform_sphere() {
        let map = EnvironmentMap::from_pixels(5, 3, vec![[0.0; 3]; 15]).unwrap();
        assert!(map.importance_cdf.is_empty());
        let mut rng = StdRng::seed_from_u64(42);
        let mut uniform_rng = StdRng::seed_from_u64(42);
        let mut mean = Vector3::zeros();
        for _ in 0..8192 {
            let sample = map.sample_direction(&mut rng).unwrap();
            assert_eq!(sample, sample_uniform_sphere(&mut uniform_rng));
            assert_eq!(
                map.pdf_direction(sample.direction),
                Some(UNIFORM_SPHERE_PDF)
            );
            mean += sample.direction;
        }
        assert!((mean / 8192.0).norm() < 0.03);
        for direction in [Vector3::z(), -Vector3::z(), -Vector3::x()] {
            assert_eq!(map.pdf_direction(direction), Some(UNIFORM_SPHERE_PDF));
        }
    }

    #[test]
    fn pdf_seam_poles_and_boundary_random_values_are_finite_and_consistent() {
        let map =
            EnvironmentMap::from_pixels(4, 3, (0..12).map(|i| [i as f32; 3]).collect()).unwrap();
        assert_eq!(
            map.pdf_direction(Vector3::new(-1.0, 0.0, 0.0)),
            map.pdf_direction(Vector3::new(-1.0, -0.0, 0.0))
        );
        for (y, u) in [(1e-9, 0.999), (-1e-9, 0.001)] {
            assert_eq!(
                map.pdf_direction(Vector3::new(-1.0, y, 0.0)),
                map.pdf_direction(direction_at(u, 0.5))
            );
        }
        for (z, v) in [(1.0, 0.01), (-1.0, 0.99)] {
            let expected = map.pdf_direction(direction_at(0.625, v));
            for x in [0.0, -0.0] {
                for y in [0.0, -0.0] {
                    assert_eq!(map.pdf_direction(Vector3::new(x, y, z)), expected);
                }
            }
        }
        struct ScriptedRng(std::collections::VecDeque<u64>);
        impl RngCore for ScriptedRng {
            fn next_u32(&mut self) -> u32 {
                self.next_u64() as u32
            }
            fn next_u64(&mut self) -> u64 {
                self.0.pop_front().unwrap_or(0)
            }
            fn fill_bytes(&mut self, dest: &mut [u8]) {
                for chunk in dest.chunks_mut(8) {
                    chunk.copy_from_slice(&self.next_u64().to_le_bytes()[..chunk.len()]);
                }
            }
        }
        // 分别覆盖 CDF 的首/末非零区间、极点、接缝和均匀分支。
        for values in [
            [0; 4],
            [0, u64::MAX, u64::MAX, u64::MAX],
            [u64::MAX, 0, 0, 0],
        ] {
            let sample = map
                .sample_direction(&mut ScriptedRng(values.into()))
                .unwrap();
            assert!((sample.direction.norm() - 1.0).abs() < 1e-14);
            assert!(sample.pdf.is_finite() && sample.pdf > 0.0);
            assert_eq!(map.pdf_direction(sample.direction), Some(sample.pdf));
        }
    }

    #[test]
    fn hdr_dynamic_range_keeps_finite_pdf_and_full_support() {
        let map = EnvironmentMap::from_pixels(
            3,
            2,
            vec![
                [f32::MAX; 3],
                [0.0; 3],
                [f32::from_bits(1); 3],
                [1.0; 3],
                [f32::MAX; 3],
                [0.0; 3],
            ],
        )
        .unwrap();
        let mut rng = StdRng::seed_from_u64(9);
        for _ in 0..1024 {
            let sample = map.sample_direction(&mut rng).unwrap();
            assert!(sample.pdf.is_finite() && sample.pdf > 0.0);
        }
        for col in 0..3 {
            let direction = direction_at((f64::from(col) + 0.5) / 3.0, 0.25);
            for scale in [f64::MAX / 2.0, 1e-300] {
                assert_eq!(
                    map.pdf_direction(direction * scale),
                    map.pdf_direction(direction)
                );
            }
        }
    }

    #[test]
    fn pdf_compensation_recovers_the_original_radiance_integral() {
        // 单行图只沿经度线性插值，其球面平均亮度解析值为 (1+8+1+8)/4。
        let map = EnvironmentMap::from_pixels(4, 1, vec![[1.0; 3], [8.0; 3], [1.0; 3], [8.0; 3]])
            .unwrap();
        let mut rng = StdRng::seed_from_u64(123);
        let mut weighted = 0.0;
        let mut unweighted = 0.0;
        let count = 65536;
        for _ in 0..count {
            let sample = map.sample_direction(&mut rng).unwrap();
            let radiance = map.radiance(sample.direction).unwrap().x;
            weighted += radiance / sample.pdf;
            unweighted += radiance;
        }
        let expected = 4.5 * 4.0 * PI;
        assert!((weighted / f64::from(count) / expected - 1.0).abs() < 0.02);
        assert!(unweighted / f64::from(count) > 4.5 * 1.2);
    }

    #[test]
    fn two_level_row_cdf_lookup_matches_flat_cdf_including_zero_rows() {
        let mut pixels = vec![[0.0; 3]; 5 * 6];
        // Rows 0, 2, 5 completely black; rows 1, 3, 4 sparse bright pixels.
        pixels[5 + 2] = [3.0, 2.0, 1.0];
        pixels[3 * 5] = [10.0, 0.0, 0.0];
        pixels[3 * 5 + 4] = [0.0, 5.0, 0.0];
        pixels[4 * 5 + 1] = [1.0, 1.0, 8.0];
        let map = EnvironmentMap::from_pixels(5, 6, pixels).unwrap();
        let mut candidates = vec![0.0, 1.0_f64.next_down()];
        for &c in &map.importance_cdf {
            if c < 1.0 {
                candidates.push(c);
                candidates.push(c.next_down().max(0.0));
                if c.next_up() < 1.0 {
                    candidates.push(c.next_up());
                }
            }
        }
        let mut rng = StdRng::seed_from_u64(99);
        for _ in 0..256 {
            candidates.push(rng.random::<f64>());
        }
        for u in candidates {
            let flat = map.importance_cdf.partition_point(|&p| p <= u) - 1;
            let y = map.row_cdf.partition_point(|&p| p <= u) - 1;
            let row_start = y * map.width;
            let x = map.importance_cdf[row_start..=row_start + map.width]
                .partition_point(|&p| p <= u)
                - 1;
            assert_eq!(y * map.width + x, flat, "u={u}");
        }
    }
}

//! 真实相机续追的观测记录；不采样、不计光，也不改变路径状态。
//! AOV 是直接量化的统计 RGB，不能经过曝光、XYZ 或 display sRGB 转换。

use crate::{
    geometry::hit::Hit,
    material::{Material, ScatterType, dielectric::fresnel_dielectric, unit_vector},
};
use image::{ImageFormat, Rgb, RgbImage};
use nalgebra::Vector3;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

/// 正常路径退出的原因。数值/配置错误仍通过原 Result 返回，不生成不完整的 AOV。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathTermination {
    EnvironmentEscape,
    BackgroundEscape,
    MissingMaterial,
    NoScatteringComponents,
    Backface,
    SampleRejected,
    RussianRoulette,
    NonDeltaBudgetExhausted,
    DeltaBudgetExhausted,
    AllBudgetsExhausted,
    ZeroThroughput,
}

/// 已接受的路径续追事件数；不包含局部环境 NEE 的 BSDF 方向样本。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PathEventCounts {
    pub specular_reflection: u64,
    pub specular_transmission: u64,
    pub glossy_reflection: u64,
    pub diffuse_reflection: u64,
}

impl PathEventCounts {
    fn record(&mut self, event: ScatterType) {
        let count = match event {
            ScatterType::SpecularReflection => &mut self.specular_reflection,
            ScatterType::SpecularTransmission => &mut self.specular_transmission,
            ScatterType::GlossyReflection => &mut self.glossy_reflection,
            ScatterType::DiffuseReflection => &mut self.diffuse_reflection,
        };
        // 单条路径最多 32 个 Delta 和 1024 个连续连接。
        *count += 1;
    }

    fn accumulate(&mut self, other: Self) -> Result<(), &'static str> {
        add(&mut self.specular_reflection, other.specular_reflection)?;
        add(&mut self.specular_transmission, other.specular_transmission)?;
        add(&mut self.glossy_reflection, other.glossy_reflection)?;
        add(&mut self.diffuse_reflection, other.diffuse_reflection)
    }
}

/// 只在 trace_camera_path_impl 的一次真实调用期间存在，不保存每条完整路径。
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct PathDebugSample {
    pub(super) first_hit_object_id: Option<usize>,
    pub(super) first_hit_normal: Option<[f64; 3]>,
    pub(super) events: PathEventCounts,
    pub(super) reached_ground: bool,
    /// 几何上逃逸到已配置的环境；不代表该 miss 一定累加了环境辐亮度。
    pub(super) escaped_environment: bool,
    pub(super) total_internal_reflection: bool,
    pub(super) termination: Option<PathTermination>,
}

impl PathDebugSample {
    pub(super) fn hit(&mut self, hit: &Hit, ground_id: Option<usize>) {
        if self.first_hit_normal.is_none() {
            self.first_hit_object_id = hit.object_id;
            self.first_hit_normal = Some(hit.normal.into());
        }
        self.reached_ground |= ground_id.is_some() && hit.object_id == ground_id;
    }

    pub(super) fn scatter(
        &mut self,
        event: ScatterType,
        material: &dyn Material,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
    ) {
        self.events.record(event);
        self.total_internal_reflection |= internal_tir(event, material, wo, normal);
    }
}

/// 反射事件本身不能证明 TIR：必须由理想介质内侧射向空气，并满足 Snell 临界条件。
fn internal_tir(
    event: ScatterType,
    material: &dyn Material,
    wo: Vector3<f64>,
    normal: Vector3<f64>,
) -> bool {
    if event != ScatterType::SpecularReflection {
        return false;
    }
    let (Some(ior), Some(wo), Some(normal)) =
        (material.ior(), unit_vector(wo), unit_vector(normal))
    else {
        return false;
    };
    let cos_i = -wo.dot(&normal);
    if !ior.is_finite() || ior <= 1.0 || cos_i <= 0.0 {
        return false;
    }
    let cos_i = cos_i.min(1.0);
    let sin_i = (1.0 - cos_i * cos_i).sqrt();
    // 与现有 Fresnel 的缩放临界条件一致，不用高折射率的平方或乘积。
    sin_i >= 1.0 / ior && fresnel_dielectric(cos_i, ior, 1.0) == Some(1.0)
}

/// 一像素的原始整数总计。除 samples / first_hit_objects 外，只统计首次命中调试目标的子样本。
/// AOV 用总 samples 作分母，保留子像素覆盖率；不将一条路径放大成整像素。
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct PathDebugPixel {
    pub samples: u64,
    pub first_hit_objects: BTreeMap<usize, u64>,
    pub target_samples: u64,
    pub first_hit_normal_sum: [f64; 3],
    pub events: PathEventCounts,
    pub reached_ground: u64,
    pub escaped_environment: u64,
    pub total_internal_reflection: u64,
    pub terminations: BTreeMap<PathTermination, u64>,
}

impl PathDebugPixel {
    pub(super) fn record(
        &mut self,
        sample: &PathDebugSample,
        target_object_id: Option<usize>,
    ) -> Result<(), &'static str> {
        add(&mut self.samples, 1)?;
        if let Some(id) = sample.first_hit_object_id {
            add(self.first_hit_objects.entry(id).or_default(), 1)?;
        }
        if target_object_id.is_none() || sample.first_hit_object_id != target_object_id {
            return Ok(());
        }
        add(&mut self.target_samples, 1)?;
        if let Some(normal) = sample.first_hit_normal {
            for (sum, value) in self.first_hit_normal_sum.iter_mut().zip(normal) {
                *sum += value;
            }
        }
        self.events.accumulate(sample.events)?;
        add(&mut self.reached_ground, u64::from(sample.reached_ground))?;
        add(
            &mut self.escaped_environment,
            u64::from(sample.escaped_environment),
        )?;
        add(
            &mut self.total_internal_reflection,
            u64::from(sample.total_internal_reflection),
        )?;
        let termination = sample.termination.ok_or("调试路径缺少终止原因")?;
        add(self.terminations.entry(termination).or_default(), 1)
    }
}

fn add(total: &mut u64, value: u64) -> Result<(), &'static str> {
    *total = total.checked_add(value).ok_or("路径调试计数溢出")?;
    Ok(())
}

/// 行优先原始统计；开启调试时随 CameraLightingImages 返回。
#[derive(Debug, PartialEq, Serialize)]
pub struct PathDebugBuffer {
    pub width: usize,
    pub height: usize,
    pub target_object_id: Option<usize>,
    pub pixels: Vec<PathDebugPixel>,
}

impl PathDebugBuffer {
    pub(super) fn new(width: usize, height: usize, target_object_id: Option<usize>) -> Self {
        Self {
            width,
            height,
            target_object_id,
            pixels: vec![PathDebugPixel::default(); width * height],
        }
    }

    /// 保存四张统计 PNG 与原始 JSON。传入普通主图路径，文件放在同目录；重名时加 aux_。
    /// 不接受曝光参数，RGB8 直接量化统计值，不应用任何颜色空间转换。
    pub fn save(&self, primary_output: impl AsRef<Path>) -> Result<Vec<PathBuf>, Box<dyn Error>> {
        let width = u32::try_from(self.width)?;
        let height = u32::try_from(self.height)?;
        if width == 0
            || height == 0
            || self.width.checked_mul(self.height) != Some(self.pixels.len())
        {
            return Err("路径调试缓冲尺寸无效".into());
        }
        let primary = primary_output.as_ref();
        let directory = primary
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(directory)?;
        let output = |name: &str| {
            let path = directory.join(name);
            if primary.file_name() == Some(std::ffi::OsStr::new(name)) {
                directory.join(format!("aux_{name}"))
            } else {
                path
            }
        };
        let mut paths = Vec::new();
        for (index, name) in [
            "path_target.png",
            "reflection_count.png",
            "transmission_count.png",
            "first_hit_normal.png",
        ]
        .iter()
        .enumerate()
        {
            let mut image = RgbImage::new(width, height);
            for (pixel, statistics) in image.pixels_mut().zip(&self.pixels) {
                *pixel = Rgb(aov_pixel(statistics, index));
            }
            let path = output(name);
            image.save_with_format(&path, ImageFormat::Png)?;
            paths.push(path);
        }
        let path = output("path_debug.json");
        let mut writer = BufWriter::new(fs::File::create(&path)?);
        serde_json::to_writer(&mut writer, self)?;
        writer.flush()?;
        paths.push(path);
        Ok(paths)
    }
}

fn aov_pixel(pixel: &PathDebugPixel, index: usize) -> [u8; 3] {
    if pixel.samples == 0 || pixel.target_samples == 0 {
        return [0; 3];
    }
    let n = pixel.samples as f64;
    match index {
        0 => [
            pixel.reached_ground,
            pixel.escaped_environment,
            pixel.total_internal_reflection,
        ]
        .map(|v| byte(v as f64 / n)),
        1 => {
            [count_byte(
                (pixel.events.specular_reflection as f64 + pixel.events.glossy_reflection as f64)
                    / n,
            ); 3]
        }
        2 => [count_byte(pixel.events.specular_transmission as f64 / n); 3],
        _ => pixel
            .first_hit_normal_sum
            .map(|v| byte((v + pixel.target_samples as f64) / (2.0 * n))),
    }
}

fn byte(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// 固定标尺：log2(1 + 平均次数) / log2(33)，32 次及以上为白色。
fn count_byte(mean: f64) -> u8 {
    byte(mean.ln_1p() / 33.0_f64.ln())
}

#[cfg(test)]
mod tests;

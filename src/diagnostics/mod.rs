//! 真实相机续追的观测记录；不采样、不计光，也不改变路径状态。
//! AOV 是直接量化的统计 RGB，不能经过曝光、XYZ 或 display sRGB 转换。

use crate::geometry::hit::Hit;
pub use crate::integrator::camera::path::StopReason;
use crate::material::Material;
use crate::material::ScatterType;
use crate::material::unit_vector;
use crate::optics::dispersion::IorMode;
use crate::optics::interface::fresnel_dielectric;
use image::ImageFormat;
use image::Rgb;
use image::RgbImage;
use nalgebra::Vector3;
use serde::Serialize;
use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

/// 已接受的路径续追事件数；不包含局部环境 NEE 的 BSDF 方向样本。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PathEventCounts {
    pub specular_reflection: u64,
    pub specular_transmission: u64,
    pub glossy_reflection: u64,
    pub diffuse_reflection: u64,
}

impl PathEventCounts {
    /// 把输入散射事件对应的计数加一，供单条真实相机路径记录事件次数；原地修改，无返回值。
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

    /// 把输入 other 的四种事件数分别加到 self，供像素汇总路径统计；成功 Ok(())，任一计数溢出 Err。
    fn accumulate(&mut self, other: Self) -> Result<(), &'static str> {
        add(&mut self.specular_reflection, other.specular_reflection)?;
        add(&mut self.specular_transmission, other.specular_transmission)?;
        add(&mut self.glossy_reflection, other.glossy_reflection)?;
        add(&mut self.diffuse_reflection, other.diffuse_reflection)
    }
}

/// 只在 trace_camera_path 的一次真实调用期间存在，不保存每条完整路径。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PathDebugSample {
    pub(crate) first_hit_object_id: Option<usize>,
    pub(crate) first_hit_normal: Option<[f64; 3]>,
    pub(crate) events: PathEventCounts,
    pub(crate) reached_ground: bool,
    /// 几何上逃逸到已配置的环境；不代表该 miss 一定累加了环境辐亮度。
    pub(crate) escaped_environment: bool,
    pub(crate) total_internal_reflection: bool,
    pub(crate) termination: Option<StopReason>,
}

impl PathDebugSample {
    /// 记录一次实际求交，供路径 AOV 标记首次物体/法线及是否到过地面。
    ///
    /// 输入：Hit 和可选 ground_id；输出：原地修改记录，无返回值；首次法线只写一次，后续命中可更新到地面标志。
    pub(crate) fn hit(&mut self, hit: &Hit, ground_id: Option<usize>) {
        if self.first_hit_normal.is_none() {
            self.first_hit_object_id = Some(hit.object_id);
            self.first_hit_normal = Some(hit.normal.into());
        }
        self.reached_ground |= ground_id.is_some() && Some(hit.object_id) == ground_id;
    }

    /// 记录一次已接受的路径散射，供事件次数和全反射 AOV 使用。
    ///
    /// 输入：event、材质、wo、几何法线、当前路径可选波长（nm）与折射率模式；
    /// 输出：原地更新计数/TIR 标志，无返回值，不抽样或改变物理路径。
    pub(crate) fn scatter(
        &mut self,
        event: ScatterType,
        material: &dyn Material,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        wavelength_nm: Option<f64>,
        ior_mode: IorMode,
    ) {
        self.events.record(event);
        self.total_internal_reflection |=
            internal_tir(event, material, wo, normal, wavelength_nm, ior_mode);
    }
}

/// 判断一次理想反射是否确实由玻璃内部全反射造成，供调试标记。
///
/// 输入：事件、材质、wo、朝外法线、可选波长（nm）与折射率模式；输出：bool，无效参数或普通反射返回 false。
/// 条件：内部玻璃 n(λ)>1、事件为镜面反射且 sinθ_i≥1/n，并核对精确 Fresnel F=1。
fn internal_tir(
    event: ScatterType,
    material: &dyn Material,
    wo: Vector3<f64>,
    normal: Vector3<f64>,
    wavelength_nm: Option<f64>,
    ior_mode: IorMode,
) -> bool {
    if event != ScatterType::SpecularReflection {
        return false;
    }
    let ior = match wavelength_nm {
        Some(nm) => material.photon_ior(Some(nm), ior_mode),
        None => material.ior(),
    };
    let (Some(ior), Some(wo), Some(normal)) = (ior, unit_vector(wo), unit_vector(normal)) else {
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

/// 像素内热循环使用的栈上累加器，避免每个子样本查询 `BTreeMap`。
#[derive(Default)]
pub(crate) struct PathDebugAccumulator {
    samples: u64,
    primary_hit_object: Option<(usize, u64)>,
    extra_hit_objects: BTreeMap<usize, u64>,
    target_samples: u64,
    first_hit_normal_sum: [f64; 3],
    events: PathEventCounts,
    reached_ground: u64,
    escaped_environment: u64,
    total_internal_reflection: u64,
    terminations: [u64; StopReason::ALL.len()],
}

impl PathDebugAccumulator {
    pub(crate) fn record(
        &mut self,
        sample: &PathDebugSample,
        target_object_id: Option<usize>,
    ) -> Result<(), &'static str> {
        add(&mut self.samples, 1)?;
        if let Some(id) = sample.first_hit_object_id {
            match &mut self.primary_hit_object {
                Some((primary_id, count)) if *primary_id == id => add(count, 1)?,
                None => self.primary_hit_object = Some((id, 1)),
                Some(_) => add(self.extra_hit_objects.entry(id).or_default(), 1)?,
            }
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
        add(&mut self.terminations[termination.index()], 1)
    }

    pub(crate) fn finish(mut self) -> PathDebugPixel {
        if let Some((id, count)) = self.primary_hit_object {
            self.extra_hit_objects.insert(id, count);
        }
        let mut terminations = BTreeMap::new();
        for reason in StopReason::ALL {
            let count = self.terminations[reason.index()];
            if count > 0 {
                terminations.insert(reason, count);
            }
        }
        PathDebugPixel {
            samples: self.samples,
            first_hit_objects: self.extra_hit_objects,
            target_samples: self.target_samples,
            first_hit_normal_sum: self.first_hit_normal_sum,
            events: self.events,
            reached_ground: self.reached_ground,
            escaped_environment: self.escaped_environment,
            total_internal_reflection: self.total_internal_reflection,
            terminations,
        }
    }
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
    pub terminations: BTreeMap<StopReason, u64>,
}

/// 把输入 value 安全加到可变 u64 total，供调试统计共用；成功 Ok(())，溢出 Err。
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
    /// 分配全零的逐像素路径统计，供开启 AOV 的 RGB 与光谱渲染共用。
    ///
    /// 输入：已校验宽高与可选目标物体编号；输出：PathDebugBuffer，像素按行存储，不分配逐路径历史。
    pub(crate) fn new(width: usize, height: usize, target_object_id: Option<usize>) -> Self {
        Self {
            width,
            height,
            target_object_id,
            pixels: vec![PathDebugPixel::default(); width * height],
        }
    }

    /// 保存路径统计图与原始计数，供检查反射/透射、终态和法线。
    ///
    /// 输入：self 统计缓冲及主图路径 primary_output；输出：四张 PNG 和一份 JSON 的路径列表，布局或写入失败 Err。
    /// 文件放在主图同目录，重名加 aux_；AOV 直接量化统计值，不应用曝光、XYZ 转换或 sRGB 编码。
    pub fn save(
        &self,
        primary_output: impl AsRef<Path>,
    ) -> Result<Vec<PathBuf>, Box<dyn Error + Send + Sync>> {
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
        let mut image = RgbImage::new(width, height);
        for (index, name) in [
            "path_target.png",
            "reflection_count.png",
            "transmission_count.png",
            "first_hit_normal.png",
        ]
        .iter()
        .enumerate()
        {
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

/// 把一个像素的原始统计映射成一张 AOV 的三个字节，供调试 PNG 导出。
///
/// 输入：PathDebugPixel 及图编号 0/1/2/其他；输出：`[R,G,B]`，无样本或无目标覆盖时为黑。
/// 0 表示到地面/逃逸环境/TIR 比例，1/2 为反射/透射平均次数，其他为首次法线映射；分母用所有样本数以保留覆盖率。
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

/// 将输入比例限制到 `[0,1]` 后按 round(255·value) 量化为 u8，供统计 AOV 编码。
fn byte(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// 把输入的平均事件次数映射成灰度字节，便于同时观察少量和多次事件。
///
/// 公式：byte=round(255·clamp(ln(1+mean)/ln(33),0,1))；32 次及以上为白色，所有图使用固定标尺。
fn count_byte(mean: f64) -> u8 {
    byte(mean.ln_1p() / 33.0_f64.ln())
}

#[cfg(test)]
mod tests;

mod wavelength;
pub use wavelength::{FixedWavelengthImage, render_fixed_wavelength_camera};
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub path: bool,
    pub target: Option<usize>,
    pub disable_ground_sun: bool,
}

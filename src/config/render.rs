//! 可保存为 JSON 的渲染参数；不包含 Framebuffer、RNG 或线程池。

use crate::optics::dispersion::IorMode;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;

pub(crate) const MAX_PHOTON_VOLUME_SCATTERS: usize = 1024;

pub(crate) fn validate_camera_workload(
    width: usize,
    height: usize,
    paths_per_pixel: usize,
    vertices_per_path: usize,
    environment_samples: usize,
    light_samples: usize,
) -> Result<(), &'static str> {
    let vertices = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(paths_per_pixel))
        .and_then(|paths| paths.checked_mul(vertices_per_path))
        .ok_or("相机路径顶点总数溢出")?;
    vertices
        .checked_mul(environment_samples)
        .ok_or("环境方向样本总数溢出")?;
    vertices
        .checked_mul(light_samples)
        .ok_or("有限光源样本总数溢出")?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraMode {
    #[default]
    Rgb,
    Spectral,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderSetting {
    #[cfg(feature = "diagnostics")]
    #[serde(default)]
    pub diagnostics: crate::diagnostics::Settings,
    /// 独立相机传输模式；省略时保留 RGB。光谱相机固定采样 380..780 nm。
    #[serde(default)]
    pub camera_mode: CameraMode,
    /// 输出图片宽、高，单位为像素。
    pub width: usize,
    pub height: usize,
    /// 光源发射的每轴分层数 s；网格为 (width·s)×(height·s)，总空间样本 N=width·height·s²。
    /// 例如 s=4 时，每个图像像素对应 16 条发射样本；不控制相机路径数。
    pub samples_per_pixel_axis: usize,
    /// 玻璃相机渲染的每像素路径总数，与光源每轴分层数独立。
    #[serde(default = "default_camera_spp")]
    pub camera_spp: usize,
    /// 每条初始相机射线承载的独立光谱路径数，仅支持 1/4；camera_spp 仍为总路径数。
    #[serde(default = "default_spectral_packet_size")]
    pub spectral_packet_size: usize,
    /// 光源端/光谱相机的 IOR 模式；RGB 相机始终使用材质的常数 ior。
    pub mode: IorMode,
    /// 仅 Forward 使用；每次理想反射/透射（含全反射）占一个事件。
    #[serde(default = "default_photon_max_events")]
    pub photon_max_events: usize,
    /// 仅 Forward 使用；首次玻璃事件后的真实体散射上限。0 只计算消光。
    #[serde(default)]
    pub photon_max_volume_scatters: usize,
    /// 光源端离散积分的波长范围和步长，单位 nm；Hero 相机固定为 380..780 nm。
    pub wavelength_min_nm: f64,
    pub wavelength_max_nm: f64,
    pub wavelength_step_nm: f64,
    /// 线性曝光倍率；在 sRGB 编码前使用。
    pub exposure: f64,
    pub seed: u64,
    /// None 使用 Rayon 默认线程数，Some(n) 指定正整数个线程。
    pub thread_count: Option<usize>,
    /// 空间采样块数量上限，与线程数量分开设置。
    pub max_spectral_blocks: usize,
    /// 主图输出路径；相对路径相对于启动程序的工作目录，三分量辅助图保存到同目录。
    pub output_path: PathBuf,
}

/// 为省略 camera_spp 的 JSON 返回内置值 64（每像素总路径数），展示场景可显式设为 256。
fn default_camera_spp() -> usize {
    64
}

/// 为省略 photon_max_events 的 JSON 返回 32，即 Forward 单条光子路径的散射事件上限。
fn default_photon_max_events() -> usize {
    32
}

/// 为省略 spectral_packet_size 的 JSON 返回 1，即每条主射线仅追踪一个波长。
fn default_spectral_packet_size() -> usize {
    1
}

/// 计算光谱相机每像素的空间样本数和总路径数，供配置与渲染入口统一解释 SPP。
///
/// 输入：总光谱路径数 spp 和 packet_size（仅 1/4）；输出：(spp/packet_size,spp)。
/// spp=0、非法 Packet 或 spp 不能整除 Packet 时返回 Err。
pub(crate) fn spectral_sample_counts(
    spp: usize,
    packet_size: usize,
) -> Result<(usize, usize), String> {
    if spp == 0 {
        return Err("render.camera_spp 必须大于零".into());
    }
    if !matches!(packet_size, 1 | 4) {
        return Err("render.spectral_packet_size 仅支持 1 或 4".into());
    }
    if !spp.is_multiple_of(packet_size) {
        return Err("render.camera_spp 必须能被 spectral_packet_size 整除".into());
    }
    let spatial_spp = spp / packet_size;
    let total_paths = spatial_spp
        .checked_mul(packet_size)
        .ok_or("光谱 Packet 路径数量溢出")?;
    Ok((spatial_spp, total_paths))
}

impl Default for RenderSetting {
    /// 返回 Rust 内置渲染配置：512×512、相机 64 SPP、Packet=1、每轴光子分层 4、seed=42。
    ///
    /// 输入：无；输出：RenderSetting，其他字段见构造体；展示 JSON 可覆盖这些数值。
    fn default() -> Self {
        Self {
            #[cfg(feature = "diagnostics")]
            diagnostics: Default::default(),
            camera_mode: CameraMode::Rgb,
            width: 512,
            height: 512,
            samples_per_pixel_axis: 4,
            camera_spp: default_camera_spp(),
            spectral_packet_size: default_spectral_packet_size(),
            mode: IorMode::Dispersion,
            photon_max_events: default_photon_max_events(),
            photon_max_volume_scatters: 0,
            wavelength_min_nm: 380.0,
            wavelength_max_nm: 780.0,
            wavelength_step_nm: 10.0,
            exposure: 1.0,
            seed: 42,
            thread_count: None,
            max_spectral_blocks: 16,
            output_path: PathBuf::from("output/caustics.png"),
        }
    }
}

impl RenderSetting {
    /// 生成离散光谱梯形积分所需的波长节点与权重，供光源 XYZ/Eλ 图集使用。
    ///
    /// 输入：self 的起止波长和步长（nm）；输出：`Vec<(λ_nm,Δλ_nm)>`，配置或分配无效返回 Err。
    ///
    /// 公式：内部节点 Δλ_i=(λ_{i+1}-λ_{i-1})/2，端点取相邻区间的一半；所有 Δλ 之和等于波段宽度。
    /// 最后区间可短于设置步长，精确包含右端点；这里的 Δλ 不用于 Hero 相机的随机波长权重。
    pub fn wavelength_samples(&self) -> Result<Vec<(f64, f64)>, String> {
        self.validate()?;
        let count =
            ((self.wavelength_max_nm - self.wavelength_min_nm) / self.wavelength_step_nm).ceil();
        if count >= (usize::MAX - 1) as f64 {
            return Err("波长积分样本数量溢出".into());
        }
        let intervals = count as usize;
        let mut wavelengths = Vec::new();
        wavelengths
            .try_reserve_exact(intervals + 1)
            .map_err(|_| "无法分配波长积分样本")?;
        for i in 0..intervals {
            let nm = self.wavelength_min_nm + i as f64 * self.wavelength_step_nm;
            if nm >= self.wavelength_max_nm {
                break;
            }
            if wavelengths.last().is_some_and(|previous| nm <= *previous) {
                return Err("波长步长低于浮点精度".into());
            }
            wavelengths.push(nm);
        }
        wavelengths.push(self.wavelength_max_nm);
        let mut samples = Vec::with_capacity(wavelengths.len());
        for (i, &nm) in wavelengths.iter().enumerate() {
            let left = if i == 0 { nm } else { wavelengths[i - 1] };
            let right = wavelengths.get(i + 1).copied().unwrap_or(nm);
            let width = (right - left) * 0.5;
            if !width.is_finite() || width <= 0.0 {
                return Err("波长步长低于浮点精度，不能建立积分节点".into());
            }
            samples.push((nm, width));
        }
        Ok(samples)
    }

    /// 检查渲染参数，供分配图像和开始追踪前使用。
    ///
    /// 输入：self；输出：有效时 Ok(())，非法尺寸/采样/波段/曝光/线程数、空路径或工作量溢出时 Err。
    /// 只检查数值，不分配图像或创建线程池。
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=1024).contains(&self.photon_max_events) {
            return Err("render.photon_max_events 必须位于 1..1024".into());
        }
        if self.photon_max_volume_scatters > MAX_PHOTON_VOLUME_SCATTERS {
            return Err(format!(
                "render.photon_max_volume_scatters 不能超过 {MAX_PHOTON_VOLUME_SCATTERS}"
            ));
        }
        if self.width == 0 || self.height == 0 || self.samples_per_pixel_axis == 0 {
            return Err("render 的图片尺寸和每轴采样数必须大于零".into());
        }
        if u32::try_from(self.width).is_err() || u32::try_from(self.height).is_err() {
            return Err("render 图片尺寸超出 PNG 支持的 u32 范围".into());
        }
        self.width
            .checked_mul(self.height)
            .ok_or("render 像素数量溢出")?;
        let (_, total_paths) = spectral_sample_counts(self.camera_spp, self.spectral_packet_size)?;
        self.width
            .checked_mul(self.height)
            .and_then(|pixels| pixels.checked_mul(total_paths))
            .ok_or("render 相机路径样本总数溢出")?;
        self.width
            .checked_mul(self.samples_per_pixel_axis)
            .and_then(|x| {
                self.height
                    .checked_mul(self.samples_per_pixel_axis)
                    .and_then(|y| x.checked_mul(y))
            })
            .ok_or("render 空间光线数量溢出")?;
        if !self.wavelength_min_nm.is_finite()
            || !self.wavelength_max_nm.is_finite()
            || self.wavelength_min_nm < 380.0
            || self.wavelength_max_nm > 780.0
            || self.wavelength_min_nm >= self.wavelength_max_nm
            || !self.wavelength_step_nm.is_finite()
            || self.wavelength_step_nm <= 0.0
            || self.wavelength_step_nm > self.wavelength_max_nm - self.wavelength_min_nm
        {
            return Err("render 波长范围须位于 380–780 nm，且范围和积分步长必须有限、为正".into());
        }
        if !((self.wavelength_max_nm - self.wavelength_min_nm) / self.wavelength_step_nm)
            .is_finite()
        {
            return Err("render 波长积分样本数量溢出".into());
        }
        if self.wavelength_min_nm + self.wavelength_step_nm <= self.wavelength_min_nm {
            return Err("render 波长步长低于浮点精度".into());
        }
        if !self.exposure.is_finite() || self.exposure < 0.0 {
            return Err("render.exposure 必须有限且非负".into());
        }
        if self.thread_count == Some(0) || self.max_spectral_blocks == 0 {
            return Err("render 线程数和采样块数必须大于零".into());
        }
        if self.output_path.as_os_str().is_empty() {
            return Err("render.output_path 不能为空".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wavelength_nodes_include_short_last_interval_and_keep_trapezoid_energy() {
        let settings = RenderSetting {
            wavelength_min_nm: 400.0,
            wavelength_max_nm: 425.0,
            wavelength_step_nm: 10.0,
            ..RenderSetting::default()
        };
        assert_eq!(
            settings.wavelength_samples().unwrap(),
            vec![(400.0, 5.0), (410.0, 10.0), (420.0, 7.5), (425.0, 2.5)]
        );
        let samples = RenderSetting::default().wavelength_samples().unwrap();
        assert_eq!(samples.len(), 41);
        assert_eq!(samples.first(), Some(&(380.0, 5.0)));
        assert_eq!(samples.last(), Some(&(780.0, 5.0)));
        assert_eq!(samples.iter().map(|s| s.1).sum::<f64>(), 400.0);
        let fractional = RenderSetting {
            wavelength_min_nm: 400.0,
            wavelength_max_nm: 401.1,
            wavelength_step_nm: 0.1,
            ..settings
        };
        let samples = fractional.wavelength_samples().unwrap();
        assert_eq!(samples.first().unwrap().0, 400.0);
        assert_eq!(samples.last().unwrap().0, 401.1);
        assert!(samples.windows(2).all(|pair| pair[1].0 > pair[0].0));
        assert!((samples.iter().map(|s| s.1).sum::<f64>() - 1.1).abs() < 1e-12);
    }
}

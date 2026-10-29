//! 可保存为 JSON 的渲染参数；不包含 Framebuffer、RNG 或线程池。

use crate::optics::iormode::IorMode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderSetting {
    /// 输出图片宽、高，单位为像素。
    pub width: usize,
    pub height: usize,
    /// 每个发射小格沿每轴分层的数量；4 表示 4×4 个空间样本。
    pub samples_per_pixel_axis: usize,
    /// 玻璃相机渲染的每像素路径总数，与光源每轴分层数独立。
    #[serde(default = "default_camera_spp")]
    pub camera_spp: usize,
    pub mode: IorMode,
    /// 波长范围和积分步长，单位为 nm。
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
    /// 主焦散图的输出路径；相对路径相对于启动程序的工作目录。
    pub output_path: PathBuf,
}

fn default_camera_spp() -> usize {
    64
}

impl Default for RenderSetting {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            samples_per_pixel_axis: 4,
            camera_spp: default_camera_spp(),
            mode: IorMode::Dispersion,
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
    /// 梯形积分节点和每点宽度；最后一段允许短于步长，并精确包含右端点。
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

    /// 只校验配置数值，不分配图片或创建运行时渲染器。
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.samples_per_pixel_axis == 0 {
            return Err("render 的图片尺寸和每轴采样数必须大于零".into());
        }
        if u32::try_from(self.width).is_err() || u32::try_from(self.height).is_err() {
            return Err("render 图片尺寸超出 PNG 支持的 u32 范围".into());
        }
        self.width
            .checked_mul(self.height)
            .ok_or("render 像素数量溢出")?;
        if self.camera_spp == 0 {
            return Err("render.camera_spp 必须大于零".into());
        }
        self.width
            .checked_mul(self.height)
            .and_then(|pixels| pixels.checked_mul(self.camera_spp))
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

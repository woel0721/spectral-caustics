//! 未积分的地面光谱辐照度；与 RGB 光源图共享光子追踪和通量标尺。

use super::{for_each_emitted_hit, ground_splat_weights, spectral_sample_blocks};
use crate::{
    config::rendersetting::RenderSetting,
    optics::{iormode::IorMode, spectrum::d65_spd},
    scene::{GroundReceiver, Scene, take_photon_from_hit},
};
use nalgebra::Vector3;
use rayon::prelude::*;
use std::error::Error;

/// 每层为 Eλ（相对通量 / m² / nm），不含 CIE、Δλ、地面反射率或曝光。
/// 字段私有以保持布局有效；渲染后只读借用，不在相机像素/Packet 中复制。
pub struct SpectralCausticsMap {
    width: usize,
    height: usize,
    receiver: GroundReceiver,
    ground_id: usize,
    mode: IorMode,
    /// 与原光源 XYZ 积分完全相同的节点及梯形积分宽度。
    samples: Vec<(f64, f64)>,
    layers: Vec<Vec<f64>>,
}

impl SpectralCausticsMap {
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub fn ground_id(&self) -> usize {
        self.ground_id
    }

    pub fn mode(&self) -> IorMode {
        self.mode
    }

    pub fn receiver(&self) -> &GroundReceiver {
        &self.receiver
    }

    /// (λ nm, 梯形积分宽度 nm)；宽度仅用于离散 XYZ 校验，不用于 Camera 查询。
    pub fn wavelength_samples(&self) -> &[(f64, f64)] {
        &self.samples
    }

    /// 行优先的原始 Eλ，只读；可用于通量/落点诊断。
    pub fn layer_irradiance(&self, index: usize) -> Option<&[f64]> {
        self.layers.get(index).map(Vec::as_slice)
    }

    /// 世界 x/y 双线性采样，再在相邻波长节点间线性插值。
    /// 接收域为 [min,max)，域外/配置发射波段外为零；非有限输入返回 None。
    /// 不外推窄波段数据，也不重复乘 Δλ 或除以像素面积。
    pub fn sample(&self, position: Vector3<f64>, wavelength_nm: f64) -> Option<f64> {
        if !wavelength_nm.is_finite() || position.iter().any(|v| !v.is_finite()) {
            return None;
        }
        if wavelength_nm < self.samples.first()?.0 || wavelength_nm > self.samples.last()?.0 {
            return Some(0.0);
        }
        let weights =
            ground_splat_weights(position, self.width, self.height, &self.receiver).ok()?;
        let right = self.samples.partition_point(|&(nm, _)| nm < wavelength_nm);
        let left = right.saturating_sub(1);
        let t = if left == right {
            0.0
        } else {
            (wavelength_nm - self.samples[left].0) / (self.samples[right].0 - self.samples[left].0)
        };
        let spatial = |layer: usize| -> f64 {
            weights
                .iter()
                .map(|&(x, y, w)| self.layers[layer][y * self.width + x] * w)
                .sum()
        };
        let value = spatial(left) * (1.0 - t) + spatial(right) * t;
        (value.is_finite() && value >= 0.0).then_some(value)
    }
}

/// 仅显式启用且支持理想双透射的焦散目标才生成图集；否则立即返回 None。
/// Dispersion 按波长并行，每层只分配一个标量缓冲；没有每采样块的完整图集。
/// Constant 只追踪一次几何通量图，再乘每个节点的 D65 密度。
pub fn render_wavelength_caustics(
    scene: &Scene,
    settings: &RenderSetting,
) -> Result<Option<SpectralCausticsMap>, Box<dyn Error>> {
    settings.validate()?;
    let Some(object_id) = scene.caustics_object_id else {
        return Ok(None);
    };
    if object_id >= scene.objects.len() {
        return Err("焦散物体编号不在场景中".into());
    }
    if !scene.supports_ideal_caustics(object_id) {
        return Ok(None);
    }
    let ground_id = scene.ground_id.ok_or("光谱焦散缺少接收地面")?;
    if ground_id == object_id
        || scene
            .objects
            .get(ground_id)
            .and_then(|o| o.material_id())
            .and_then(|id| scene.material(id))
            .and_then(|m| m.irradiance_reflectance())
            .is_none()
    {
        return Err("光谱焦散要求有效的 Lambertian 地面".into());
    }
    scene.light.validate_emission()?;
    let receiver = scene.receiver;
    let span = receiver.max_m - receiver.min_m;
    if receiver
        .min_m
        .iter()
        .chain(receiver.max_m.iter())
        .any(|v| !v.is_finite())
        || span.iter().any(|v| !v.is_finite() || *v <= 0.0)
        || !receiver.area().is_finite()
        || receiver.area() <= 0.0
    {
        return Err("光谱焦散接收范围无效".into());
    }
    let samples = settings.wavelength_samples()?;
    let count = settings
        .width
        .checked_mul(settings.height)
        .ok_or("焦散图像素数溢出")?;
    count
        .checked_mul(samples.len())
        .and_then(|n| n.checked_mul(size_of::<f64>()))
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or("单波长焦散图集大小溢出")?;
    let nx = settings
        .width
        .checked_mul(settings.samples_per_pixel_axis)
        .ok_or("焦散采样数溢出")?;
    let ny = settings
        .height
        .checked_mul(settings.samples_per_pixel_axis)
        .ok_or("焦散采样数溢出")?;
    let rays = nx.checked_mul(ny).ok_or("焦散光线数量溢出")?;
    // 与原光源图完全相同：每条空间光线的通量 / 地面像素面积。
    let spatial_weight =
        (scene.light.total_flux() / rays as f64) / (receiver.area() / count as f64);
    let k = scene
        .light
        .spectral_normalization(&samples)
        .ok_or("D65 光谱归一化失败")?;
    let geometry_layer = |nm: f64| -> Result<Vec<f64>, &'static str> {
        let ior = scene
            .photon_ior(object_id, Some(nm), settings.mode)
            .ok_or("焦散 IOR 无效")?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(count)
            .map_err(|_| "无法分配单波长焦散层")?;
        pixels.resize(count, 0.0);
        // 每层以同一固定顺序累加同一批空间样本；线程数不改变浮点分组。
        for (rows, mut rng) in spectral_sample_blocks(
            nx,
            ny,
            settings.seed,
            settings.max_spectral_blocks,
            scene.light.emission_dimensions(),
        ) {
            for_each_emitted_hit(scene, nx, ny, rows, &mut rng, |ray, hit| {
                if hit.object_id != Some(object_id) {
                    return Ok(());
                }
                if let Some(landing) =
                    take_photon_from_hit(scene, ray, hit, object_id, ground_id, ior)
                {
                    let value = spatial_weight * landing.weight;
                    if !value.is_finite() || value < 0.0 {
                        return Err("单波长焦散通量溢出");
                    }
                    for (x, y, w) in ground_splat_weights(
                        landing.position,
                        settings.width,
                        settings.height,
                        &receiver,
                    )? {
                        let p = &mut pixels[y * settings.width + x];
                        *p += value * w;
                        if !p.is_finite() {
                            return Err("单波长焦散累积溢出");
                        }
                    }
                }
                Ok(())
            })?;
        }
        Ok(pixels)
    };
    let constant = (settings.mode == IorMode::Constant)
        .then(|| geometry_layer(samples[0].0))
        .transpose()?;
    let layers: Result<Vec<_>, &'static str> = samples
        .par_iter()
        .map(|&(nm, _)| {
            let mut layer = match &constant {
                Some(pixels) => pixels.clone(),
                None => geometry_layer(nm)?,
            };
            // Eλ = spatial_weight × T1 × T2 × k × D65(λ)，这里没有 Δλ。
            let density = k * d65_spd(nm).ok_or("D65 查询失败")?;
            for value in &mut layer {
                *value *= density;
                if !value.is_finite() || *value < 0.0 {
                    return Err("单波长辐照度溢出");
                }
            }
            Ok(layer)
        })
        .collect();
    Ok(Some(SpectralCausticsMap {
        width: settings.width,
        height: settings.height,
        receiver,
        ground_id,
        mode: settings.mode,
        samples,
        layers: layers?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra::Vector2;

    #[test]
    fn queries_interpolate_scalar_density_in_space_and_wavelength_with_clamped_edges() {
        let map = SpectralCausticsMap {
            width: 2,
            height: 2,
            receiver: GroundReceiver {
                min_m: Vector2::zeros(),
                max_m: Vector2::repeat(2.0),
            },
            ground_id: 0,
            mode: IorMode::Constant,
            samples: vec![(400.0, 50.0), (500.0, 55.0), (510.0, 5.0)],
            layers: vec![
                vec![1.0, 3.0, 5.0, 7.0],
                vec![3.0, 9.0, 15.0, 21.0],
                vec![4.0; 4],
            ],
        };
        let at = |x, y, nm| map.sample(Vector3::new(x, y, 0.0), nm).unwrap();
        assert_eq!(at(0.5, 1.5, 400.0), 1.0);
        assert_eq!(at(0.0, 0.0, 400.0), 5.0);
        assert_eq!(at(1.0, 1.0, 400.0), 4.0);
        assert_eq!(at(1.0, 1.0, 450.0), 8.0);
        assert_eq!(at(1.0, 1.0, 500.0), 12.0);
        assert_eq!(at(1.0, 1.0, 505.0), 8.0);
        assert_eq!(at(1.0, 1.0, 510.0), 4.0);
        for (x, y, nm) in [
            (2.0, 1.0, 450.0),
            (-0.1, 1.0, 450.0),
            (1.0, 2.0, 450.0),
            (1.0, 1.0, 399.0),
            (1.0, 1.0, 511.0),
        ] {
            assert_eq!(at(x, y, nm), 0.0);
        }
        assert!(map.sample(Vector3::zeros(), f64::NAN).is_none());
        assert!(map.sample(Vector3::repeat(f64::INFINITY), 450.0).is_none());
    }
}

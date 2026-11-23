//! World-space irradiance queries and conservative bilinear deposition.
use super::GroundReceiver;
use crate::optics::dispersion::IorMode;
use nalgebra::Vector3;
use std::{fmt, ops::Range};
pub struct SpectralCausticsMap {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) receiver: GroundReceiver,
    pub(crate) ground_id: usize,
    pub(crate) mode: IorMode,
    /// 与原光源 XYZ 积分完全相同的节点及梯形积分宽度。
    pub(crate) samples: Vec<(f64, f64)>,
    pub(crate) storage: SpectralAtlasStorage,
}

pub(crate) enum SpectralAtlasStorage {
    PerWavelength(Vec<Vec<f64>>),
    ScaledSingle {
        base: Option<Vec<f64>>,
        node_densities: Vec<f64>,
    },
}

/// Read-only, row-major Eλ values in relative flux per m² per nm.
#[derive(Clone, Copy)]
pub struct LayerIrradiance<'a> {
    pixels: &'a [f64],
    len: usize,
    scale: f64,
}

impl<'a> LayerIrradiance<'a> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Read a pixel by its row-major index.
    pub fn get(&self, index: usize) -> Option<f64> {
        (index < self.len).then(|| self.value(index))
    }

    pub fn iter(&self) -> LayerIrradianceIter<'a> {
        self.into_iter()
    }

    /// Materialize a dense copy of the layer.
    pub fn to_vec(&self) -> Vec<f64> {
        self.iter().collect()
    }

    fn value(&self, index: usize) -> f64 {
        self.pixels.get(index).map_or(0.0, |&v| v * self.scale)
    }
}

impl fmt::Debug for LayerIrradiance<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq for LayerIrradiance<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

/// Iterator over the logical values of a spectral irradiance layer.
pub struct LayerIrradianceIter<'a> {
    layer: LayerIrradiance<'a>,
    indices: Range<usize>,
}

impl Iterator for LayerIrradianceIter<'_> {
    type Item = f64;

    fn next(&mut self) -> Option<Self::Item> {
        self.indices.next().map(|i| self.layer.value(i))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.indices.size_hint()
    }
}

impl DoubleEndedIterator for LayerIrradianceIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.indices.next_back().map(|i| self.layer.value(i))
    }
}

impl ExactSizeIterator for LayerIrradianceIter<'_> {}

impl<'a> IntoIterator for LayerIrradiance<'a> {
    type Item = f64;
    type IntoIter = LayerIrradianceIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        LayerIrradianceIter {
            layer: self,
            indices: 0..self.len,
        }
    }
}

impl<'a> IntoIterator for &LayerIrradiance<'a> {
    type Item = f64;
    type IntoIter = LayerIrradianceIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        (*self).into_iter()
    }
}

impl SpectralCausticsMap {
    /// 返回每个波长层的 (宽,高)，单位像素，供查询和诊断读取图集布局。
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// 返回图集所绑定的地面物体编号，供相机检查是否查询了正确地面。
    pub fn ground_id(&self) -> usize {
        self.ground_id
    }

    /// 返回生成图集时使用的 IOR 模式，供相机拒绝不匹配的 Constant/Dispersion 数据。
    pub fn mode(&self) -> IorMode {
        self.mode
    }

    /// 返回图集覆盖的世界 XY 接收范围（米）的只读引用，供采样和通量诊断。
    pub fn receiver(&self) -> &GroundReceiver {
        &self.receiver
    }

    /// 返回只读节点表 (λ_nm,Δλ_nm)，供离散 XYZ 重积分校验；Δλ 不能再乘到相机的 Eλ 查询结果。
    pub fn wavelength_samples(&self) -> &[(f64, f64)] {
        &self.samples
    }

    /// 读取指定波长层的原始 Eλ，供通量和落点诊断。
    ///
    /// 输入：节点下标 index。输出：行优先、零分配的只读视图，越界返回 None。
    pub fn layer_irradiance(&self, index: usize) -> Option<LayerIrradiance<'_>> {
        let (pixels, scale) = match &self.storage {
            SpectralAtlasStorage::PerWavelength(layers) => (layers.get(index)?.as_slice(), 1.0),
            SpectralAtlasStorage::ScaledSingle {
                base,
                node_densities,
            } => (
                base.as_deref().unwrap_or_default(),
                *node_densities.get(index)?,
            ),
        };
        Some(LayerIrradiance {
            pixels,
            len: self.width * self.height,
            scale,
        })
    }

    /// 在世界位置和波长上插值焦散辐照度，供光谱相机地面着色。
    ///
    /// 输入：世界 position（米）和 wavelength_nm；输出：Eλ（相对通量/(m²·nm)），非有限或计算无效返回 None。
    /// XY 使用双线性插值，λ 使用节点间线性插值。共享底图时先插值节点密度，再乘底图的空间插值。
    /// 接收区 [min,max) 或发射波段外返回零；不外推，不再乘 Δλ 或除像素面积。
    pub fn sample(&self, position: Vector3<f64>, wavelength_nm: f64) -> Option<f64> {
        if !wavelength_nm.is_finite() || position.iter().any(|v| !v.is_finite()) {
            return None;
        }
        if wavelength_nm < self.samples.first()?.0 || wavelength_nm > self.samples.last()?.0 {
            return Some(0.0);
        }
        let Some(weights) =
            ground_splat_weights(position, self.width, self.height, &self.receiver).ok()?
        else {
            return Some(0.0);
        };
        let right = self.samples.partition_point(|&(nm, _)| nm < wavelength_nm);
        // Exact nodes use a single value, without evaluating the adjacent layer.
        let left = if wavelength_nm == self.samples[right].0 {
            right
        } else {
            right - 1
        };
        let t = if left == right {
            0.0
        } else {
            (wavelength_nm - self.samples[left].0) / (self.samples[right].0 - self.samples[left].0)
        };
        let spatial = |pixels: &[f64]| -> f64 {
            let mut sum = 0.0;
            for &(x, y, w) in &weights {
                if w != 0.0 {
                    sum += pixels[y * self.width + x] * w;
                }
            }
            sum
        };
        let value = match &self.storage {
            SpectralAtlasStorage::PerWavelength(layers) => {
                let a = spatial(&layers[left]);
                if left == right {
                    a
                } else {
                    a * (1.0 - t) + spatial(&layers[right]) * t
                }
            }
            SpectralAtlasStorage::ScaledSingle {
                base,
                node_densities,
            } => {
                let Some(base) = base else { return Some(0.0) };
                let a = node_densities[left];
                let density = if left == right {
                    a
                } else {
                    a * (1.0 - t) + node_densities[right] * t
                };
                spatial(base) * density
            }
        };
        (value.is_finite() && value >= 0.0).then_some(value)
    }
}
pub fn ground_to_pixel(
    position: Vector3<f64>,
    width: usize,
    height: usize,
    receiver: &GroundReceiver,
) -> Option<(f64, f64)> {
    if width == 0
        || height == 0
        || !position.iter().all(|v| v.is_finite())
        || !(receiver.min_m.x..receiver.max_m.x).contains(&position.x)
        || !(receiver.min_m.y..receiver.max_m.y).contains(&position.y)
    {
        return None;
    }
    let span = receiver.max_m - receiver.min_m;
    let u = (position.x - receiver.min_m.x) / span.x;
    let v = (position.y - receiver.min_m.y) / span.y;
    Some((u * width as f64 - 0.5, (1.0 - v) * height as f64 - 0.5))
}
pub(crate) type BilinearWeights = [(usize, usize, f64); 4];

pub(crate) fn ground_splat_weights(
    position: Vector3<f64>,
    width: usize,
    height: usize,
    receiver: &GroundReceiver,
) -> Result<Option<BilinearWeights>, &'static str> {
    match ground_to_pixel(position, width, height, receiver) {
        Some((x, y)) => bilinear_weights(x, y, width, height)
            .map(Some)
            .ok_or("双线性权重计算失败"),
        None => Ok(None),
    }
}
pub fn bilinear_weights(x: f64, y: f64, width: usize, height: usize) -> Option<BilinearWeights> {
    if width == 0
        || height == 0
        || !x.is_finite()
        || !y.is_finite()
        || x < -0.5
        || x > width as f64 - 0.5
        || y < -0.5
        || y > height as f64 - 0.5
    {
        return None;
    }

    // 区域内的边缘落点仍应保留全部能量，不向图片之外分配权重。
    let x = x.clamp(0.0, (width - 1) as f64);
    let y = y.clamp(0.0, (height - 1) as f64);
    let x0 = (x.floor() as usize).min(width - 1);
    let y0 = (y.floor() as usize).min(height - 1);
    let x1 = x0.saturating_add(1).min(width - 1);
    let y1 = y0.saturating_add(1).min(height - 1);
    let dx = (x - x0 as f64).clamp(0.0, 1.0);
    let dy = (y - y0 as f64).clamp(0.0, 1.0);

    // 先在 x 方向分成 (1-dx)、dx，再在 y 方向分成 (1-dy)、dy。
    Some([
        (x0, y0, (1.0 - dx) * (1.0 - dy)),
        (x1, y0, dx * (1.0 - dy)),
        (x0, y1, (1.0 - dx) * dy),
        (x1, y1, dx * dy),
    ])
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
            storage: SpectralAtlasStorage::PerWavelength(vec![
                vec![1.0, 3.0, 5.0, 7.0],
                vec![3.0, 9.0, 15.0, 21.0],
                vec![4.0; 4],
            ]),
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

    #[test]
    fn scaled_single_interpolates_density_and_exposes_logical_layers() {
        let mut map = SpectralCausticsMap {
            width: 2,
            height: 2,
            receiver: GroundReceiver {
                min_m: Vector2::zeros(),
                max_m: Vector2::repeat(2.0),
            },
            ground_id: 0,
            mode: IorMode::Constant,
            samples: vec![(400.0, 50.0), (500.0, 55.0), (510.0, 5.0)],
            storage: SpectralAtlasStorage::ScaledSingle {
                base: Some(vec![1.0, 3.0, 5.0, 7.0]),
                node_densities: vec![1.0, 3.0, 0.0],
            },
        };
        let p = Vector3::new(1.0, 1.0, 0.0);
        for (nm, expected) in [
            (400.0, 4.0),
            (450.0, 8.0),
            (500.0, 12.0),
            (505.0, 6.0),
            (510.0, 0.0),
        ] {
            assert_eq!(map.sample(p, nm), Some(expected));
        }
        let layer = map.layer_irradiance(1).unwrap();
        assert_eq!(layer.len(), 4);
        assert!(!layer.is_empty());
        assert_eq!(layer.get(3), Some(21.0));
        assert_eq!(layer.get(4), None);
        let scaled = vec![3.0, 9.0, 15.0, 21.0];
        assert_eq!(layer.to_vec(), scaled);
        assert_eq!(layer.iter().len(), 4);
        assert_eq!(layer.iter().next_back(), Some(21.0));
        assert!(layer.into_iter().eq(scaled.iter().copied()));
        assert!(IntoIterator::into_iter(&layer).eq(scaled.iter().copied()));
        assert_eq!(
            layer,
            LayerIrradiance {
                pixels: &scaled,
                len: 4,
                scale: 1.0
            }
        );
        assert!(map.layer_irradiance(3).is_none());

        map.storage = SpectralAtlasStorage::ScaledSingle {
            base: None,
            node_densities: vec![1.0, 3.0, 0.0],
        };
        assert_eq!(map.sample(p, 450.0), Some(0.0));
        let layer = map.layer_irradiance(1).unwrap();
        assert_eq!(layer.get(0), Some(0.0));
        assert_eq!(layer.get(4), None);
        assert_eq!(layer.to_vec(), vec![0.0; 4]);
    }

    #[test]
    fn fractional_position_has_expected_four_weights() {
        assert_eq!(
            bilinear_weights(1.25, 0.75, 4, 3),
            Some([
                (1, 0, 0.1875),
                (2, 0, 0.0625),
                (1, 1, 0.5625),
                (2, 1, 0.1875)
            ])
        );
    }

    #[test]
    fn pixel_center_contributes_only_to_that_pixel() {
        let samples = bilinear_weights(1.0, 1.0, 4, 3).unwrap();
        assert_eq!(samples[0], (1, 1, 1.0));
        assert!(samples[1..].iter().all(|&(_, _, weight)| weight == 0.0));
    }

    #[test]
    fn edge_and_single_pixel_weights_stay_in_bounds_and_sum_to_one() {
        for (width, height) in [(4, 3), (1, 4), (4, 1), (1, 1)] {
            for x in [-0.5, -0.25, 0.0, (width - 1) as f64, width as f64 - 0.5] {
                for y in [-0.5, -0.25, 0.0, (height - 1) as f64, height as f64 - 0.5] {
                    let samples = bilinear_weights(x, y, width, height).unwrap();
                    assert!(
                        samples
                            .iter()
                            .all(|&(x, y, w)| x < width && y < height && w.is_finite() && w >= 0.0)
                    );
                    let total: f64 = samples.iter().map(|&(_, _, weight)| weight).sum();
                    assert!((total - 1.0).abs() < 1e-12);
                }
            }
        }
        let samples = bilinear_weights(-0.25, 1.25, 4, 3).unwrap();
        assert!(
            samples
                .iter()
                .filter(|&&(_, _, w)| w > 0.0)
                .all(|&(x, _, _)| x == 0)
        );
    }

    #[test]
    fn invalid_coordinates_and_empty_dimensions_are_rejected() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5001, 3.5001] {
            assert!(bilinear_weights(value, 0.0, 4, 4).is_none());
            assert!(bilinear_weights(0.0, value, 4, 4).is_none());
        }
        assert!(bilinear_weights(0.0, 0.0, 0, 4).is_none());
        assert!(bilinear_weights(0.0, 0.0, 4, 0).is_none());
    }
}

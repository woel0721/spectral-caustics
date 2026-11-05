//! World-space irradiance queries and conservative bilinear deposition.
use super::GroundReceiver;
use crate::optics::dispersion::IorMode;
use nalgebra::Vector3;
pub struct SpectralCausticsMap {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) receiver: GroundReceiver,
    pub(crate) ground_id: usize,
    pub(crate) mode: IorMode,
    /// 与原光源 XYZ 积分完全相同的节点及梯形积分宽度。
    pub(crate) samples: Vec<(f64, f64)>,
    pub(crate) layers: Vec<Vec<f64>>,
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
    /// 输入：节点下标 index；输出：行优先只读标量切片，单位相对通量/(m²·nm)，越界返回 None。
    pub fn layer_irradiance(&self, index: usize) -> Option<&[f64]> {
        self.layers.get(index).map(Vec::as_slice)
    }

    /// 在世界位置和波长上插值焦散辐照度，供光谱相机地面着色。
    ///
    /// 输入：世界 position（米）和 wavelength_nm；输出：Eλ（相对通量/(m²·nm)），非有限或计算无效返回 None。
    /// 先按 XY 双线性权重求两层 E0/E1，再用 t=(λ-λ0)/(λ1-λ0) 得到 Eλ=(1-t)E0+tE1。
    /// 接收区 [min,max) 或发射波段外返回零；不外推，不再乘 Δλ 或除像素面积。
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
pub(crate) fn ground_splat_weights(
    position: Vector3<f64>,
    width: usize,
    height: usize,
    receiver: &GroundReceiver,
) -> Result<[(usize, usize, f64); 4], &'static str> {
    match ground_to_pixel(position, width, height, receiver) {
        Some((x, y)) => bilinear_weights(x, y, width, height).ok_or("双线性权重计算失败"),
        None => Ok([(0, 0, 0.0); 4]),
    }
}
pub fn bilinear_weights(
    x: f64,
    y: f64,
    width: usize,
    height: usize,
) -> Option<[(usize, usize, f64); 4]> {
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

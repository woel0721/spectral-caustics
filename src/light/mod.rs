mod parallel;
pub use parallel::ParallelLight;
// Finite disk and parallel sunlight.

mod disk;
pub use disk::DiskAreaLight;
pub use disk::DiskDirectSample;
pub use disk::DiskEmissionSample;
pub use disk::DiskPositionSample;

use nalgebra::Vector3;

#[derive(Debug, Clone)]
pub enum Light {
    Parallel(ParallelLight),
    DiskArea(DiskAreaLight),
}

impl Light {
    /// 计算发射 D65 光谱的归一化系数，使白光亮度 Y=1。
    ///
    /// 输入：(波长 nm,积分宽度 nm) 节点；输出：Some(k)，离散归一化失败时 None。
    /// 平行光 k=1/Σ[D65·ȳ·Δλ]；圆盘使用连续 380–780 nm 的固定 k，避免节点步长改变光谱密度。
    pub(crate) fn spectral_normalization(&self, samples: &[(f64, f64)]) -> Option<f64> {
        match self {
            Self::Parallel(_) => crate::spectrum::d65_normalization(samples),
            Self::DiskArea(_) => Some(crate::spectrum::emission::d65_normalization_continuous()),
        }
    }
    /// Integrate the normalized source spectrum into XYZ, independently of caustics geometry.
    pub(crate) fn white_xyz(&self, samples: &[(f64, f64)]) -> Option<Vector3<f64>> {
        let k = self.spectral_normalization(samples)?;
        let mut xyz = Vector3::zeros();
        for &(nm, width) in samples {
            xyz += crate::spectrum::cie_xyz(nm)? * (k * crate::spectrum::d65_spd(nm)? * width);
        }
        xyz.iter().all(|v| v.is_finite()).then_some(xyz)
    }
    /// 尝试读取平行光参数；当前为 Parallel 时返回只读引用，否则 None。
    pub fn as_parallel(&self) -> Option<&ParallelLight> {
        match self {
            Self::Parallel(light) => Some(light),
            Self::DiskArea(_) => None,
        }
    }
    /// 尝试修改平行光参数；当前为 Parallel 时返回可变引用，否则 None，修改后由使用入口重新校验。
    pub fn as_parallel_mut(&mut self) -> Option<&mut ParallelLight> {
        match self {
            Self::Parallel(light) => Some(light),
            Self::DiskArea(_) => None,
        }
    }
    /// 尝试读取圆盘光源参数；当前为 DiskArea 时返回只读引用，否则 None。
    pub fn as_disk(&self) -> Option<&DiskAreaLight> {
        match self {
            Self::DiskArea(light) => Some(light),
            Self::Parallel(_) => None,
        }
    }
    /// 返回当前光源的相对总通量 Φ；采用项目的 D65/Y 标尺，未标定为瓦特。
    pub fn total_flux(&self) -> f64 {
        match self {
            Self::Parallel(light) => light.total_flux,
            Self::DiskArea(light) => light.total_flux,
        }
    }
    /// 查询解析太阳参数，返回 (指向太阳的单位方向,光束法向辐照度)；圆盘或无效平行光返回 None。
    pub fn sunlight(&self) -> Option<(Vector3<f64>, f64)> {
        self.as_parallel()?.sunlight()
    }
    /// 检查当前光源能否用于光子发射。
    ///
    /// 输入：self 中的参数；输出：有效时 Ok(())，方向、窗口、圆盘几何或通量无效时 Err。
    pub fn validate_emission(&self) -> Result<(), String> {
        match self {
            Self::Parallel(light) => light
                .sunlight()
                .map(|_| ())
                .ok_or("平行光子发射要求有效的向下方向、窗口与通量".into()),
            Self::DiskArea(_) => Ok(()),
        }
    }
    /// 为直接光计算提供可选的解析太阳项。
    ///
    /// 输入：当前光源；输出：平行光的 Some((wi,E_perp))，圆盘为 None；发射参数非法返回 Err。
    pub fn analytic_sun(&self) -> Result<Option<(Vector3<f64>, f64)>, String> {
        self.validate_emission()?;
        Ok(self.sunlight())
    }
    /// 返回每条发射射线需要的随机维数：平行光为位置 2 维，圆盘为位置加方向共 4 维，用于定位采样块。
    pub(crate) fn emission_dimensions(&self) -> usize {
        if self.as_disk().is_some() { 4 } else { 2 }
    }
}

impl From<ParallelLight> for Light {
    /// 把输入的 ParallelLight 包装成 Light::Parallel，供统一光源接口使用，不重新校验参数。
    fn from(value: ParallelLight) -> Self {
        Self::Parallel(value)
    }
}
impl From<DiskAreaLight> for Light {
    /// 把输入的 DiskAreaLight 包装成 Light::DiskArea，供统一光源接口使用，不重新校验参数。
    fn from(value: DiskAreaLight) -> Self {
        Self::DiskArea(value)
    }
}

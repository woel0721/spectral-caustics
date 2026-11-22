use crate::caustics::GroundReceiver;
use crate::geometry::ray::PARALLEL_EPSILON;
use nalgebra::{Vector2, Vector3};
#[derive(Debug, Clone)]
pub struct ParallelLight {
    pub height_m: f64,
    pub direction: Vector3<f64>,
    pub aperture_min_m: Vector2<f64>,
    pub aperture_max_m: Vector2<f64>,
    /// 穿过水平窗口的相对通量 Phi，D65 积分以 Y=1 归一化，无绝对瓦特标定。
    /// 水平面未遮挡辐照度为 Phi/A；垂直于光束的辐照度为 Phi/(A*|direction.z|)。
    pub total_flux: f64,
}

impl ParallelLight {
    /// 把平行光发射参数换算成解析太阳照明，避免相机从光子图读取太阳直射。
    ///
    /// 输入：self 的传播方向 d、水平窗口面积 A 和相对通量 Φ；d 内部归一化。
    ///
    /// 输出：(指向太阳的单位方向 wi,光束法向辐照度 E_perp)，无效参数或非向下光束返回 None。
    ///
    /// 公式：wi=-normalize(d)，E_perp=Φ/(A·wi.z)；未遮挡表面的 E=E_perp·max(n·wi,0)。
    /// 因此水平面 E=Φ/A；窗口只限定发射光子的位置，不裁切解析太阳直射。
    pub fn sunlight(&self) -> Option<(Vector3<f64>, f64)> {
        let length = self.direction.norm();
        let (_, area) = GroundReceiver {
            min_m: self.aperture_min_m,
            max_m: self.aperture_max_m,
        }
        .checked_area()
        .ok()?;
        if !self.height_m.is_finite()
            || !length.is_finite()
            || length == 0.0
            || !self.total_flux.is_finite()
            || self.total_flux < 0.0
        {
            return None;
        }
        let wi = -self.direction / length;
        if wi.z <= PARALLEL_EPSILON {
            return None;
        }
        let irradiance = (self.total_flux / area) / wi.z;
        irradiance.is_finite().then_some((wi, irradiance))
    }
}

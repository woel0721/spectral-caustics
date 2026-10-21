use nalgebra::Vector3;

/// 正向交点的最小距离，单位为米；忽略贴着起点的交点以避免自相交。
pub(crate) const INTERSECTION_EPSILON: f64 = 1e-6;
/// 单位方向在单位表面法线上的投影绝对值不大于此阈值时，视为平行。
pub(crate) const PARALLEL_EPSILON: f64 = 1e-12;

#[derive(Debug, Clone, Copy)]
pub struct Ray {
    /// 光线的起点。
    pub origin: Vector3<f64>,
    /// 光线的单位方向；改变方向时应重新构造 Ray，以同步私有倒数缓存。
    pub direction: Vector3<f64>,
    /// 单位方向各分量的倒数；零分量的倒数为带符号的无穷大。
    /// 此缓存供后续包围盒求交使用，不能要求所有分量都有限。
    pub direction_inv: Vector3<f64>,
    /// 光线携带的时间参数；目前仅保存，不参与距离或交点计算。
    pub time: f64,
    /// 预留的查询距离下限（米）；当前几何 intersect 不读取此字段。
    pub t_min: f64,
    /// 预留的查询距离上限（米）；当前几何 intersect 不读取此字段。
    pub t_max: f64,
}

impl Ray {
    /// 创建带时间参数的光线，自动归一化方向；非法输入返回 None。
    /// 不需要时间变化时，显式传入 time = 0.0。
    /// 预留查询范围初始值为 (0, f64::MAX)，当前几何求交不使用。
    pub fn new(origin: Vector3<f64>, direction: Vector3<f64>, time: f64) -> Option<Self> {
        // 起点、方向和时间都必须是有限值。
        if !origin.iter().all(|v| v.is_finite())
            || !direction.iter().all(|v| v.is_finite())
            || !time.is_finite()
        {
            return None;
        }

        let length = direction.norm();

        // 零向量没有方向；模长计算溢出时也不能继续。
        if !length.is_finite() || length == 0.0 {
            return None;
        }

        let direction = direction / length;
        // IEEE 754 浮点除法：1 / (+0) = +inf，1 / (-0) = -inf。
        // 必须先归一化，再计算倒数，保证缓存与实际保存的方向一致。
        let direction_inv = direction.map(|component| 1.0 / component);

        Some(Self {
            origin,
            direction,
            direction_inv,
            time,
            t_min: 0.0,
            t_max: f64::MAX,
        })
    }

    /// 计算 origin + distance * direction；distance 是距离，与时间字段 time 独立。
    /// 调用者应提供有限且不会使计算溢出的 distance。
    pub fn at(&self, distance: f64) -> Vector3<f64> {
        self.origin + self.direction * distance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_is_normalized() {
        let ray = Ray::new(Vector3::zeros(), Vector3::new(3.0, 4.0, 0.0), 0.0).unwrap();
        let expected = Vector3::new(0.6, 0.8, 0.0);

        assert!((ray.direction - expected).norm() < 1e-12);
        assert!((ray.direction.norm() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn constructor_initializes_time_and_distance_limits() {
        let ray = Ray::new(Vector3::zeros(), Vector3::new(1.0, 0.0, 0.0), 0.0).unwrap();

        assert_eq!(ray.time, 0.0);
        assert_eq!(ray.t_min, 0.0);
        assert_eq!(ray.t_max, f64::MAX);
    }

    #[test]
    fn inverse_direction_matches_normalized_direction() {
        let ray = Ray::new(Vector3::zeros(), Vector3::new(2.0, -3.0, 6.0), 0.0).unwrap();

        for axis in 0..3 {
            assert!((ray.direction[axis] * ray.direction_inv[axis] - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn zero_direction_components_have_signed_infinite_inverses() {
        let ray = Ray::new(Vector3::zeros(), Vector3::new(0.0, -0.0, -5.0), 0.0).unwrap();

        assert_eq!(ray.direction_inv.x, f64::INFINITY);
        assert_eq!(ray.direction_inv.y, f64::NEG_INFINITY);
        assert_eq!(ray.direction_inv.z, -1.0);
        assert!(
            ray.direction_inv
                .iter()
                .all(|component| !component.is_nan())
        );
    }

    #[test]
    fn time_is_independent_of_position() {
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, 2.0),
            Vector3::new(0.0, 0.0, -5.0),
            0.25,
        )
        .unwrap();

        assert_eq!(ray.time, 0.25);
        assert_eq!(ray.at(2.0), Vector3::zeros());
    }

    #[test]
    fn non_finite_time_is_rejected() {
        for time in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Ray::new(Vector3::zeros(), Vector3::new(0.0, 0.0, -1.0), time).is_none());
        }
    }

    #[test]
    fn at_returns_expected_position() {
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, 2.0),
            Vector3::new(0.0, 0.0, -5.0),
            0.0,
        )
        .unwrap();

        assert_eq!(ray.at(0.0), ray.origin);
        assert_eq!(ray.at(1.0), Vector3::new(0.0, 0.0, 1.0));
        assert_eq!(ray.at(2.0), Vector3::zeros());
    }
}

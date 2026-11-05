use nalgebra::Vector3;

/// 正向交点的最小距离，单位为米；忽略贴着起点的交点以避免自相交。
pub(crate) const INTERSECTION_EPSILON: f64 = 1e-6;
/// 单位方向在单位表面法线上的投影绝对值不大于此阈值时，视为平行。
pub(crate) const PARALLEL_EPSILON: f64 = 1e-12;

#[derive(Debug, Clone, Copy)]
pub struct Ray {
    pub origin: Vector3<f64>,
    pub direction: Vector3<f64>,
}

impl Ray {
    /// 把世界空间起点和方向打包成光线，供所有几何求交使用。
    ///
    /// 输入：起点 origin（米）和非零方向 direction；方向不必预先归一化。
    ///
    /// 输出：保存 d=direction/‖direction‖ 及逐分量倒数 1/d 的 Ray；非法值或长度溢出返回 None。
    /// 预留距离范围为 (0,f64::MAX)，当前几何求交不读取它；零方向分量的倒数允许为无穷大。
    pub fn new(origin: Vector3<f64>, direction: Vector3<f64>) -> Option<Self> {
        // 起点和方向都必须是有限值。
        if !origin.iter().all(|v| v.is_finite()) || !direction.iter().all(|v| v.is_finite()) {
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

        Some(Self { origin, direction })
    }

    /// 求光线前进指定距离后的位置。
    ///
    /// 输入：距离 t（米）；调用方负责保证 t 有限且运算不溢出。
    ///
    /// 输出：世界坐标 P(t)=origin+t·direction；负 t 表示起点后方。
    pub fn at(&self, distance: f64) -> Vector3<f64> {
        self.origin + self.direction * distance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_is_normalized() {
        let ray = Ray::new(Vector3::zeros(), Vector3::new(3.0, 4.0, 0.0)).unwrap();
        let expected = Vector3::new(0.6, 0.8, 0.0);

        assert!((ray.direction - expected).norm() < 1e-12);
        assert!((ray.direction.norm() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn invalid_origins_and_directions_are_rejected() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for axis in 0..3 {
                let mut invalid = Vector3::zeros();
                invalid[axis] = value;
                assert!(Ray::new(invalid, Vector3::z()).is_none());
                assert!(Ray::new(Vector3::zeros(), invalid).is_none());
            }
        }
        for direction in [
            Vector3::zeros(),
            Vector3::repeat(f64::MAX),
            Vector3::repeat(f64::from_bits(1)),
        ] {
            assert!(Ray::new(Vector3::zeros(), direction).is_none());
        }
    }

    #[test]
    fn at_returns_expected_position() {
        let ray = Ray::new(Vector3::new(0.0, 0.0, 2.0), Vector3::new(0.0, 0.0, -5.0)).unwrap();

        assert_eq!(ray.at(0.0), ray.origin);
        assert_eq!(ray.at(1.0), Vector3::new(0.0, 0.0, 1.0));
        assert_eq!(ray.at(2.0), Vector3::zeros());
    }
}

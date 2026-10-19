use nalgebra::Vector3;

/// 正向交点的最小距离，单位为米；忽略贴着起点的交点以避免自相交。
pub(crate) const INTERSECTION_EPSILON: f64 = 1e-6;
/// 单位方向在单位表面法线上的投影绝对值不大于此阈值时，视为平行。
pub(crate) const PARALLEL_EPSILON: f64 = 1e-12;

#[derive(Debug, Clone, Copy)]
pub struct Ray {
    /// 光线的起点。
    pub origin: Vector3<f64>,
    /// 光线的单位方向；手动修改时需同步更新 direction_inv。
    pub direction: Vector3<f64>,
    /// 单位方向各分量的倒数；零分量的倒数为带符号的无穷大。
    /// 此缓存供后续包围盒求交使用，不能要求所有分量都有限。
    pub direction_inv: Vector3<f64>,
    /// 光线携带的时间参数；目前仅保存，不参与距离或交点计算。
    pub t: f64,
    /// 求交距离下限，单位为米；只接受距离大于此值的交点。
    pub t_min: f64,
    /// 求交距离上限，单位为米；只接受距离小于此值的交点。
    pub t_max: f64,
}

impl Ray {
    /// 创建带时间参数的光线，自动归一化方向；非法输入返回 None。
    /// 不需要时间变化时，显式传入 time = 0.0。
    /// 默认距离区间为 (0, f64::MAX)。
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
            t: time,
            t_min: 0.0,
            t_max: f64::MAX,
        })
    }

    /// 计算 origin + distance * direction；distance 是距离，与时间字段 t 独立。
    /// 调用者应提供有限且不会使计算溢出的 distance。
    pub fn at(&self, distance: f64) -> Vector3<f64> {
        self.origin + self.direction * distance
    }

    /// 与无限地面 z = 0 求交，返回距离起点的正向距离 t（米）。
    /// 地面两侧均可命中；平行、距离过近、身后或非有限的交点返回 None。
    /// 同时要求交点满足 t_min < t < t_max；允许将 t_max 改为正无穷大。
    /// 方向应保持 Ray::new 建立的单位长度约定。
    pub fn intersect_ground(&self) -> Option<f64> {
        // 字段目前公开，因此求交时也检查数据，防止外部修改后引入 NaN。
        if !self.origin.iter().all(|v| v.is_finite())
            || !self.direction.iter().all(|v| v.is_finite())
        {
            return None;
        }

        // t_min 必须为非负有限值；t_max 可以是正无穷大，但不能为 NaN。
        if !self.t_min.is_finite()
            || self.t_min < 0.0
            || self.t_max.is_nan()
            || self.t_max <= self.t_min
        {
            return None;
        }

        // 分母为零或过小时，视为平行，不执行除法。
        if self.direction.z.abs() <= PARALLEL_EPSILON {
            return None;
        }

        // p(t).z = origin.z + t * direction.z = 0。
        let t = -self.origin.z / self.direction.z;

        if !t.is_finite() || t <= self.t_min.max(INTERSECTION_EPSILON) || t >= self.t_max {
            return None;
        }

        // t 有限仍可能在计算 x、y 坐标时溢出。
        if !self.at(t).iter().all(|v| v.is_finite()) {
            return None;
        }

        Some(t)
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

        assert_eq!(ray.t, 0.0);
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
    fn time_is_independent_of_position_and_ground_distance() {
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, 2.0),
            Vector3::new(0.0, 0.0, -5.0),
            0.25,
        )
        .unwrap();

        assert_eq!(ray.t, 0.25);
        assert_eq!(ray.at(2.0), Vector3::zeros());
        assert_eq!(ray.intersect_ground(), Some(2.0));
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

    #[test]
    fn invalid_directions_are_rejected() {
        let invalid_directions = [
            Vector3::zeros(),
            Vector3::new(f64::NAN, 0.0, 0.0),
            Vector3::new(0.0, f64::INFINITY, 0.0),
        ];

        for direction in invalid_directions {
            assert!(Ray::new(Vector3::zeros(), direction, 0.0).is_none());
        }
    }

    #[test]
    fn downward_ray_hits_ground_at_known_distance() {
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, 2.0),
            Vector3::new(0.0, 0.0, -5.0),
            0.0,
        )
        .unwrap();
        let t = ray.intersect_ground().unwrap();

        assert_eq!(t, 2.0);
        assert_eq!(ray.at(t), Vector3::zeros());
    }

    #[test]
    fn oblique_ray_hits_ground_at_expected_position() {
        let ray = Ray::new(
            Vector3::new(1.0, 2.0, 2.0),
            Vector3::new(1.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();
        let t = ray.intersect_ground().unwrap();

        assert!((t - 2.0 * 2.0_f64.sqrt()).abs() < 1e-12);
        assert!((ray.at(t) - Vector3::new(3.0, 2.0, 0.0)).norm() < 1e-12);
    }

    #[test]
    fn parallel_and_nearly_parallel_rays_miss_ground() {
        for z in [0.0, -PARALLEL_EPSILON / 2.0, PARALLEL_EPSILON / 2.0] {
            let ray =
                Ray::new(Vector3::new(0.0, 0.0, 2.0), Vector3::new(1.0, 0.0, z), 0.0).unwrap();

            assert!(ray.intersect_ground().is_none());
        }
    }

    #[test]
    fn ground_intersection_behind_origin_is_rejected() {
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, 2.0),
            Vector3::new(0.0, 0.0, 1.0),
            0.0,
        )
        .unwrap();

        assert!(ray.intersect_ground().is_none());
    }

    #[test]
    fn ground_intersection_respects_distance_limits() {
        let mut ray = Ray::new(
            Vector3::new(0.0, 0.0, 2.0),
            Vector3::new(0.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();

        ray.t_min = 1.0;
        ray.t_max = 3.0;
        assert_eq!(ray.intersect_ground(), Some(2.0));

        for (t_min, t_max) in [(2.0, 3.0), (2.5, 3.0), (0.0, 2.0), (0.0, 1.5)] {
            ray.t_min = t_min;
            ray.t_max = t_max;
            assert!(ray.intersect_ground().is_none());
        }

        ray.t_min = 0.0;
        ray.t_max = f64::INFINITY;
        assert_eq!(ray.intersect_ground(), Some(2.0));
    }

    #[test]
    fn invalid_distance_limits_are_rejected() {
        let mut ray = Ray::new(
            Vector3::new(0.0, 0.0, 2.0),
            Vector3::new(0.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();

        for (t_min, t_max) in [
            (f64::NAN, 3.0),
            (f64::INFINITY, f64::INFINITY),
            (-1.0, 3.0),
            (3.0, 3.0),
            (3.0, 1.0),
            (0.0, f64::NAN),
            (0.0, f64::NEG_INFINITY),
        ] {
            ray.t_min = t_min;
            ray.t_max = t_max;
            assert!(ray.intersect_ground().is_none());
        }
    }

    #[test]
    fn ground_intersection_at_or_near_origin_is_rejected() {
        for z in [0.0, INTERSECTION_EPSILON / 2.0, INTERSECTION_EPSILON] {
            let ray =
                Ray::new(Vector3::new(0.0, 0.0, z), Vector3::new(0.0, 0.0, -1.0), 0.0).unwrap();

            assert!(ray.intersect_ground().is_none());
        }

        let ray = Ray::new(
            Vector3::new(0.0, 0.0, 2.0 * INTERSECTION_EPSILON),
            Vector3::new(0.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();

        assert_eq!(ray.intersect_ground(), Some(2.0 * INTERSECTION_EPSILON));
    }

    #[test]
    fn ray_below_ground_can_hit_upward() {
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, -1.0),
            Vector3::new(0.0, 0.0, 1.0),
            0.0,
        )
        .unwrap();
        let t = ray.intersect_ground().unwrap();

        assert_eq!(t, 1.0);
        assert_eq!(ray.at(t), Vector3::zeros());
    }

    #[test]
    fn non_finite_ray_fields_are_rejected_by_ground_intersection() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for component in 0..3 {
                let mut ray = Ray::new(
                    Vector3::new(0.0, 0.0, 2.0),
                    Vector3::new(0.0, 0.0, -1.0),
                    0.0,
                )
                .unwrap();

                ray.origin[component] = value;
                assert!(ray.intersect_ground().is_none());

                ray.origin[component] = 0.0;
                ray.direction[component] = value;
                assert!(ray.intersect_ground().is_none());
            }
        }
    }

    #[test]
    fn overflowing_ground_distance_is_rejected() {
        let ray = Ray::new(
            Vector3::new(0.0, 0.0, f64::MAX),
            Vector3::new(1.0, 0.0, -1e-10),
            0.0,
        )
        .unwrap();

        assert!(ray.intersect_ground().is_none());
    }

    #[test]
    fn overflowing_ground_position_is_rejected() {
        let ray = Ray::new(
            Vector3::new(f64::MAX, 0.0, f64::MAX / 2.0),
            Vector3::new(1.0, 0.0, -1.0),
            0.0,
        )
        .unwrap();

        assert!(ray.intersect_ground().is_none());
    }
}

//! 可求交物体的统一接口；具体的求交算法由各物体实现。

use crate::{hit::Hit, ray::Ray};

/// 实现此 trait 的类型可以接受光线查询，并返回最近的有效交点。
pub trait Hittable {
    /// 在 ray.t_min、ray.t_max 限定的距离范围内查找最近交点。
    ///
    /// - ray 的方向应为单位向量，此时 t 的单位是米。
    /// - ray.t_min 应为非负有限值，实现时还需用 epsilon 排除自相交。
    /// - ray.t_max 应大于 ray.t_min，可使用 f64::INFINITY 表示没有距离上限。
    /// - 没有命中、区间非法或计算结果非有限时，返回 None。
    /// - 求交算法负责填写 Hit，并通过 Some(hit) 返回命中记录。
    ///
    /// 此处只定义接口；三角形、网格等类型各自实现具体算法。
    fn intersect(&self, ray: &Ray) -> Option<Hit>;
}

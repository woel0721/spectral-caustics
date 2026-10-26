//! 可求交物体的统一接口；具体的求交算法由各物体实现。

use crate::{hit::Hit, ray::Ray};

/// 实现此 trait 的类型可以接受光线查询，并允许并行追踪时共享只读引用。
pub trait Hittable: Send + Sync {
    /// 查找光线正向的最近几何交点。
    ///
    /// - ray 的方向应为单位向量，此时 t 的单位是米。
    /// - 实现时使用 epsilon 排除自相交，仅接受有限的正向距离。
    /// - 当前几何求交不读取 ray.t_min、ray.t_max，两个字段保留待后续查询使用。
    /// - 没有命中或计算结果非有限时，返回 None。
    /// - 求交算法负责填写 Hit，并通过 Some(hit) 返回命中记录。
    ///
    /// 此处只定义接口；三角形、网格等类型各自实现具体算法。
    fn intersect(&self, ray: &Ray) -> Option<Hit>;
}

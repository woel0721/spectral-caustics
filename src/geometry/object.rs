//! 可求交物体的统一接口；具体的求交算法由各物体实现。

use super::hit::GeometryHit as Hit;
use super::ray::Ray;

/// 实现此 trait 的类型可以接受光线查询，并允许并行追踪时共享只读引用。
pub trait Hittable: Send + Sync {
    /// 查询一条光线最先碰到物体的哪个位置，供相机、阴影和光子追踪共用。
    ///
    /// 输入：世界空间 Ray，方向应为单位向量，因此距离 t 的单位为米。
    ///
    /// 输出：最近的有限正向交点 Some(Hit)；未命中或数值无效时返回 None。
    /// 求交须排除 t≤INTERSECTION_EPSILON 的自交。
    /// 几何只填写坐标、法线和距离，物体/材质编号由 RenderJob::intersect 补充。
    fn intersect(&self, ray: &Ray) -> Option<Hit>;
}

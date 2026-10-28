//! 求交结果的数据记录；各字段由具体求交算法填写。

use nalgebra::Vector3;

#[derive(Debug, Clone, Copy)]
pub struct Hit {
    /// 交点的世界坐标。
    pub coords: Vector3<f64>,
    /// 由求交算法填写的单位几何法线，封闭物体约定朝外。
    pub normal: Vector3<f64>,
    /// 光线起点到交点的距离（米）。
    pub distance: f64,
    /// 场景中的物体编号；None 对应 C++ 示例中 obj = nullptr。
    pub object_id: Option<usize>,
    /// 材质编号；None 对应 C++ 示例中 m = nullptr。
    pub material_id: Option<usize>,
}

impl Default for Hit {
    /// 仅提供记录的字段初始值；是否命中由 Option<Hit> 表示。
    fn default() -> Self {
        Self {
            coords: Vector3::zeros(),
            normal: Vector3::zeros(),
            distance: f64::MAX,
            object_id: None,
            material_id: None,
        }
    }
}

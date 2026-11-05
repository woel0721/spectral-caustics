use super::Scene;
use nalgebra::Vector3;
impl Scene {
    /// 读取一条逃逸光线看到的背景，供路径累计背景辐亮度。
    ///
    /// 输入：世界方向；输出：环境的线性 RGB，未配置环境时返回常量背景；颜色或方向查询无效时 None。
    /// 常量背景只用于背景查询，不自动成为表面的环境照明。
    pub fn background_radiance(&self, direction: Vector3<f64>) -> Option<Vector3<f64>> {
        match &self.environment {
            Some(environment) => environment.radiance(direction),
            None => self
                .background_color
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)
                .then_some(self.background_color),
        }
    }
}

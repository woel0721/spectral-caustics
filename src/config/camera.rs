//! 针孔相机的 JSON 描述；坐标使用世界空间，长度单位为米。

use crate::camera::Camera;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraConfig {
    pub position: [f64; 3],
    pub target: [f64; 3],
    pub up: [f64; 3],
    /// 垂直视场角，单位为度。
    pub fov_y_degrees: f64,
}

impl Default for CameraConfig {
    /// 返回内置相机配置：位置 [2.5,-4.5,2.6] m、目标 [0,0,0.4] m、向上 +Z、垂直视场角 35°。
    fn default() -> Self {
        Self {
            position: [2.5, -4.5, 2.6],
            target: [0.0, 0.0, 0.4],
            up: [0.0, 0.0, 1.0],
            fov_y_degrees: 35.0,
        }
    }
}

impl CameraConfig {
    /// 检查 self 是否能构建有效针孔相机，供配置加载时提前报错。
    ///
    /// 输出：可构建时 Ok(())；视场角、位置或坐标系退化时 Err，临时相机不保留。
    pub fn validate(&self) -> Result<(), String> {
        Camera::from_config(self).map(|_| ())
    }
}

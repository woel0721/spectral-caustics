//! 针孔相机的 JSON 描述；坐标使用世界空间，长度单位为米。

use crate::camera::Camera;
use serde::{Deserialize, Serialize};

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
    fn default() -> Self {
        Self {
            position: [2.5, -4.5, 2.6],
            target: [0.0, 0.0, 0.9],
            up: [0.0, 0.0, 1.0],
            fov_y_degrees: 35.0,
        }
    }
}

impl CameraConfig {
    pub fn validate(&self) -> Result<(), String> {
        Camera::from_config(self).map(|_| ())
    }
}

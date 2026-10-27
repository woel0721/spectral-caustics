//! 针孔透视相机：从相机位置穿过像素中心，生成用于场景求交的主光线。

use crate::{config::cameraconfig::CameraConfig, geometry::ray::Ray};
use nalgebra::Vector3;

#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    /// 相机位置和观察目标，使用世界坐标，单位为米。
    pub position: Vector3<f64>,
    pub target: Vector3<f64>,
    /// 指定画面朝上的参考方向，不必为单位向量，但不能平行于观察方向。
    pub up: Vector3<f64>,
    /// 垂直视场角，单位为度，必须位于 (0, 180)。
    pub fov_y_degrees: f64,
}

impl Camera {
    pub fn from_config(config: &CameraConfig) -> Result<Self, String> {
        let camera = Self {
            position: Vector3::from(config.position),
            target: Vector3::from(config.target),
            up: Vector3::from(config.up),
            fov_y_degrees: config.fov_y_degrees,
        };
        camera.frame().ok_or(
            "scene.camera 需要有限坐标、不同的 position/target、非零且不平行于观察方向的 up，以及 (0, 180) 度的 fov_y_degrees",
        )?;
        Ok(camera)
    }

    /// (x, y) 是从零开始的像素下标；图片原点在左上，x 向右、y 向下。
    /// 宽高由 RenderSetting 提供，画面宽高比为 width / height。
    /// 光线起点为 position，穿过像素中心，方向由 Ray::new 归一化，time 为 0。
    /// 越界、零尺寸或非法相机参数返回 None。
    pub fn ray_for_pixel(&self, x: usize, y: usize, width: usize, height: usize) -> Option<Ray> {
        if width == 0 || height == 0 || x >= width || y >= height {
            return None;
        }
        let [forward, right, camera_up] = self.frame()?;

        // 将虚拟成像平面放在相机前方 1 米处，半高由垂直 FOV 决定。
        let half_height = (self.fov_y_degrees.to_radians() * 0.5).tan();
        let half_width = half_height * width as f64 / height as f64;
        // +0.5 取像素中心；图片 y 向下，因此成像平面的竖直坐标要翻转。
        let image_x = (2.0 * (x as f64 + 0.5) / width as f64 - 1.0) * half_width;
        let image_y = (1.0 - 2.0 * (y as f64 + 0.5) / height as f64) * half_height;
        // 平面上的点为 position + forward + image_x * right + image_y * camera_up。
        let direction = forward + image_x * right + image_y * camera_up;
        Ray::new(self.position, direction, 0.0)
    }

    /// 建立正交相机坐标系；不能直接将参考 up 当作画面的竖直轴。
    fn frame(&self) -> Option<[Vector3<f64>; 3]> {
        if !self.fov_y_degrees.is_finite()
            || self.fov_y_degrees <= 0.0
            || self.fov_y_degrees >= 180.0
        {
            return None;
        }
        let forward = Ray::new(self.position, self.target - self.position, 0.0)?.direction;
        let up = Ray::new(Vector3::zeros(), self.up, 0.0)?.direction;
        let right = forward.cross(&up);
        let length = right.norm();
        // 单位方向的叉积长度为夹角的正弦；近平行时也拒绝，避免不稳定的画面方向。
        if !length.is_finite() || length <= 1e-12 {
            return None;
        }
        let right = right / length;
        let camera_up = right.cross(&forward);
        Some([forward, right, camera_up])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front_camera() -> Camera {
        Camera {
            position: Vector3::new(0.0, 0.0, 3.0),
            target: Vector3::zeros(),
            up: Vector3::y(),
            fov_y_degrees: 90.0,
        }
    }

    #[test]
    fn center_ray_starts_at_camera_and_looks_at_target() {
        let camera = front_camera();
        for size in [1, 3, 5] {
            let ray = camera
                .ray_for_pixel(size / 2, size / 2, size, size)
                .unwrap();
            assert_eq!(ray.origin, camera.position);
            assert!((ray.direction + Vector3::z()).norm() < 1e-12);
            assert!((ray.direction.norm() - 1.0).abs() < 1e-12);
            assert_eq!(ray.time, 0.0);
        }
    }

    #[test]
    fn four_pixel_centers_have_correct_plane_positions_and_orientation() {
        let camera = front_camera();
        for (x, y, plane_x, plane_y) in [
            (0, 0, -0.5, 0.5),
            (1, 0, 0.5, 0.5),
            (0, 1, -0.5, -0.5),
            (1, 1, 0.5, -0.5),
        ] {
            let ray = camera.ray_for_pixel(x, y, 2, 2).unwrap();
            let plane_point = ray.at(-1.0 / ray.direction.z);
            assert!((plane_point - Vector3::new(plane_x, plane_y, 2.0)).norm() < 1e-12);
            assert!((ray.direction.norm() - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn vertical_fov_and_non_square_aspect_set_the_plane_size() {
        let mut camera = front_camera();
        let wide = camera.ray_for_pixel(0, 0, 4, 2).unwrap();
        let expected = Vector3::new(-1.5, 0.5, -1.0).normalize();
        assert!((wide.direction - expected).norm() < 1e-12);
        camera.fov_y_degrees = 60.0;
        let top = camera.ray_for_pixel(1, 0, 3, 3).unwrap();
        let expected =
            Vector3::new(0.0, (30.0_f64).to_radians().tan() * 2.0 / 3.0, -1.0).normalize();
        assert!((top.direction - expected).norm() < 1e-12);
    }

    #[test]
    fn arbitrary_view_and_non_orthogonal_up_form_an_orthogonal_frame() {
        let camera = Camera {
            position: Vector3::new(2.0, 3.0, 4.0),
            target: Vector3::new(3.0, 3.0, 4.0),
            up: Vector3::new(1.0, 0.0, 3.0),
            fov_y_degrees: 90.0,
        };
        let center = camera.ray_for_pixel(1, 1, 3, 3).unwrap();
        assert!((center.direction - Vector3::x()).norm() < 1e-12);
        let top_right = camera.ray_for_pixel(1, 0, 2, 2).unwrap();
        let expected = Vector3::new(1.0, -0.5, 0.5).normalize();
        assert!((top_right.direction - expected).norm() < 1e-12);
    }

    #[test]
    fn invalid_sizes_and_pixel_indices_are_rejected() {
        let camera = front_camera();
        for (x, y, width, height) in [
            (0, 0, 0, 1),
            (0, 0, 1, 0),
            (2, 0, 2, 2),
            (0, 2, 2, 2),
            (usize::MAX, usize::MAX, 2, 2),
        ] {
            assert!(camera.ray_for_pixel(x, y, width, height).is_none());
        }
    }

    #[test]
    fn invalid_fov_and_degenerate_or_non_finite_frames_are_rejected() {
        let original = front_camera();
        for fov_y_degrees in [0.0, -1.0, 180.0, 181.0, f64::NAN, f64::INFINITY] {
            let camera = Camera {
                fov_y_degrees,
                ..original.clone()
            };
            assert!(camera.ray_for_pixel(0, 0, 1, 1).is_none());
        }
        for (position, target, up) in [
            (original.position, original.position, original.up),
            (original.position, original.target, Vector3::zeros()),
            (original.position, original.target, Vector3::z()),
            (original.position, original.target, -Vector3::z()),
            (
                original.position,
                original.target,
                Vector3::new(1e-14, 0.0, 1.0),
            ),
            (Vector3::repeat(f64::NAN), original.target, original.up),
            (
                original.position,
                Vector3::repeat(f64::INFINITY),
                original.up,
            ),
            (
                original.position,
                original.target,
                Vector3::repeat(f64::NAN),
            ),
            (
                Vector3::repeat(f64::MAX),
                Vector3::repeat(-f64::MAX),
                original.up,
            ),
        ] {
            let camera = Camera {
                position,
                target,
                up,
                ..original.clone()
            };
            assert!(camera.ray_for_pixel(0, 0, 1, 1).is_none());
        }
    }
}

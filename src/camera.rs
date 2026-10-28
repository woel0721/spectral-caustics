//! 针孔透视相机：从相机位置穿过像素中心，生成用于场景求交的主光线。

use crate::{config::cameraconfig::CameraConfig, geometry::ray::Ray};
use nalgebra::Vector3;

/// 已校验的运行时相机，参数只读，预计算结果由所有像素复用。
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    /// 相机位置和观察目标，使用世界坐标，单位为米。
    position: Vector3<f64>,
    target: Vector3<f64>,
    /// 指定画面朝上的参考方向，不必为单位向量，但不能平行于观察方向。
    up: Vector3<f64>,
    /// 垂直视场角，单位为度，必须位于 (0, 180)。
    fov_y_degrees: f64,
    /// 相机的正交坐标系只在构建时计算，所有像素共用。
    forward: Vector3<f64>,
    right: Vector3<f64>,
    camera_up: Vector3<f64>,
    /// 相机前方 1 米处成像平面的半高，缓存 tan(fov_y / 2)。
    half_height: f64,
}

impl Camera {
    /// 校验配置并预计算相机坐标系和投影尺度；改变配置后需重新构建相机。
    pub fn from_config(config: &CameraConfig) -> Result<Self, String> {
        let position = Vector3::from(config.position);
        let target = Vector3::from(config.target);
        let up = Vector3::from(config.up);
        let fov_y_degrees = config.fov_y_degrees;
        if !fov_y_degrees.is_finite() || fov_y_degrees <= 0.0 || fov_y_degrees >= 180.0 {
            return Err("scene.camera.fov_y_degrees 必须有限且位于 (0, 180) 度".into());
        }
        let forward = Ray::new(position, target - position)
            .ok_or("scene.camera 的 position/target 必须有限、不同，且方向长度不能溢出")?
            .direction;
        let normalized_up = Ray::new(Vector3::zeros(), up)
            .ok_or("scene.camera.up 必须有限、非零，且长度不能溢出")?
            .direction;
        let right = forward.cross(&normalized_up);
        let length = right.norm();
        // 单位方向的叉积长度为夹角的正弦；近平行时也拒绝，避免不稳定的画面方向。
        if !length.is_finite() || length <= 1e-12 {
            return Err("scene.camera.up 不能与观察方向平行或近平行".into());
        }
        let right = right / length;
        let camera_up = right.cross(&forward);
        let half_height = (fov_y_degrees.to_radians() * 0.5).tan();
        if !half_height.is_finite() || half_height <= 0.0 {
            return Err("scene.camera.fov_y_degrees 无法形成有限、正值的成像平面半高".into());
        }
        Ok(Self {
            position,
            target,
            up,
            fov_y_degrees,
            forward,
            right,
            camera_up,
            half_height,
        })
    }

    pub fn position(&self) -> Vector3<f64> {
        self.position
    }

    pub fn target(&self) -> Vector3<f64> {
        self.target
    }

    pub fn up(&self) -> Vector3<f64> {
        self.up
    }

    pub fn fov_y_degrees(&self) -> f64 {
        self.fov_y_degrees
    }

    /// (x, y) 是从零开始的像素下标；图片原点在左上，x 向右、y 向下。
    /// 宽高由 RenderSetting 提供，画面宽高比为 width / height。
    /// 光线起点为 position，穿过像素中心，方向由 Ray::new 归一化。
    /// 使用构建时缓存的坐标系与半高；越界或零尺寸返回 None。
    pub fn ray_for_pixel(&self, x: usize, y: usize, width: usize, height: usize) -> Option<Ray> {
        if width == 0 || height == 0 || x >= width || y >= height {
            return None;
        }
        // 半高由相机构建时计算，半宽根据本次输出图片的宽高比确定。
        let half_width = self.half_height * width as f64 / height as f64;
        // +0.5 取像素中心；图片 y 向下，因此成像平面的竖直坐标要翻转。
        let image_x = (2.0 * (x as f64 + 0.5) / width as f64 - 1.0) * half_width;
        let image_y = (1.0 - 2.0 * (y as f64 + 0.5) / height as f64) * self.half_height;
        // 平面上的点为 position + forward + image_x * right + image_y * camera_up。
        let direction = self.forward + image_x * self.right + image_y * self.camera_up;
        Ray::new(self.position, direction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front_config() -> CameraConfig {
        CameraConfig {
            position: [0.0, 0.0, 3.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 90.0,
        }
    }

    fn front_camera() -> Camera {
        Camera::from_config(&front_config()).unwrap()
    }

    #[test]
    fn center_ray_starts_at_camera_and_looks_at_target() {
        let camera = front_camera();
        for size in [1, 3, 5] {
            let ray = camera
                .ray_for_pixel(size / 2, size / 2, size, size)
                .unwrap();
            assert_eq!(ray.origin, camera.position());
            assert!((ray.direction + Vector3::z()).norm() < 1e-12);
            assert!((ray.direction.norm() - 1.0).abs() < 1e-12);
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
        let camera = front_camera();
        let wide = camera.ray_for_pixel(0, 0, 4, 2).unwrap();
        let expected = Vector3::new(-1.5, 0.5, -1.0).normalize();
        assert!((wide.direction - expected).norm() < 1e-12);
        let camera = Camera::from_config(&CameraConfig {
            fov_y_degrees: 60.0,
            ..front_config()
        })
        .unwrap();
        let top = camera.ray_for_pixel(1, 0, 3, 3).unwrap();
        let expected =
            Vector3::new(0.0, (30.0_f64).to_radians().tan() * 2.0 / 3.0, -1.0).normalize();
        assert!((top.direction - expected).norm() < 1e-12);
    }

    #[test]
    fn arbitrary_view_and_non_orthogonal_up_form_an_orthogonal_frame() {
        let camera = Camera::from_config(&CameraConfig {
            position: [2.0, 3.0, 4.0],
            target: [3.0, 3.0, 4.0],
            up: [1.0, 0.0, 3.0],
            fov_y_degrees: 90.0,
        })
        .unwrap();
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
        let original = front_config();
        for fov_y_degrees in [
            0.0,
            -1.0,
            180.0,
            181.0,
            f64::NAN,
            f64::INFINITY,
            f64::from_bits(1),
        ] {
            let config = CameraConfig {
                fov_y_degrees,
                ..original.clone()
            };
            assert!(Camera::from_config(&config).is_err());
        }
        for (position, target, up) in [
            (original.position, original.position, original.up),
            (original.position, original.target, [0.0; 3]),
            (original.position, original.target, [0.0, 0.0, 1.0]),
            (original.position, original.target, [0.0, 0.0, -1.0]),
            (original.position, original.target, [1e-14, 0.0, 1.0]),
            ([f64::NAN; 3], original.target, original.up),
            (original.position, [f64::INFINITY; 3], original.up),
            (original.position, original.target, [f64::NAN; 3]),
            ([f64::MAX; 3], [-f64::MAX; 3], original.up),
        ] {
            let config = CameraConfig {
                position,
                target,
                up,
                ..original.clone()
            };
            assert!(Camera::from_config(&config).is_err());
        }
    }

    #[test]
    fn changing_source_config_requires_rebuilding_the_camera() {
        let mut config = front_config();
        let camera = Camera::from_config(&config).unwrap();
        let rays: Vec<_> = (0..2)
            .flat_map(|y| (0..4).map(move |x| (x, y)))
            .map(|(x, y)| camera.ray_for_pixel(x, y, 4, 2).unwrap())
            .collect();

        config.position = [2.0, 3.0, 4.0];
        config.target = [3.0, 3.0, 4.0];
        config.up = [1.0, 0.0, 3.0];
        config.fov_y_degrees = 60.0;
        for (i, expected) in rays.iter().enumerate() {
            let ray = camera.ray_for_pixel(i % 4, i / 4, 4, 2).unwrap();
            assert_eq!(ray.origin, expected.origin);
            assert_eq!(ray.direction, expected.direction);
        }
        let rebuilt = Camera::from_config(&config).unwrap();
        assert_eq!(rebuilt.position(), Vector3::from(config.position));
        assert_eq!(rebuilt.target(), Vector3::from(config.target));
        assert_eq!(rebuilt.up(), Vector3::from(config.up));
        assert_eq!(rebuilt.fov_y_degrees(), config.fov_y_degrees);
        let center = rebuilt.ray_for_pixel(1, 1, 3, 3).unwrap();
        assert_eq!(center.origin, Vector3::new(2.0, 3.0, 4.0));
        assert!((center.direction - Vector3::x()).norm() < 1e-12);
        let top_right = rebuilt.ray_for_pixel(1, 0, 2, 2).unwrap();
        let half_height = (30.0_f64).to_radians().tan();
        let expected = Vector3::new(1.0, -0.5 * half_height, 0.5 * half_height).normalize();
        assert!((top_right.direction - expected).norm() < 1e-12);
    }
}

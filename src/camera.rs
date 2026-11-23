//! 针孔透视相机：从相机位置穿过像素中心或指定的像素内采样点。

use crate::config::camera::CameraConfig;
use crate::geometry::ray::Ray;
use nalgebra::Vector2;
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
    /// 把相机配置转换为可直接生成光线的相机，提前算好所有像素共用的坐标系。
    ///
    /// 输入：世界坐标中的位置 P、目标 T、向上参考 U，以及垂直视场角 θ（度）。
    ///
    /// 输出：校验后的 Camera；位置重合、方向退化或视场角无效时返回 Err。
    ///
    /// 公式：f = normalize(T-P)，r = normalize(f×U)，u = r×f；成像平面半高 h = tan(θ/2)。
    /// 配置变化后需要重新构建，相机不会自动读取原配置的新值。
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

    /// 读取相机所在的世界坐标 P（米），返回坐标副本。
    pub fn position(&self) -> Vector3<f64> {
        self.position
    }

    /// 读取相机正在看的目标位置 T（世界坐标，米），返回坐标副本。
    pub fn target(&self) -> Vector3<f64> {
        self.target
    }

    /// 返回配置中的向上参考向量；它不一定是单位向量，也不一定与观察方向垂直。
    pub fn up(&self) -> Vector3<f64> {
        self.up
    }

    /// 返回相机的垂直视场角，单位为度。
    pub fn fov_y_degrees(&self) -> f64 {
        self.fov_y_degrees
    }

    /// 生成从相机出发、穿过指定像素中心的光线，用于无像素内抖动的查询。
    ///
    /// 输入：零起始像素下标 (x,y) 与图像宽高；图像原点在左上角。
    ///
    /// 输出：方向已归一化的 Ray；零尺寸或坐标越界时返回 None。
    /// 等价于 ray_for_pixel_sample(..., offset=(0.5,0.5))。
    pub fn ray_for_pixel(&self, x: usize, y: usize, width: usize, height: usize) -> Option<Ray> {
        self.ray_for_pixel_sample(x, y, width, height, Vector2::repeat(0.5))
    }

    /// 把像素内的一个采样位置转换为世界空间主光线，用于抗锯齿采样。
    ///
    /// 输入：像素下标 (x,y)、宽高 W/H、两轴均在 [0,1) 的偏移 offset。
    ///
    /// 输出：起点为相机位置 P 的单位方向 Ray；尺寸、下标或偏移无效时返回 None。
    ///
    /// 公式：h=tan(fov_y/2)，w=hW/H；a=(2(x+offset.x)/W-1)w，b=(1-2(y+offset.y)/H)h。
    /// 光线方向 d=normalize(forward+a·right+b·camera_up)；b 的符号使图像 y 轴向下。
    pub fn ray_for_pixel_sample(
        &self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        offset: Vector2<f64>,
    ) -> Option<Ray> {
        if width == 0
            || height == 0
            || x >= width
            || y >= height
            || offset.iter().any(|value| !(0.0..1.0).contains(value))
        {
            return None;
        }
        // 半高由相机构建时计算，半宽根据本次输出图片的宽高比确定。
        let half_width = self.half_height * width as f64 / height as f64;
        // 图片 y 向下，因此成像平面的竖直坐标要翻转。
        let image_x = (2.0 * (x as f64 + offset.x) / width as f64 - 1.0) * half_width;
        let image_y = (1.0 - 2.0 * (y as f64 + offset.y) / height as f64) * self.half_height;
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
    fn pixel_sample_offsets_preserve_centers_and_match_camera_plane_coordinates() {
        let camera = front_camera();
        for y in 0..2 {
            for x in 0..4 {
                let center = camera.ray_for_pixel(x, y, 4, 2).unwrap();
                let sample = camera
                    .ray_for_pixel_sample(x, y, 4, 2, Vector2::repeat(0.5))
                    .unwrap();
                assert_eq!(center.origin, sample.origin);
                assert_eq!(center.direction, sample.direction);
            }
        }
        for (x, y, width, height, offset, expected) in [
            (
                0,
                0,
                2,
                2,
                Vector2::new(0.0, 0.0),
                Vector3::new(-1.0, 1.0, 2.0),
            ),
            (
                0,
                0,
                2,
                2,
                Vector2::new(0.25, 0.75),
                Vector3::new(-0.75, 0.25, 2.0),
            ),
            (
                2,
                1,
                4,
                2,
                Vector2::new(0.25, 0.75),
                Vector3::new(0.25, -0.75, 2.0),
            ),
        ] {
            let ray = camera
                .ray_for_pixel_sample(x, y, width, height, offset)
                .unwrap();
            let on_plane = ray.at(-1.0 / ray.direction.z);
            assert!((on_plane - expected).norm() < 1e-12);
            assert_eq!(ray.origin, camera.position());
            assert!((ray.direction.norm() - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn pixel_samples_reject_invalid_offsets_dimensions_and_indices() {
        let camera = front_camera();
        for value in [
            -f64::EPSILON,
            1.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            for offset in [Vector2::new(value, 0.5), Vector2::new(0.5, value)] {
                assert!(camera.ray_for_pixel_sample(0, 0, 2, 2, offset).is_none());
            }
        }
        for (x, y, width, height) in [
            (0, 0, 0, 1),
            (0, 0, 1, 0),
            (2, 0, 2, 2),
            (0, 2, 2, 2),
            (usize::MAX, usize::MAX, 2, 2),
        ] {
            assert!(
                camera
                    .ray_for_pixel_sample(x, y, width, height, Vector2::repeat(0.5))
                    .is_none()
            );
            assert!(camera.ray_for_pixel(x, y, width, height).is_none());
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
}

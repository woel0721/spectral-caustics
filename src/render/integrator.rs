//! 相机视角的单次直接光照，在线性 RGB 中着色并保存为 XYZ。

use super::{framebuffer::Framebuffer, utils::linear_srgb_to_xyz};
use crate::{
    material::{BsdfValue, EvaluateMode},
    scene::Scene,
};
use nalgebra::Vector3;
use std::{error::Error, mem::size_of};

/// 每像素从相机发射一条主光线，计算 L_o = f_r L_light max(n · wi, 0)。
///
/// wi 指向光源（光源传播方向的反向），wo 指向相机；保留几何法线朝向。
/// 将 total_flux / 水平孔径面积作为白色方向光的相对强度 L_light，
/// 不做有限孔径裁切、方向投影校正或阴影查询；这不是绝对辐射度标定。
/// 未命中使用场景线性 RGB 背景；未绑定材质的命中和理想玻璃为黑色。
/// 本阶段不追踪反射/折射或间接光，也不叠加地面焦散。
/// 返回未经曝光的线性 XYZ，交给 Render::save 统一曝光和 sRGB 编码。
pub fn render_camera_lit(
    scene: &Scene,
    width: usize,
    height: usize,
) -> Result<Framebuffer, Box<dyn Error>> {
    if width == 0 || height == 0 {
        return Err("相机光照图的宽高必须大于零".into());
    }
    u32::try_from(width)?;
    u32::try_from(height)?;
    width
        .checked_mul(height)
        .and_then(|count| count.checked_mul(size_of::<Vector3<f64>>()))
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or("相机光照图的像素缓冲大小溢出")?;

    let direction_length = scene.light.direction.norm();
    if !direction_length.is_finite() || direction_length == 0.0 {
        return Err("光源方向必须有限且非零".into());
    }
    let wi = -scene.light.direction / direction_length;
    let span = scene.light.aperture_max_m - scene.light.aperture_min_m;
    let area = span.x * span.y;
    if span.iter().any(|v| !v.is_finite() || *v <= 0.0)
        || !area.is_finite()
        || area <= 0.0
        || !scene.light.total_flux.is_finite()
        || scene.light.total_flux < 0.0
    {
        return Err("光源孔径面积须有限且为正，总通量须有限且非负".into());
    }
    let light = scene.light.total_flux / area;
    if !light.is_finite() {
        return Err("直接光照强度计算溢出".into());
    }

    let mut framebuffer = Framebuffer::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let ray = scene
                .camera
                .ray_for_pixel(x, y, width, height)
                .ok_or("无法生成摄像机光线")?;
            let linear_rgb = match scene.intersect(&ray) {
                None => scene.background_color,
                Some(hit) => {
                    let Some(material) = hit.material_id.and_then(|id| scene.material(id)) else {
                        continue; // 保留帧缓冲的黑色初值。
                    };
                    let normal_length = hit.normal.norm();
                    if !normal_length.is_finite() || normal_length == 0.0 {
                        return Err("命中法线无效，无法计算直接光照".into());
                    }
                    let normal = hit.normal / normal_length;
                    let wo = -ray.direction;
                    let BsdfValue::Rgb(f_r) = material.evaluate(wi, wo, normal, EvaluateMode::Rgb)
                    else {
                        return Err("材质 RGB 求值必须返回 RGB BSDF".into());
                    };
                    let cosine = normal.dot(&wi).clamp(0.0, 1.0);
                    f_r * (light * cosine)
                }
            };
            let xyz = linear_srgb_to_xyz(linear_rgb).ok_or("直接光照颜色非法或计算溢出")?;
            if !framebuffer.set(x, y, xyz) {
                return Err("无法写入相机光照图像素".into());
            }
        }
    }
    Ok(framebuffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        camera::Camera,
        config::{
            cameraconfig::CameraConfig,
            materialconfig::MaterialConfig,
            sceneconfig::{ObjectConfig, SceneConfig},
        },
        geometry::{ground::Ground, hit::Hit, object::Hittable, ray::Ray},
        material::{Material, lambertian::Lambertian},
        render::utils::xyz_to_linear_srgb,
    };
    use std::f64::consts::PI;

    fn ground_scene() -> Scene {
        Scene::from_config(&SceneConfig {
            camera: CameraConfig {
                position: [0.0, 0.0, 2.0],
                target: [0.0; 3],
                up: [0.0, 1.0, 0.0],
                fov_y_degrees: 45.0,
            },
            objects: vec![ObjectConfig::Ground {
                height_m: 0.0,
                material: MaterialConfig::Lambertian {
                    albedo: [0.2, 0.5, 0.8],
                },
            }],
            ..SceneConfig::default()
        })
        .unwrap()
    }

    #[test]
    fn direct_lambertian_matches_albedo_light_and_one_cosine_without_clipping() {
        let mut scene = ground_scene();
        for (direction, cosine) in [
            (Vector3::new(0.0, 0.0, -5.0), 1.0),
            (Vector3::new(-3.0, 0.0, -4.0), 0.8),
            (Vector3::z(), 0.0),
            (Vector3::x(), 0.0),
        ] {
            scene.light.direction = direction;
            for flux in [0.0, 8.0, 64.0] {
                scene.light.total_flux = flux;
                let image = render_camera_lit(&scene, 3, 2).unwrap();
                assert_eq!((image.width, image.height), (3, 2));
                // 孔径为 4×4 米，Lambertian 为 albedo/π；只乘一次 cosθ。
                let expected = Vector3::new(0.2, 0.5, 0.8) * (flux / 16.0 * cosine / PI);
                for xyz in &image.pixels {
                    assert!((xyz_to_linear_srgb(*xyz).unwrap() - expected).norm() < 1e-12);
                }
            }
        }
        // 保持总通量，孔径面积减半使当前相对方向光强度加倍。
        scene.light.direction = -Vector3::z();
        scene.light.total_flux = 8.0;
        scene.light.aperture_max_m.x = 0.0;
        let image = render_camera_lit(&scene, 1, 1).unwrap();
        let rgb = xyz_to_linear_srgb(*image.get(0, 0).unwrap()).unwrap();
        assert!((rgb - Vector3::new(0.2, 0.5, 0.8) / PI).norm() < 1e-12);
    }

    #[test]
    fn camera_lighting_uses_nearest_material_and_linear_background() {
        let mut config = SceneConfig::default();
        config.camera.position = [0.0, -4.0, 2.0];
        config.camera.target = [0.0, 0.0, 1.0];
        config.camera.fov_y_degrees = 90.0;
        config.background_linear_rgb = [0.1, 0.2, 0.3];
        let scene = Scene::from_config(&config).unwrap();
        let image = render_camera_lit(&scene, 5, 3).unwrap();
        let rgb_at = |x, y| xyz_to_linear_srgb(*image.get(x, y).unwrap()).unwrap();
        assert!((rgb_at(2, 0) - Vector3::new(0.1, 0.2, 0.3)).norm() < 1e-12);
        assert_eq!(rgb_at(2, 1), Vector3::zeros()); // 最近命中理想玻璃，不能显示后方地面。
        assert!((rgb_at(2, 2) - Vector3::repeat(0.4 / PI)).norm() < 1e-12);
        config.objects.reverse();
        let scene = Scene::from_config(&config).unwrap();
        assert_eq!(
            render_camera_lit(&scene, 5, 3).unwrap().pixels,
            image.pixels
        );

        let mut scene = ground_scene();
        scene.background_color = Vector3::repeat(1.0);
        // 未绑定材质的最近表面保留黑色，而不是背景或后方材质。
        scene.add(Box::new(Ground::new(1.0).unwrap()));
        assert_eq!(
            render_camera_lit(&scene, 1, 1).unwrap().pixels,
            vec![Vector3::zeros()]
        );

        let mut scene = ground_scene();
        scene.camera = Camera::from_config(&CameraConfig {
            position: [0.0, 0.0, -1.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 45.0,
        })
        .unwrap();
        assert_eq!(
            render_camera_lit(&scene, 1, 1).unwrap().pixels,
            vec![Vector3::zeros()]
        );
    }

    #[test]
    fn camera_lighting_rejects_invalid_dimensions_and_light() {
        let mut scene = ground_scene();
        for (width, height) in [
            (0, 1),
            (1, 0),
            (usize::MAX, 1),
            (u32::MAX as usize, u32::MAX as usize),
        ] {
            assert!(render_camera_lit(&scene, width, height).is_err());
        }
        for direction in [
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
            Vector3::repeat(f64::INFINITY),
            Vector3::repeat(f64::MAX),
        ] {
            scene.light.direction = direction;
            assert!(render_camera_lit(&scene, 1, 1).is_err());
        }
        scene.light.direction = -Vector3::z();
        for flux in [-1.0, f64::NAN, f64::INFINITY] {
            scene.light.total_flux = flux;
            assert!(render_camera_lit(&scene, 1, 1).is_err());
        }
        scene.light.total_flux = 8.0;
        scene.light.aperture_max_m = scene.light.aperture_min_m;
        assert!(render_camera_lit(&scene, 1, 1).is_err());
        scene.light.aperture_min_m.fill(0.0);
        scene.light.aperture_max_m.fill(1e-150);
        scene.light.total_flux = f64::MAX;
        assert!(render_camera_lit(&scene, 1, 1).is_err());
    }

    #[test]
    fn camera_lighting_rejects_invalid_normals_and_bsdf_results() {
        struct TestSurface(Vector3<f64>);
        impl Hittable for TestSurface {
            fn intersect(&self, _ray: &Ray) -> Option<Hit> {
                Some(Hit {
                    normal: self.0,
                    distance: 0.5,
                    ..Hit::default()
                })
            }
        }
        for normal in [
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
            Vector3::repeat(f64::MAX),
        ] {
            let mut scene = ground_scene();
            scene.add_with_material(
                Box::new(TestSurface(normal)),
                Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
            );
            assert!(render_camera_lit(&scene, 1, 1).is_err());
        }
        struct TestMaterial(BsdfValue);
        impl Material for TestMaterial {
            fn evaluate(
                &self,
                _wi: Vector3<f64>,
                _wo: Vector3<f64>,
                _normal: Vector3<f64>,
                _mode: EvaluateMode,
            ) -> BsdfValue {
                self.0
            }
        }
        for value in [
            BsdfValue::Rgb(Vector3::repeat(f64::NAN)),
            BsdfValue::Spectral(0.5),
        ] {
            let mut scene = ground_scene();
            scene.add_with_material(
                Box::new(TestSurface(Vector3::z() * 3.0)),
                Box::new(TestMaterial(value)),
            );
            assert!(render_camera_lit(&scene, 1, 1).is_err());
        }
    }
}

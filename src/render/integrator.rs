//! 相机视角的直接光照与有界玻璃路径追踪，在线性 RGB 中计算并保存为 XYZ。

use super::{framebuffer::Framebuffer, utils::linear_srgb_to_xyz};
use crate::{
    geometry::{
        hit::Hit,
        ray::{INTERSECTION_EPSILON, Ray},
    },
    material::{BsdfValue, EvaluateMode, Material, MaterialSample},
    scene::Scene,
};
use nalgebra::Vector3;
use rand::{SeedableRng, rngs::StdRng};
use std::{error::Error, mem::size_of};

const MAX_GLASS_BOUNCES: usize = 8;

/// 每像素从相机发射一条主光线，计算 L_o = V f_r L_light max(n · wi, 0)。
///
/// wi 指向光源（光源传播方向的反向），wo 指向相机；保留几何法线朝向。
/// 将 total_flux / 水平孔径面积作为白色方向光的相对强度 L_light，
/// 不做有限孔径裁切或方向投影校正；这不是绝对辐射度标定。
/// 对有直接光贡献的命中，沿法线偏移起点后向 wi 发射 shadow ray；
/// 平行光位于无穷远，沿途任意表面（包括玻璃）均令可见性 V = 0。
/// 未命中使用场景线性 RGB 背景；未绑定材质的命中和理想玻璃为黑色。
/// 本阶段不追踪反射/折射或间接光，也不叠加地面焦散。
/// 返回未经曝光的线性 XYZ，交给 Render::save 统一曝光和 sRGB 编码。
pub fn render_camera_lit(
    scene: &Scene,
    width: usize,
    height: usize,
) -> Result<Framebuffer, Box<dyn Error>> {
    let (wi, light) = camera_lighting_setup(scene, width, height)?;
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
                        continue;
                    };
                    direct_lighting(scene, &hit, material, -ray.direction, wi, light)?
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

/// 相机路径追踪：每像素一条中心射线，最多采样八次玻璃反射/透射。
///
/// seed 初始化本次渲染独有的 RNG，按 y、x 和路径顺序采样，固定种子可复现。
/// wo = -ray.direction，保留朝外几何法线供玻璃判断进入/离开。
/// 玻璃只乘 sample.weight，不再乘 Fresnel/余弦或除以 probability。
/// 每次沿采样方向所在侧的法线偏移 1e-5 米，再构造下一条 Ray。
/// 未命中时累加 throughput * 背景；命中非 delta 材质时累加带硬阴影的
/// throughput * 直接光照并终止，不采样漫反射间接光。
/// 第八次散射后仍查询背景/非 delta 表面，第九个玻璃界面处截断；
/// 未绑定材质或没有采样结果的路径终止，未获得的贡献为零。
/// 返回未经曝光的线性 XYZ，不叠加光源焦散，也不模拟透明 shadow ray。
pub fn render_camera_glass(
    scene: &Scene,
    width: usize,
    height: usize,
    seed: u64,
) -> Result<Framebuffer, Box<dyn Error>> {
    let (light_wi, light) = camera_lighting_setup(scene, width, height)?;
    let mut rng = StdRng::seed_from_u64(seed);
    let mut framebuffer = Framebuffer::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let mut ray = scene
                .camera
                .ray_for_pixel(x, y, width, height)
                .ok_or("无法生成摄像机光线")?;
            let mut throughput = 1.0;
            let mut radiance = Vector3::zeros();
            for bounce in 0..=MAX_GLASS_BOUNCES {
                let Some(hit) = scene.intersect(&ray) else {
                    radiance += scene.background_color * throughput;
                    break;
                };
                let Some(material) = hit.material_id.and_then(|id| scene.material(id)) else {
                    break;
                };
                let wo = -ray.direction;
                if !material.is_delta() {
                    radiance +=
                        direct_lighting(scene, &hit, material, wo, light_wi, light)? * throughput;
                    break;
                }
                if bounce == MAX_GLASS_BOUNCES {
                    break;
                }
                let normal_length = hit.normal.norm();
                if !normal_length.is_finite() || normal_length == 0.0 {
                    return Err("命中法线无效，无法采样玻璃路径".into());
                }
                let normal = hit.normal / normal_length;
                let Some(sample) = material.sample(wo, normal, &mut rng) else {
                    break;
                };
                let MaterialSample::Glass(sample) = sample else {
                    return Err("delta 材质必须返回玻璃离散事件采样".into());
                };
                if !sample.probability.is_finite()
                    || sample.probability <= 0.0
                    || sample.probability > 1.0
                    || !sample.weight.is_finite()
                    || sample.weight < 0.0
                {
                    return Err("玻璃采样概率或权重无效".into());
                }
                throughput *= sample.weight;
                if !throughput.is_finite() {
                    return Err("玻璃路径吞吐量计算溢出".into());
                }
                if throughput == 0.0 {
                    break;
                }
                let side = if sample.wi.dot(&normal) >= 0.0 {
                    1.0
                } else {
                    -1.0
                };
                let origin = hit.coords + normal * (side * 10.0 * INTERSECTION_EPSILON);
                ray = Ray::new(origin, sample.wi).ok_or("玻璃路径射线起点或方向无效")?;
            }
            let xyz = linear_srgb_to_xyz(radiance).ok_or("玻璃路径颜色非法或计算溢出")?;
            if !framebuffer.set(x, y, xyz) {
                return Err("无法写入玻璃路径图像素".into());
            }
        }
    }
    Ok(framebuffer)
}

/// 两个相机入口共享尺寸和方向光校验，以及同一相对光强约定。
fn camera_lighting_setup(
    scene: &Scene,
    width: usize,
    height: usize,
) -> Result<(Vector3<f64>, f64), Box<dyn Error>> {
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
    Ok((wi, light))
}

fn direct_lighting(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    wi: Vector3<f64>,
    light: f64,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let normal_length = hit.normal.norm();
    if !normal_length.is_finite() || normal_length == 0.0 {
        return Err("命中法线无效，无法计算直接光照".into());
    }
    let normal = hit.normal / normal_length;
    let BsdfValue::Rgb(f_r) = material.evaluate(wi, wo, normal, EvaluateMode::Rgb) else {
        return Err("材质 RGB 求值必须返回 RGB BSDF".into());
    };
    let cosine = normal.dot(&wi).clamp(0.0, 1.0);
    let direct_rgb = f_r * (light * cosine);
    if !direct_rgb.iter().all(|v| v.is_finite()) {
        return Err("直接光照颜色非法或计算溢出".into());
    }
    if direct_rgb != Vector3::zeros() {
        let origin = hit.coords + normal * (10.0 * INTERSECTION_EPSILON);
        let shadow_ray = Ray::new(origin, wi).ok_or("阴影光线起点或方向无效")?;
        if scene.intersect(&shadow_ray).is_some() {
            return Ok(Vector3::zeros());
        }
    }
    Ok(direct_rgb)
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
        geometry::{ground::Ground, hit::Hit, object::Hittable, prism::Prism, triangle::Triangle},
        material::{
            GlassEvent, GlassSample, Material, dielectric::Dielectric, lambertian::Lambertian,
        },
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

    fn glass_background_scene() -> Scene {
        let mut scene = Scene::new();
        scene.camera = Camera::from_config(&CameraConfig {
            position: [0.0, 0.0, 3.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 45.0,
        })
        .unwrap();
        scene.background_color = Vector3::new(0.2, 0.4, 0.8);
        scene
    }

    struct TestGlass(Option<MaterialSample>);

    impl Material for TestGlass {
        fn evaluate(
            &self,
            _wi: Vector3<f64>,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
            _mode: EvaluateMode,
        ) -> BsdfValue {
            panic!("玻璃路径应使用采样记录，不能直接求普通 BSDF");
        }

        fn sample(
            &self,
            _wo: Vector3<f64>,
            _normal: Vector3<f64>,
            _rng: &mut dyn rand::RngCore,
        ) -> Option<MaterialSample> {
            self.0
        }

        fn is_delta(&self) -> bool {
            true
        }
    }

    fn transmission(wi: Vector3<f64>, probability: f64, weight: f64) -> MaterialSample {
        MaterialSample::Glass(GlassSample {
            wi,
            event: GlassEvent::Transmission,
            probability,
            weight,
        })
    }

    #[test]
    fn glass_renderer_matches_direct_lighting_without_glass_and_respects_unbound_hits() {
        let mut scene = ground_scene();
        for seed in [0, 42] {
            assert_eq!(
                render_camera_glass(&scene, 3, 2, seed).unwrap().pixels,
                render_camera_lit(&scene, 3, 2).unwrap().pixels
            );
        }
        scene.add(Box::new(Ground::new(1.0).unwrap()));
        assert_eq!(
            render_camera_glass(&scene, 1, 1, 42).unwrap().pixels,
            vec![Vector3::zeros()]
        );
        scene.objects.clear();
        scene.background_color = Vector3::new(0.1, 0.3, 0.7);
        assert_eq!(
            render_camera_glass(&scene, 3, 2, 42).unwrap().pixels,
            render_camera_lit(&scene, 3, 2).unwrap().pixels
        );
    }

    #[test]
    fn glass_renderer_weights_background_on_both_sides_and_repeats_seeded_paths() {
        let glass = Dielectric::new(1.5).unwrap();
        for (position, target) in [
            ([0.0, 0.0, 3.0], [0.0; 3]),
            ([0.0; 3], [0.0, 0.0, 1.0]),
            ([-3.0_f64.sqrt(), 0.0, 0.0], [0.0, 0.0, 1.0]),
        ] {
            let mut scene = glass_background_scene();
            scene.camera = Camera::from_config(&CameraConfig {
                position,
                target,
                up: [0.0, 1.0, 0.0],
                fov_y_degrees: 45.0,
            })
            .unwrap();
            scene.add_with_material(Box::new(Ground::new(1.0).unwrap()), Box::new(glass.clone()));
            let mut saw_reflection = false;
            let mut saw_transmission = false;
            for seed in 0..64 {
                let wo = -scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap().direction;
                let MaterialSample::Glass(sample) = glass
                    .sample(wo, Vector3::z(), &mut StdRng::seed_from_u64(seed))
                    .unwrap()
                else {
                    panic!("必须是玻璃采样");
                };
                saw_reflection |= sample.event == GlassEvent::Reflection;
                saw_transmission |= sample.event == GlassEvent::Transmission;
                let image = render_camera_glass(&scene, 1, 1, seed).unwrap();
                let rgb = xyz_to_linear_srgb(image.pixels[0]).unwrap();
                assert!((rgb - scene.background_color * sample.weight).norm() < 1e-12);
            }
            assert!(saw_reflection);
            if position[0] == 0.0 {
                assert!(saw_transmission);
            }
        }
        let mut scene = glass_background_scene();
        scene.add_with_material(Box::new(Ground::new(1.0).unwrap()), Box::new(glass));
        let reference = render_camera_glass(&scene, 32, 16, 42).unwrap();
        assert_eq!(
            reference.pixels,
            render_camera_glass(&scene, 32, 16, 42).unwrap().pixels
        );
        assert_ne!(
            reference.pixels,
            render_camera_glass(&scene, 32, 16, 43).unwrap().pixels
        );
    }

    #[test]
    fn glass_renderer_crosses_both_slab_interfaces_without_self_intersection() {
        struct Bottom(Ground);
        impl Hittable for Bottom {
            fn intersect(&self, ray: &Ray) -> Option<Hit> {
                self.0.intersect(ray).map(|mut hit| {
                    hit.normal = -hit.normal;
                    hit
                })
            }
        }
        let mut scene = glass_background_scene();
        scene.add_with_material(
            Box::new(Ground::new(1.5).unwrap()),
            Box::new(Dielectric::new(1.5).unwrap()),
        );
        scene.add_with_material(
            Box::new(Bottom(Ground::new(1.0).unwrap())),
            Box::new(Dielectric::new(1.5).unwrap()),
        );
        // 入射/出射的 eta² 权重互为倒数；内部反射后离开也应恢复背景。
        for seed in 0..64 {
            let rgb =
                xyz_to_linear_srgb(render_camera_glass(&scene, 1, 1, seed).unwrap().pixels[0])
                    .unwrap();
            assert!((rgb - scene.background_color).norm() < 1e-12);
        }
    }

    #[test]
    fn glass_renderer_reaches_diffuse_light_and_retains_binary_shadows() {
        let mut scene = ground_scene();
        scene.add_with_material(
            Box::new(
                Triangle::new(
                    Vector3::new(-0.5, -0.5, 1.0),
                    Vector3::new(0.5, -0.5, 1.0),
                    Vector3::new(0.0, 0.5, 1.0),
                )
                .unwrap(),
            ),
            Box::new(TestGlass(Some(transmission(-Vector3::z(), 0.25, 0.5)))),
        );
        // 斜向 shadow ray 绕过有限玻璃面；概率与余弦不能再乘到吞吐量上。
        scene.light.direction = Vector3::new(0.8, 0.0, -0.6);
        let rgb =
            xyz_to_linear_srgb(render_camera_glass(&scene, 1, 1, 42).unwrap().pixels[0]).unwrap();
        let expected = Vector3::new(0.2, 0.5, 0.8) * (0.5 * 0.6 * 0.5 / PI);
        assert!((rgb - expected).norm() < 1e-12);
        // 向上的 shadow ray 被玻璃遮挡，仍按现有的二值硬阴影处理。
        scene.light.direction = -Vector3::z();
        assert_eq!(
            render_camera_glass(&scene, 1, 1, 42).unwrap().pixels,
            vec![Vector3::zeros()]
        );
    }

    #[test]
    fn glass_renderer_allows_eight_events_and_truncates_the_ninth_glass_hit() {
        for count in [8, 9] {
            let mut scene = glass_background_scene();
            scene.camera = Camera::from_config(&CameraConfig {
                position: [0.0, 0.0, 12.0],
                target: [0.0; 3],
                up: [0.0, 1.0, 0.0],
                fov_y_degrees: 45.0,
            })
            .unwrap();
            for height in 1..=count {
                scene.add_with_material(
                    Box::new(Ground::new(f64::from(height)).unwrap()),
                    Box::new(Dielectric::new(1.0).unwrap()),
                );
            }
            let expected = if count == 8 {
                scene.background_color
            } else {
                Vector3::zeros()
            };
            let rgb = xyz_to_linear_srgb(render_camera_glass(&scene, 1, 1, 42).unwrap().pixels[0])
                .unwrap();
            assert!((rgb - expected).norm() < 1e-12);
        }
    }

    #[test]
    fn glass_renderer_handles_failed_samples_and_rejects_invalid_values() {
        for sample in [None, Some(transmission(-Vector3::z(), 1.0, 0.0))] {
            let mut scene = glass_background_scene();
            scene.add_with_material(
                Box::new(Ground::new(1.0).unwrap()),
                Box::new(TestGlass(sample)),
            );
            assert_eq!(
                render_camera_glass(&scene, 1, 1, 42).unwrap().pixels,
                vec![Vector3::zeros()]
            );
        }
        let mut invalid = vec![MaterialSample::Continuous { wi: -Vector3::z() }];
        for value in [f64::NAN, f64::INFINITY, -1.0] {
            invalid.push(transmission(-Vector3::z(), 1.0, value));
            invalid.push(transmission(-Vector3::z(), value, 1.0));
        }
        for probability in [0.0, 1.1] {
            invalid.push(transmission(-Vector3::z(), probability, 1.0));
        }
        for wi in [
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
            Vector3::repeat(f64::INFINITY),
        ] {
            invalid.push(transmission(wi, 1.0, 1.0));
        }
        for sample in invalid {
            let mut scene = glass_background_scene();
            scene.add_with_material(
                Box::new(Ground::new(1.0).unwrap()),
                Box::new(TestGlass(Some(sample))),
            );
            assert!(render_camera_glass(&scene, 1, 1, 42).is_err());
        }
        let mut scene = glass_background_scene();
        for height in [1.0, 1.5] {
            scene.add_with_material(
                Box::new(Ground::new(height).unwrap()),
                Box::new(TestGlass(Some(transmission(-Vector3::z(), 1.0, 1e308)))),
            );
        }
        assert!(render_camera_glass(&scene, 1, 1, 42).is_err());
        for (width, height) in [
            (0, 1),
            (1, 0),
            (usize::MAX, 2),
            (u32::MAX as usize, u32::MAX as usize),
        ] {
            assert!(render_camera_glass(&scene, width, height, 42).is_err());
        }
        scene.light.direction = Vector3::zeros();
        assert!(render_camera_glass(&scene, 1, 1, 42).is_err());
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
    fn shadow_ray_blocks_light_through_glass_without_hiding_the_ground_from_camera() {
        let mut config = SceneConfig::default();
        // 相机从侧面看到三棱镜正下方的地面，只有向上的 shadow ray 命中玻璃。
        config.camera.position = [3.0, 0.0, 2.0];
        config.camera.target = [0.25, 0.0, 0.0];
        config.camera.up = [0.0, 1.0, 0.0];
        config.background_linear_rgb = [0.2, 0.3, 0.4];
        for reverse in [false, true] {
            if reverse {
                config.objects.reverse();
            }
            let scene = Scene::from_config(&config).unwrap();
            let ray = scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap();
            let hit = scene.intersect(&ray).unwrap();
            assert_eq!(hit.object_id, scene.ground_id);
            assert!((hit.coords - Vector3::new(0.25, 0.0, 0.0)).norm() < 1e-12);
            assert_eq!(
                render_camera_lit(&scene, 1, 1).unwrap().pixels,
                vec![Vector3::zeros()]
            );
        }
        // 同样的受光地面移出玻璃投影，恢复原有直接光照。
        config.camera.target = [1.5, 0.0, 0.0];
        let scene = Scene::from_config(&config).unwrap();
        let image = render_camera_lit(&scene, 1, 1).unwrap();
        let rgb = xyz_to_linear_srgb(*image.get(0, 0).unwrap()).unwrap();
        assert!((rgb - Vector3::repeat(0.4 / PI)).norm() < 1e-12);
    }

    #[test]
    fn shadow_ray_ignores_surfaces_behind_it_and_checks_directional_light_to_infinity() {
        let mut scene = ground_scene();
        scene.camera = Camera::from_config(&CameraConfig {
            position: [3.0, 0.0, 2.0],
            target: [0.25, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 45.0,
        })
        .unwrap();
        let reference = render_camera_lit(&scene, 1, 1).unwrap();
        assert!(reference.pixels[0].norm() > 0.0);
        scene.add(Box::new(Ground::new(-1.0).unwrap()));
        assert_eq!(
            render_camera_lit(&scene, 1, 1).unwrap().pixels,
            reference.pixels
        );

        // 遮挡物在配置的发射高度以上，方向光仍需检测它；无材质也会遮光。
        let mut vertices = SceneConfig::default()
            .objects
            .into_iter()
            .find_map(|object| {
                if let ObjectConfig::Prism { vertices_m, .. } = object {
                    Some(vertices_m.map(Vector3::from))
                } else {
                    None
                }
            })
            .unwrap();
        for vertex in &mut vertices {
            vertex.z += 3.0;
        }
        scene.add(Box::new(Prism::from_vertices(vertices).unwrap()));
        assert_eq!(
            render_camera_lit(&scene, 1, 1).unwrap().pixels,
            vec![Vector3::zeros()]
        );
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

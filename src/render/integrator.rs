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
use nalgebra::{Vector2, Vector3};
use rand::{Rng, SeedableRng, rngs::StdRng};
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

/// 每像素独立追踪 spp 条路径，在线性 RGB 中平均后保存为线性 XYZ。
///
/// spp 必须大于零，表示每像素路径总数；每个样本在像素内均匀选取
/// 两个 [0, 1) 偏移，随后独立追踪玻璃路径；spp=1 也包含子像素抖动。
/// seed 只在渲染开始时初始化 RNG，按 y、x、样本（偏移、路径）顺序消耗随机数。
/// 路径最多经过八次玻璃散射，其余光照和截断规则见 trace_camera_path。
/// 返回未经曝光的线性 XYZ，交给 Render::save 统一曝光和 sRGB 编码。
pub fn render_camera_glass(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
) -> Result<Framebuffer, Box<dyn Error>> {
    if spp == 0 {
        return Err("相机 SPP 必须大于零".into());
    }
    let (light_wi, light) = camera_lighting_setup(scene, width, height)?;
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(spp))
        .ok_or("相机路径样本总数溢出")?;
    let mut rng = StdRng::seed_from_u64(seed);
    let mut framebuffer = Framebuffer::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let mut mean_rgb = Vector3::zeros();
            for _ in 0..spp {
                let offset = Vector2::new(rng.random::<f64>(), rng.random::<f64>());
                let ray = scene
                    .camera
                    .ray_for_pixel_sample(x, y, width, height, offset)
                    .ok_or("无法生成子像素摄像机光线")?;
                // 先缩放每项再相加，避免有限样本之和溢出而平均值仍有限。
                mean_rgb += trace_camera_path(scene, ray, &mut rng, light_wi, light)? / spp as f64;
            }
            let xyz = linear_srgb_to_xyz(mean_rgb).ok_or("相机样本平均颜色非法或计算溢出")?;
            if !framebuffer.set(x, y, xyz) {
                return Err("无法写入玻璃路径图像素".into());
            }
        }
    }
    Ok(framebuffer)
}

/// 追踪一条相机射线，返回未曝光的线性 RGB 辐亮度。
///
/// 每条路径重新初始化吞吐量与辐亮度；RNG 由像素采样循环持有，不能逐路径重设种子。
/// wo = -ray.direction，保留朝外几何法线；玻璃只乘 sample.weight。
/// 新起点沿采样方向所在侧的法线偏移 1e-5 米，第九个玻璃命中处截断。
/// 未命中返回背景贡献；非 delta 命中累加带硬阴影的直接光照并终止。
/// 不采样漫反射间接光、不叠加焦散，也不模拟透明 shadow ray。
fn trace_camera_path(
    scene: &Scene,
    mut ray: Ray,
    rng: &mut dyn rand::RngCore,
    light_wi: Vector3<f64>,
    light: f64,
) -> Result<Vector3<f64>, Box<dyn Error>> {
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
            radiance += direct_lighting(scene, &hit, material, wo, light_wi, light)? * throughput;
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
        let Some(sample) = material.sample(wo, normal, rng) else {
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
    if !radiance.iter().all(|value| value.is_finite()) {
        return Err("玻璃路径颜色非法或计算溢出".into());
    }
    Ok(radiance)
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

    fn trace_center_pixel(scene: &Scene, seed: u64) -> Vector3<f64> {
        let (light_wi, light) = camera_lighting_setup(scene, 1, 1).unwrap();
        let ray = scene.camera.ray_for_pixel(0, 0, 1, 1).unwrap();
        trace_camera_path(
            scene,
            ray,
            &mut StdRng::seed_from_u64(seed),
            light_wi,
            light,
        )
        .unwrap()
    }

    #[test]
    fn camera_spp_averages_independent_paths_and_is_reproducible() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct AlternatingGlass(Arc<AtomicUsize>);
        impl Material for AlternatingGlass {
            fn evaluate(
                &self,
                _wi: Vector3<f64>,
                _wo: Vector3<f64>,
                _normal: Vector3<f64>,
                mode: EvaluateMode,
            ) -> BsdfValue {
                match mode {
                    EvaluateMode::Rgb => BsdfValue::Rgb(Vector3::zeros()),
                    EvaluateMode::Spectral { .. } => BsdfValue::Spectral(0.0),
                }
            }
            fn sample(
                &self,
                _wo: Vector3<f64>,
                _normal: Vector3<f64>,
                _rng: &mut dyn rand::RngCore,
            ) -> Option<MaterialSample> {
                let index = self.0.fetch_add(1, Ordering::Relaxed);
                Some(MaterialSample::Glass(GlassSample {
                    wi: Vector3::z(),
                    event: GlassEvent::Reflection,
                    probability: 0.5,
                    weight: if index & 1 == 0 { 0.25 } else { 1.75 },
                }))
            }
            fn is_delta(&self) -> bool {
                true
            }
        }
        for (spp, expected_weight) in [(1, 0.25), (3, 0.75), (4, 1.0), (64, 1.0)] {
            let count = Arc::new(AtomicUsize::new(0));
            let mut scene = glass_background_scene();
            scene.add_with_material(
                Box::new(Ground::new(1.0).unwrap()),
                Box::new(AlternatingGlass(count.clone())),
            );
            let image = render_camera_glass(&scene, 1, 1, spp, 42).unwrap();
            let rgb = xyz_to_linear_srgb(image.pixels[0]).unwrap();
            assert!((rgb - scene.background_color * expected_weight).norm() < 1e-12);
            assert_eq!(count.load(Ordering::Relaxed), spp);
        }
        let mut scene = glass_background_scene();
        scene.add_with_material(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(Dielectric::new(1.5).unwrap()),
        );
        let reference = render_camera_glass(&scene, 16, 8, 64, 42).unwrap();
        assert_eq!(
            reference.pixels,
            render_camera_glass(&scene, 16, 8, 64, 42).unwrap().pixels
        );
        assert_ne!(
            reference.pixels,
            render_camera_glass(&scene, 16, 8, 64, 43).unwrap().pixels
        );
    }

    #[test]
    fn jittered_camera_filters_half_pixel_coverage_in_linear_light() {
        struct LeftHalfPlane;
        impl Hittable for LeftHalfPlane {
            fn intersect(&self, ray: &Ray) -> Option<Hit> {
                let hit = Ground::new(0.0).unwrap().intersect(ray)?;
                (hit.coords.x < 0.0).then_some(hit)
            }
        }
        let mut scene = glass_background_scene();
        scene.background_color = Vector3::repeat(1.0);
        // 左半像素命中无材质黑色表面，右半像素看到白色背景，面积均值应为线性 0.5。
        scene.add(Box::new(LeftHalfPlane));
        let image = render_camera_glass(&scene, 1, 1, 4096, 42).unwrap();
        let rgb = xyz_to_linear_srgb(image.pixels[0]).unwrap();
        assert!((rgb - Vector3::repeat(0.5)).norm() < 0.03);
        assert_eq!(
            image.pixels,
            render_camera_glass(&scene, 1, 1, 4096, 42).unwrap().pixels
        );
    }

    #[test]
    fn camera_spp_preserves_deterministic_brightness_and_rejects_invalid_counts() {
        let scene = ground_scene();
        let reference = render_camera_lit(&scene, 3, 2).unwrap();
        for spp in [1, 3, 64] {
            let image = render_camera_glass(&scene, 3, 2, spp, 42).unwrap();
            for (actual, expected) in image.pixels.iter().zip(&reference.pixels) {
                assert!((actual - expected).norm() < 1e-12);
            }
        }
        assert!(render_camera_glass(&scene, 1, 1, 0, 42).is_err());
        assert!(render_camera_glass(&scene, 2, 1, usize::MAX, 42).is_err());
    }

    #[test]
    fn camera_spp_reduces_error_against_analytic_fresnel_expectation() {
        use crate::material::dielectric::fresnel_dielectric;
        let mut scene = glass_background_scene();
        scene.add_with_material(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(Dielectric::new(1.5).unwrap()),
        );
        let error = |spp| {
            let image = render_camera_glass(&scene, 32, 16, spp, 42).unwrap();
            let mut sum = 0.0;
            for y in 0..16 {
                for x in 0..32 {
                    let mut f = 0.0;
                    for sy in 0..4 {
                        for sx in 0..4 {
                            let offset = Vector2::new(
                                (f64::from(sx) + 0.5) / 4.0,
                                (f64::from(sy) + 0.5) / 4.0,
                            );
                            let ray = scene
                                .camera
                                .ray_for_pixel_sample(x, y, 32, 16, offset)
                                .unwrap();
                            f += fresnel_dielectric(-ray.direction.z, 1.0, 1.5).unwrap() / 16.0;
                        }
                    }
                    let expected = scene.background_color * (f + (1.0 - f) * (4.0 / 9.0));
                    let rgb = xyz_to_linear_srgb(*image.get(x, y).unwrap()).unwrap();
                    sum += (rgb - expected).norm_squared();
                }
            }
            sum / (32.0 * 16.0)
        };
        let single = error(1);
        let multiple = error(64);
        assert!(
            multiple < single / 8.0,
            "1 SPP MSE={single}, 64 SPP MSE={multiple}"
        );
    }

    #[test]
    fn glass_renderer_matches_direct_lighting_without_glass_and_respects_unbound_hits() {
        let mut scene = ground_scene();
        for seed in [0, 42] {
            assert_eq!(
                render_camera_glass(&scene, 3, 2, 1, seed).unwrap().pixels,
                render_camera_lit(&scene, 3, 2).unwrap().pixels
            );
        }
        scene.add(Box::new(Ground::new(1.0).unwrap()));
        assert_eq!(
            render_camera_glass(&scene, 1, 1, 1, 42).unwrap().pixels,
            vec![Vector3::zeros()]
        );
        scene.objects.clear();
        scene.background_color = Vector3::new(0.1, 0.3, 0.7);
        assert_eq!(
            render_camera_glass(&scene, 3, 2, 1, 42).unwrap().pixels,
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
                let rgb = trace_center_pixel(&scene, seed);
                assert!((rgb - scene.background_color * sample.weight).norm() < 1e-12);
            }
            assert!(saw_reflection);
            if position[0] == 0.0 {
                assert!(saw_transmission);
            }
        }
        let mut scene = glass_background_scene();
        scene.add_with_material(Box::new(Ground::new(1.0).unwrap()), Box::new(glass));
        let reference = render_camera_glass(&scene, 32, 16, 1, 42).unwrap();
        assert_eq!(
            reference.pixels,
            render_camera_glass(&scene, 32, 16, 1, 42).unwrap().pixels
        );
        assert_ne!(
            reference.pixels,
            render_camera_glass(&scene, 32, 16, 1, 43).unwrap().pixels
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
            let rgb = trace_center_pixel(&scene, seed);
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
        let rgb = trace_center_pixel(&scene, 42);
        let expected = Vector3::new(0.2, 0.5, 0.8) * (0.5 * 0.6 * 0.5 / PI);
        assert!((rgb - expected).norm() < 1e-12);
        // 向上的 shadow ray 被玻璃遮挡，仍按现有的二值硬阴影处理。
        scene.light.direction = -Vector3::z();
        assert_eq!(trace_center_pixel(&scene, 42), Vector3::zeros());
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
            let rgb =
                xyz_to_linear_srgb(render_camera_glass(&scene, 1, 1, 1, 42).unwrap().pixels[0])
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
                render_camera_glass(&scene, 1, 1, 1, 42).unwrap().pixels,
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
            assert!(render_camera_glass(&scene, 1, 1, 1, 42).is_err());
        }
        let mut scene = glass_background_scene();
        for height in [1.0, 1.5] {
            scene.add_with_material(
                Box::new(Ground::new(height).unwrap()),
                Box::new(TestGlass(Some(transmission(-Vector3::z(), 1.0, 1e308)))),
            );
        }
        assert!(render_camera_glass(&scene, 1, 1, 1, 42).is_err());
        for (width, height) in [
            (0, 1),
            (1, 0),
            (usize::MAX, 2),
            (u32::MAX as usize, u32::MAX as usize),
        ] {
            assert!(render_camera_glass(&scene, width, height, 1, 42).is_err());
        }
        scene.light.direction = Vector3::zeros();
        assert!(render_camera_glass(&scene, 1, 1, 1, 42).is_err());
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

//! 分层随机发射平行光，将地面落点累积为线性 XYZ 灰度图。
//! 支持不透明遮挡、固定折射率双界面折射及三波长落点调试，尚不计算真实光谱颜色。

use crate::{
    framebuffer::Framebuffer,
    iormode::IorMode,
    ray::Ray,
    scene::{Scene, refractive_index, take_photon},
    utils::bilinear_weights,
};
use nalgebra::Vector3;
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::error::Error;

// 第一张验证图使用固定的 4×4 米接收区域与同样大小的发射区域。
const GROUND_MIN: f64 = -2.0;
const GROUND_MAX: f64 = 2.0;
const LIGHT_HEIGHT: f64 = 3.0;
const UNBLOCKED_BRIGHTNESS: f64 = 0.5;
// 每次渲染都从同一种子重新开始，两种场景使用相同的发射位置。
const SAMPLING_SEED: u64 = 42;

/// 三张落点调试图对应的波长，单位为 nm；每个波长分配 1/3 总通量。
pub const DEBUG_WAVELENGTHS_NM: [f64; 3] = [450.0, 550.0, 650.0];

/// 将地面世界坐标映射为连续像素坐标；世界 x、y 采用 [-2, 2) 米的半开区间。
/// 世界 +x 朝图片右侧，+y 朝图片上方；整数输出坐标是像素中心，边界在 -0.5。
/// 保留小数位置供双线性分配使用；z 只检查是否有限。
pub fn ground_to_pixel(position: Vector3<f64>, width: usize, height: usize) -> Option<(f64, f64)> {
    if width == 0
        || height == 0
        || !position.iter().all(|v| v.is_finite())
        || !(GROUND_MIN..GROUND_MAX).contains(&position.x)
        || !(GROUND_MIN..GROUND_MAX).contains(&position.y)
    {
        return None;
    }

    let span = GROUND_MAX - GROUND_MIN;
    let u = (position.x - GROUND_MIN) / span;
    let v = (position.y - GROUND_MIN) / span;
    let x = u * width as f64 - 0.5;
    let y = (1.0 - v) * height as f64 - 0.5;
    Some((x, y))
}

/// Day 2 的直接命中灰度图；未先命中地面的光线被遮挡，不继续追踪。
pub fn render_ground_hits(
    scene: &Scene,
    ground_id: usize,
    samples_per_pixel_axis: usize,
) -> Result<Framebuffer, Box<dyn Error>> {
    render_landing_points(scene, ground_id, samples_per_pixel_axis, |ray| {
        let hit = scene.intersect(&ray)?;
        (hit.object_id == Some(ground_id)).then_some(hit.coords)
    })
}

/// 统一追踪固定折射率或 Cauchy 色散，按波长数组顺序返回 N 张灰度落点图。
/// 固定模式传 [None] 可生成一张图；色散模式要求每项均为 Some(有效波长 nm)。
/// 同一条空间光线复用于全部波长，每张图分配 1/N 通量；灰色不代表真实光谱颜色。
pub fn render_refracted_ground_hits<const N: usize>(
    scene: &Scene,
    prism_id: usize,
    ground_id: usize,
    samples_per_pixel_axis: usize,
    wavelengths_nm: [Option<f64>; N],
    mode: IorMode,
) -> Result<[Framebuffer; N], Box<dyn Error>> {
    if prism_id == ground_id || prism_id >= scene.objects.len() {
        return Err("三棱镜编号无效或与地面编号相同".into());
    }
    if wavelengths_nm
        .iter()
        .any(|&wavelength_nm| refractive_index(wavelength_nm, mode).is_none())
    {
        return Err("色散模式要求每项波长均为正有限值，且折射率计算不能溢出".into());
    }
    render_landing_point_sets(scene, ground_id, samples_per_pixel_axis, |ray| {
        wavelengths_nm.map(|wavelength_nm| {
            take_photon(scene, ray, prism_id, ground_id, || {
                refractive_index(wavelength_nm, mode)
            })
        })
    })
}

/// 单落点光路复用批量发射和累积，N = 1 时保持原有通量尺度。
fn render_landing_points(
    scene: &Scene,
    ground_id: usize,
    samples_per_pixel_axis: usize,
    landing_point: impl Fn(Ray) -> Option<Vector3<f64>>,
) -> Result<Framebuffer, Box<dyn Error>> {
    let [framebuffer] =
        render_landing_point_sets(scene, ground_id, samples_per_pixel_axis, |ray| {
            [landing_point(ray)]
        })?;
    Ok(framebuffer)
}

/// 每条空间光线返回 N 个独立落点，每个落点组使用一张图及 1/N 通量。
/// 每个发射小格按每轴 samples_per_pixel_axis 份细分，每层独立随机取一点。
/// 固定种子保证可复现；每条光线都先消耗两个随机数，再进行命中查询。
fn render_landing_point_sets<const N: usize>(
    scene: &Scene,
    ground_id: usize,
    samples_per_pixel_axis: usize,
    landing_points: impl Fn(Ray) -> [Option<Vector3<f64>>; N],
) -> Result<[Framebuffer; N], Box<dyn Error>> {
    if N == 0 || scene.width == 0 || scene.height == 0 || samples_per_pixel_axis == 0 {
        return Err("图像尺寸、每轴采样数和落点组数必须大于零".into());
    }
    if ground_id >= scene.objects.len() {
        return Err("地面编号不在场景中".into());
    }

    let pixel_count = scene
        .width
        .checked_mul(scene.height)
        .ok_or("像素数量溢出")?;
    let samples_x = scene
        .width
        .checked_mul(samples_per_pixel_axis)
        .ok_or("横向采样数溢出")?;
    let samples_y = scene
        .height
        .checked_mul(samples_per_pixel_axis)
        .ok_or("纵向采样数溢出")?;
    let ray_count = samples_x.checked_mul(samples_y).ok_or("光线数量溢出")?;

    let span = GROUND_MAX - GROUND_MIN;
    let area = span * span;
    // 固定总通量，再除以光线数与像素对应的地面面积，得到每条光线的亮度贡献。
    // 这样增加采样数只增加落点密度，不会直接让图片变亮。
    let total_flux = UNBLOCKED_BRIGHTNESS * area;
    let flux_per_ray = total_flux / ray_count as f64;
    let pixel_area = area / pixel_count as f64;
    // 沿用 render.rs 中的 D65 参考白，仅生成已知灰色，不代表已实现光谱积分。
    let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
    // 调试光源只含这 N 个等权离散分量，权重和为 1；尚不做连续光谱积分。
    let wavelength_weight = 1.0 / N as f64;
    let contribution = white_xyz * (flux_per_ray / pixel_area * wavelength_weight);
    let mut framebuffers = std::array::from_fn(|_| Framebuffer::new(scene.width, scene.height));
    let mut rng = StdRng::seed_from_u64(SAMPLING_SEED);

    for sample_y in 0..samples_y {
        for sample_x in 0..samples_x {
            // 分层不变，只把固定的 0.5 偏移改为每轴独立的 [0, 1) 随机偏移。
            let origin_x =
                GROUND_MIN + (sample_x as f64 + rng.random::<f64>()) / samples_x as f64 * span;
            let origin_y =
                GROUND_MIN + (sample_y as f64 + rng.random::<f64>()) / samples_y as f64 * span;
            let ray = Ray::new(
                Vector3::new(origin_x, origin_y, LIGHT_HEIGHT),
                Vector3::new(0.0, 0.0, -1.0),
                0.0,
            )
            .ok_or("平行光线构造失败")?;

            for (framebuffer, position) in framebuffers.iter_mut().zip(landing_points(ray)) {
                // 某个波长未落地时只丢弃它的通量，其余波长继续累积，权重不重新归一化。
                let Some(position) = position else {
                    continue;
                };
                if let Some((x, y)) = ground_to_pixel(position, scene.width, scene.height) {
                    let neighbors = bilinear_weights(x, y, scene.width, scene.height)
                        .ok_or("双线性权重计算失败")?;
                    // 同一条光子的 XYZ 分成四份，权重和为 1；在 sRGB 编码之前累积。
                    for (pixel_x, pixel_y, weight) in neighbors {
                        if weight > 0.0
                            && !framebuffer.accumulate(pixel_x, pixel_y, contribution * weight)
                        {
                            return Err("地面像素累积失败".into());
                        }
                    }
                }
            }
        }
    }
    Ok(framebuffers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ground::Ground, prism::Prism, render::save_png};

    #[test]
    fn world_coordinates_map_to_non_square_image_with_y_flipped() {
        for (position, expected) in [
            (Vector3::new(-2.0, -2.0, 0.0), (-0.5, 1.5)),
            (Vector3::new(-1.5, 1.5, 0.0), (-0.125, -0.25)),
            (Vector3::new(1.5, -1.5, 0.0), (2.125, 1.25)),
            (Vector3::new(1.99, 1.99, 0.0), (2.4925, -0.495)),
        ] {
            let actual = ground_to_pixel(position, 3, 2).unwrap();
            assert!((actual.0 - expected.0).abs() < 1e-12);
            assert!((actual.1 - expected.1).abs() < 1e-12);
        }
        assert_eq!(
            ground_to_pixel(Vector3::new(-1.5, 1.0, 0.0), 4, 2),
            Some((0.0, 0.0))
        );
        assert_eq!(
            ground_to_pixel(Vector3::new(1.5, -1.0, 0.0), 4, 2),
            Some((3.0, 1.0))
        );
    }

    #[test]
    fn mapping_rejects_outside_non_finite_and_empty_inputs() {
        for value in [-2.01, 2.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(ground_to_pixel(Vector3::new(value, 0.0, 0.0), 3, 2), None);
            assert_eq!(ground_to_pixel(Vector3::new(0.0, value, 0.0), 3, 2), None);
        }
        assert_eq!(
            ground_to_pixel(Vector3::new(0.0, 0.0, f64::NAN), 3, 2),
            None
        );
        assert_eq!(ground_to_pixel(Vector3::zeros(), 0, 2), None);
        assert_eq!(ground_to_pixel(Vector3::zeros(), 3, 0), None);
    }

    #[test]
    fn unblocked_ground_preserves_total_energy_and_more_samples_reduce_noise() {
        let mut scene = Scene::new(32, 16);
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let coarse = render_ground_hits(&scene, ground_id, 1).unwrap();
        let fine = render_ground_hits(&scene, ground_id, 3).unwrap();
        let expected_total = 0.5 * (scene.width * scene.height) as f64;
        for image in [&coarse, &fine] {
            let total: f64 = image.pixels.iter().map(|pixel| pixel.y).sum();
            assert!((total - expected_total).abs() < 1e-10);
        }
        let coarse_error: f64 = coarse
            .pixels
            .iter()
            .map(|pixel| (pixel.y - 0.5).powi(2))
            .sum();
        let fine_error: f64 = fine
            .pixels
            .iter()
            .map(|pixel| (pixel.y - 0.5).powi(2))
            .sum();
        assert!(coarse_error > 0.0 && fine_error < coarse_error);
    }

    #[test]
    fn jittered_sampling_covers_each_stratum_once_and_repeats_with_fixed_seed() {
        use std::cell::RefCell;

        let mut scene = Scene::new(4, 3);
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let record = || {
            let origins = RefCell::new(Vec::new());
            let image = render_landing_points(&scene, ground_id, 2, |ray| {
                origins.borrow_mut().push(ray.origin);
                Some(Vector3::new(ray.origin.x, ray.origin.y, 0.0))
            })
            .unwrap();
            (origins.into_inner(), image)
        };
        let (origins, first_image) = record();
        let (repeated_origins, repeated_image) = record();
        assert_eq!(origins, repeated_origins);
        assert_eq!(first_image.pixels, repeated_image.pixels);

        let (samples_x, samples_y) = (8, 6);
        assert_eq!(origins.len(), samples_x * samples_y);
        let span = GROUND_MAX - GROUND_MIN;
        let mut off_center = false;
        let mut different_axis_offsets = false;
        for (index, origin) in origins.iter().enumerate() {
            let (sample_x, sample_y) = (index % samples_x, index / samples_x);
            let x = (origin.x - GROUND_MIN) / span * samples_x as f64;
            let y = (origin.y - GROUND_MIN) / span * samples_y as f64;
            assert!((sample_x as f64..(sample_x + 1) as f64).contains(&x));
            assert!((sample_y as f64..(sample_y + 1) as f64).contains(&y));
            assert_eq!(origin.z, LIGHT_HEIGHT);
            let (jitter_x, jitter_y) = (x - sample_x as f64, y - sample_y as f64);
            off_center |= (jitter_x - 0.5).abs() > 1e-12;
            different_axis_offsets |= (jitter_x - jitter_y).abs() > 1e-12;
        }
        assert!(off_center && different_axis_offsets);
    }

    #[test]
    fn prism_shadow_has_bilinear_edges_and_preserves_flux_in_either_object_order() {
        let mut reference_scene = Scene::new(16, 8);
        let reference_ground_id = reference_scene.add(Box::new(Ground::new(0.0).unwrap()));
        let reference = render_ground_hits(&reference_scene, reference_ground_id, 2).unwrap();
        let mut previous_image = None;
        for ground_first in [false, true] {
            let mut scene = Scene::new(16, 8);
            if !ground_first {
                scene.add(Box::new(Prism::new().unwrap()));
            }
            let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
            if ground_first {
                scene.add(Box::new(Prism::new().unwrap()));
            }
            let framebuffer = render_ground_hits(&scene, ground_id, 2).unwrap();
            // 相同种子的有/无遮挡场景共享发射位置；增加遮挡不能增加任何像素的能量。
            for (shadowed, unblocked) in framebuffer.pixels.iter().zip(&reference.pixels) {
                assert!(shadowed.y <= unblocked.y + 1e-12);
            }
            assert_eq!(framebuffer.get(7, 3).unwrap().y, 0.0);
            assert_eq!(framebuffer.get(0, 0), reference.get(0, 0));
            // 边缘仍有双线性贡献，但比无遮挡时暗；随机位置使具体数值有波动。
            assert!(framebuffer.get(6, 3).unwrap().y > 0.0);
            assert!(framebuffer.get(5, 3).unwrap().y > 0.0);
            assert!(framebuffer.get(5, 3).unwrap().y < reference.get(5, 3).unwrap().y);
            let total_y: f64 = framebuffer.pixels.iter().map(|pixel| pixel.y).sum();
            // 128 个像素中，几何投影遮挡等效的 16 个像素；过滤不改变总量。
            assert!((total_y - 56.0).abs() < 1e-12);
            if let Some(previous) = &previous_image {
                assert_eq!(&framebuffer.pixels, previous);
            }
            previous_image = Some(framebuffer.pixels);
        }
    }

    #[test]
    fn bilinear_landing_distributes_expected_xyz_and_preserves_total_energy() {
        let mut scene = Scene::new(4, 3);
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        // 此世界位置映射到连续像素 (1.25, 0.75)，让所有样本都落在这里。
        let position = Vector3::new(-0.25, 1.0 / 3.0, 0.0);
        let framebuffer = render_landing_points(&scene, ground_id, 1, |_| Some(position)).unwrap();
        for (x, y, expected_y) in [(1, 0, 1.125), (2, 0, 0.375), (1, 1, 3.375), (2, 1, 1.125)] {
            assert!((framebuffer.get(x, y).unwrap().y - expected_y).abs() < 1e-12);
        }
        let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
        let total = framebuffer
            .pixels
            .iter()
            .fold(Vector3::zeros(), |sum, pixel| sum + pixel);
        assert!((total - white_xyz * 6.0).norm() < 1e-12);
    }

    #[test]
    fn bilinear_landing_keeps_all_energy_at_receiver_edges_and_for_one_pixel_images() {
        for (width, height) in [(4, 3), (1, 4), (4, 1), (1, 1)] {
            let mut scene = Scene::new(width, height);
            let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
            let framebuffer = render_landing_points(&scene, ground_id, 2, |_| {
                Some(Vector3::new(-2.0, -2.0, 0.0))
            })
            .unwrap();
            let total_y: f64 = framebuffer.pixels.iter().map(|pixel| pixel.y).sum();
            let expected = 0.5 * (width * height) as f64;
            assert!((total_y - expected).abs() < 1e-12);
            assert!((framebuffer.get(0, height - 1).unwrap().y - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn three_landing_sets_share_spatial_samples_and_preserve_total_xyz() {
        use std::cell::Cell;

        let mut scene = Scene::new(16, 8);
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
        let expected = white_xyz * (0.5 * (scene.width * scene.height) as f64);
        for samples in [1, 3] {
            let calls = Cell::new(0);
            let images = render_landing_point_sets(&scene, ground_id, samples, |ray| {
                calls.set(calls.get() + 1);
                [Some(Vector3::new(ray.origin.x, ray.origin.y, 0.0)); 3]
            })
            .unwrap();
            assert_eq!(calls.get(), scene.width * scene.height * samples * samples);
            let reference = render_ground_hits(&scene, ground_id, samples).unwrap();
            let mut total = Vector3::zeros();
            for image in &images {
                for (pixel, unblocked) in image.pixels.iter().zip(&reference.pixels) {
                    assert!((pixel * 3.0 - unblocked).norm() < 1e-12);
                    total += pixel;
                }
            }
            assert!((total - expected).norm() < 1e-10);
        }
    }

    #[test]
    fn lost_wavelengths_do_not_stop_others_or_redistribute_their_flux() {
        let mut scene = Scene::new(4, 3);
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        for samples in [1, 4] {
            let images = render_landing_point_sets(&scene, ground_id, samples, |_| {
                [
                    None,
                    Some(Vector3::new(0.0, 0.0, 0.0)),
                    Some(Vector3::new(3.0, 0.0, 0.0)),
                ]
            })
            .unwrap();
            let totals = images.map(|image| image.pixels.iter().map(|pixel| pixel.y).sum::<f64>());
            assert_eq!(totals[0], 0.0);
            assert!((totals[1] - 2.0).abs() < 1e-12);
            assert_eq!(totals[2], 0.0);
        }
    }

    #[test]
    fn wavelength_images_match_single_wavelength_paths_and_repeat() {
        let mut scene = Scene::new(64, 32);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let images = render_refracted_ground_hits(
            &scene,
            prism_id,
            ground_id,
            2,
            DEBUG_WAVELENGTHS_NM.map(Some),
            IorMode::Dispersion,
        )
        .unwrap();
        let repeated = render_refracted_ground_hits(
            &scene,
            prism_id,
            ground_id,
            2,
            DEBUG_WAVELENGTHS_NM.map(Some),
            IorMode::Dispersion,
        )
        .unwrap();
        assert_ne!(images[0].pixels, images[1].pixels);
        assert_ne!(images[1].pixels, images[2].pixels);
        for ((wavelength_nm, image), repeated_image) in
            DEBUG_WAVELENGTHS_NM.into_iter().zip(&images).zip(repeated)
        {
            assert_eq!(image.pixels, repeated_image.pixels);
            // 独立按单个波长重放相同的发射采样，检查顺序、落点与 1/3 权重。
            let single = render_landing_points(&scene, ground_id, 2, |ray| {
                take_photon(&scene, ray, prism_id, ground_id, || {
                    refractive_index(Some(wavelength_nm), IorMode::Dispersion)
                })
            })
            .unwrap();
            for (pixel, single_pixel) in image.pixels.iter().zip(single.pixels) {
                assert!(pixel.iter().all(|value| value.is_finite() && *value >= 0.0));
                assert!((pixel * 3.0 - single_pixel).norm() < 1e-12);
            }
        }
        let coarse = render_refracted_ground_hits(
            &scene,
            prism_id,
            ground_id,
            1,
            DEBUG_WAVELENGTHS_NM.map(Some),
            IorMode::Dispersion,
        )
        .unwrap();
        let sum_y = |images: &[Framebuffer; 3]| {
            images
                .iter()
                .flat_map(|image| &image.pixels)
                .map(|pixel| pixel.y)
                .sum::<f64>()
        };
        let emitted_y = 0.5 * (scene.width * scene.height) as f64;
        assert!(sum_y(&images) > 0.0 && sum_y(&images) <= emitted_y + 1e-10);
        assert!((sum_y(&images) - sum_y(&coarse)).abs() < emitted_y * 0.02);
    }

    #[test]
    fn unified_modes_share_sampling_and_constant_wavelengths_keep_total_flux() {
        let mut scene = Scene::new(64, 32);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let [single] =
            render_refracted_ground_hits(&scene, prism_id, ground_id, 2, [None], IorMode::Constant)
                .unwrap();
        let constant = render_refracted_ground_hits(
            &scene,
            prism_id,
            ground_id,
            2,
            DEBUG_WAVELENGTHS_NM.map(Some),
            IorMode::Constant,
        )
        .unwrap();
        let dispersion = render_refracted_ground_hits(
            &scene,
            prism_id,
            ground_id,
            2,
            DEBUG_WAVELENGTHS_NM.map(Some),
            IorMode::Dispersion,
        )
        .unwrap();
        assert_eq!(constant[0].pixels, constant[1].pixels);
        assert_eq!(constant[1].pixels, constant[2].pixels);
        for (constant_image, dispersion_image) in constant.iter().zip(&dispersion) {
            // 远处直射区域相同，玻璃折射区域随模式改变。
            assert_eq!(constant_image.get(0, 0), dispersion_image.get(0, 0));
            assert_ne!(constant_image.pixels, dispersion_image.pixels);
            for (pixel, single_pixel) in constant_image.pixels.iter().zip(&single.pixels) {
                assert!((pixel * 3.0 - single_pixel).norm() < 1e-12);
            }
        }
    }

    #[test]
    fn dispersion_render_rejects_missing_invalid_or_overflowing_wavelengths() {
        let mut scene = Scene::new(8, 4);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        for wavelength_nm in [
            None,
            Some(0.0),
            Some(-450.0),
            Some(f64::NAN),
            Some(f64::INFINITY),
            Some(f64::NEG_INFINITY),
            Some(f64::MIN_POSITIVE),
        ] {
            assert!(
                render_refracted_ground_hits(
                    &scene,
                    prism_id,
                    ground_id,
                    1,
                    [Some(450.0), wavelength_nm],
                    IorMode::Dispersion,
                )
                .is_err()
            );
            // 固定模式不读取波长，仍可使用同一个入口。
            assert!(
                render_refracted_ground_hits(
                    &scene,
                    prism_id,
                    ground_id,
                    1,
                    [wavelength_nm],
                    IorMode::Constant,
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn invalid_render_settings_return_errors() {
        let mut scene = Scene::new(8, 4);
        assert!(render_ground_hits(&scene, 0, 1).is_err());
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        assert!(render_ground_hits(&scene, ground_id, 0).is_err());
        assert!(render_ground_hits(&scene, ground_id, usize::MAX).is_err());
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        for (mode, wavelengths_nm) in [
            (IorMode::Constant, [None]),
            (IorMode::Dispersion, [Some(450.0)]),
        ] {
            for (prism_id, ground_id, samples) in [
                (usize::MAX, ground_id, 1),
                (ground_id, ground_id, 1),
                (prism_id, usize::MAX, 1),
                (prism_id, ground_id, 0),
                (prism_id, ground_id, usize::MAX),
            ] {
                assert!(
                    render_refracted_ground_hits(
                        &scene,
                        prism_id,
                        ground_id,
                        samples,
                        wavelengths_nm,
                        mode,
                    )
                    .is_err()
                );
            }
            assert!(
                render_refracted_ground_hits(&scene, prism_id, ground_id, 1, [], mode).is_err()
            );
        }
        assert!(render_landing_point_sets(&scene, ground_id, 1, |_| []).is_err());
        scene.width = 0;
        assert!(render_ground_hits(&scene, ground_id, 1).is_err());
        assert!(
            render_refracted_ground_hits(&scene, prism_id, ground_id, 1, [None], IorMode::Constant)
                .is_err()
        );
    }

    #[test]
    fn ground_hit_png_contains_gray_ground_and_black_shadow() -> Result<(), Box<dyn Error>> {
        let mut scene = Scene::new(32, 16);
        scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let framebuffer = render_ground_hits(&scene, ground_id, 2)?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("ground-hit-png-{}-{timestamp}", std::process::id()));
        let path = directory.join("ground_hits.png");
        save_png(&framebuffer, path.to_str().ok_or("临时路径必须是 UTF-8")?)?;
        let decoded = image::open(&path)?;
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        let pixels = decoded.into_rgb8();
        assert_eq!(pixels.dimensions(), (32, 16));
        assert_eq!(pixels.get_pixel(16, 8).0, [0, 0, 0]);
        assert!(pixels.get_pixel(0, 0)[0] > 0);
        assert!(
            pixels
                .pixels()
                .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2])
        );
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn refracted_render_redistributes_energy_without_creating_flux() {
        let mut scene = Scene::new(64, 32);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let [refracted] =
            render_refracted_ground_hits(&scene, prism_id, ground_id, 2, [None], IorMode::Constant)
                .unwrap();
        let mut reference_scene = Scene::new(scene.width, scene.height);
        let reference_ground_id = reference_scene.add(Box::new(Ground::new(0.0).unwrap()));
        let reference = render_ground_hits(&reference_scene, reference_ground_id, 2).unwrap();
        // 相同随机发射位置下，远处相同；部分折射落点与直射光叠加后更亮。
        assert_eq!(refracted.get(0, 0), reference.get(0, 0));
        assert!(
            refracted
                .pixels
                .iter()
                .zip(&reference.pixels)
                .any(|(a, b)| a.y > b.y + 1e-12)
        );
        assert!(refracted.pixels.iter().any(|pixel| pixel.y == 0.0));
        assert!(
            refracted
                .pixels
                .iter()
                .all(|pixel| pixel.iter().all(|v| v.is_finite() && *v >= 0.0))
        );
        let total_y: f64 = refracted.pixels.iter().map(|pixel| pixel.y).sum();
        let emitted_y = 0.5 * (scene.width * scene.height) as f64;
        assert!(total_y > 0.0 && total_y <= emitted_y + 1e-12);
        let [coarse] =
            render_refracted_ground_hits(&scene, prism_id, ground_id, 1, [None], IorMode::Constant)
                .unwrap();
        let coarse_y: f64 = coarse.pixels.iter().map(|pixel| pixel.y).sum();
        // 边界采样可能改变少量全反射分类，但四倍光线数不能使总能量接近四倍。
        assert!((total_y - coarse_y).abs() < emitted_y * 0.02);
    }

    #[test]
    fn refracted_png_preserves_grayscale_and_contains_brighter_landing_regions()
    -> Result<(), Box<dyn Error>> {
        let mut scene = Scene::new(64, 32);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let [framebuffer] = render_refracted_ground_hits(
            &scene,
            prism_id,
            ground_id,
            2,
            [None],
            IorMode::Constant,
        )?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("refracted-png-{}-{timestamp}", std::process::id()));
        let path = directory.join("refracted_hits.png");
        save_png(&framebuffer, path.to_str().ok_or("临时路径必须是 UTF-8")?)?;
        let decoded = image::open(&path)?;
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        let pixels = decoded.into_rgb8();
        assert_eq!(pixels.dimensions(), (64, 32));
        assert!(pixels.get_pixel(0, 0)[0] > 0);
        assert!(
            pixels
                .pixels()
                .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2])
        );
        assert!(pixels.pixels().any(|pixel| pixel[0] > 188));
        assert!(pixels.pixels().any(|pixel| pixel[0] == 0));
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}

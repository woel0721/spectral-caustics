//! 分层随机发射平行光，将地面落点累积为线性 XYZ。
//! 支持灰度落点调试，以及按 CIE/D65 光谱积分合成的彩色焦散。

use crate::{
    framebuffer::Framebuffer,
    iormode::IorMode,
    ray::Ray,
    scene::{Scene, refractive_index, take_photon_from_hit},
    spectrum::{d65_normalization, d65_spd, wavelength_to_xyz},
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

/// 追踪 380–780 nm、每 10 nm 一点的光线，落地后直接累积到同一张线性 XYZ 图。
/// 独立完成分层随机发射、空间通量归一化和双线性 splat，不生成逐波长灰度图。
/// D65 使用同一个 k；梯形积分两个端点取半宽度，丢失通量不重新分配。
/// 每条空间光线只查询一次首次交点；直接落地时将完整光谱合成后只 splat 一次。
pub fn render_spectral_caustics(
    scene: &Scene,
    prism_id: usize,
    ground_id: usize,
    samples_per_pixel_axis: usize,
    mode: IorMode,
) -> Result<Framebuffer, Box<dyn Error>> {
    if scene.width == 0 || scene.height == 0 || samples_per_pixel_axis == 0 {
        return Err("图像尺寸和每轴采样数必须大于零".into());
    }
    if prism_id == ground_id || prism_id >= scene.objects.len() {
        return Err("三棱镜编号无效或与地面编号相同".into());
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
    let total_flux = UNBLOCKED_BRIGHTNESS * area;
    let flux_per_ray = total_flux / ray_count as f64;
    let pixel_area = area / pixel_count as f64;
    let spatial_weight = flux_per_ray / pixel_area;

    // 每项为 (波长 nm, 积分宽度 nm)，宽度之和为 400 nm。
    let samples: [(f64, f64); 41] = std::array::from_fn(|index| {
        let wavelength_nm = 380.0 + index as f64 * 10.0;
        let delta_lambda_nm = if index == 0 || index == 40 { 5.0 } else { 10.0 };
        (wavelength_nm, delta_lambda_nm)
    });
    // 归一化只取决于完整发射光谱，在追踪/丢弃光子之前计算。
    let k = d65_normalization(&samples).ok_or("D65 光谱归一化失败")?;
    // 每项缓存 (玻璃折射率, 单条光线的 XYZ)，不在追踪内重复查表或计算 Cauchy。
    let mut contributions = [(0.0, Vector3::zeros()); 41];
    for ((wavelength_nm, delta_lambda_nm), contribution) in
        samples.into_iter().zip(&mut contributions)
    {
        let glass_ior = refractive_index(Some(wavelength_nm), mode).ok_or("折射率计算失败")?;
        let spectral_xyz = wavelength_to_xyz(
            wavelength_nm,
            d65_spd(wavelength_nm).ok_or("D65 光谱查询失败")?,
            delta_lambda_nm,
            k,
        )
        .ok_or("波长到 XYZ 转换失败")?;

        // spectral_xyz 已含 Δλ，不再除以 41；空间权重只按发射数和像素面积归一化。
        *contribution = (glass_ior, spectral_xyz * spatial_weight);
    }
    // 直射路径与波长无关，提前求和保留所有 Δλ 和空间通量权重。
    let direct_xyz = contributions
        .iter()
        .fold(Vector3::zeros(), |sum, &(_, xyz)| sum + xyz);

    let mut framebuffer = Framebuffer::new(scene.width, scene.height);
    let mut rng = StdRng::seed_from_u64(SAMPLING_SEED);
    for sample_y in 0..samples_y {
        for sample_x in 0..samples_x {
            // 同一空间光线的所有波长共享发射位置，每次仅消耗两个随机数。
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

            let Some(first_hit) = scene.intersect(&ray) else {
                continue;
            };
            if first_hit.object_id == Some(ground_id) {
                splat_spectral_xyz(&mut framebuffer, first_hit.coords, direct_xyz)?;
                continue;
            }
            if first_hit.object_id != Some(prism_id) {
                continue;
            }

            for &(glass_ior, xyz) in &contributions {
                let Some(position) =
                    take_photon_from_hit(scene, ray, first_hit, prism_id, ground_id, glass_ior)
                else {
                    continue;
                };
                // 落地即累积；丢失光子不重新分配通量。
                splat_spectral_xyz(&mut framebuffer, position, xyz)?;
            }
        }
    }
    Ok(framebuffer)
}

/// 把一份线性 XYZ 按地面落点分配到四邻域，接收区域外的通量直接丢弃。
fn splat_spectral_xyz(
    framebuffer: &mut Framebuffer,
    position: Vector3<f64>,
    xyz: Vector3<f64>,
) -> Result<(), Box<dyn Error>> {
    let Some((x, y)) = ground_to_pixel(position, framebuffer.width, framebuffer.height) else {
        return Ok(());
    };
    let neighbors = bilinear_weights(x, y, framebuffer.width, framebuffer.height)
        .ok_or("双线性权重计算失败")?;
    for (pixel_x, pixel_y, weight) in neighbors {
        if weight > 0.0 && !framebuffer.accumulate(pixel_x, pixel_y, xyz * weight) {
            return Err("光谱 XYZ 像素累积失败".into());
        }
    }
    Ok(())
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
        let Some(first_hit) = scene.intersect(&ray) else {
            return [None; N];
        };
        // N 张调试图复用直射落点，各自仍保留 1/N 通量。
        if first_hit.object_id == Some(ground_id) {
            return [Some(first_hit.coords); N];
        }
        if first_hit.object_id != Some(prism_id) {
            return [None; N];
        }
        wavelengths_nm.map(|wavelength_nm| {
            let glass_ior = refractive_index(wavelength_nm, mode)?;
            take_photon_from_hit(scene, ray, first_hit, prism_id, ground_id, glass_ior)
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
    use crate::{ground::Ground, prism::Prism, render::save_png, scene::take_photon};

    #[test]
    fn direct_blocked_and_missed_rays_query_the_scene_once_per_spatial_sample() {
        use std::{cell::Cell, rc::Rc};

        struct CountedGround {
            ground: Ground,
            calls: Rc<Cell<usize>>,
        }

        impl crate::object::Hittable for CountedGround {
            fn intersect(&self, ray: &Ray) -> Option<crate::hit::Hit> {
                self.calls.set(self.calls.get() + 1);
                self.ground.intersect(ray)
            }
        }

        // 三种场景分别覆盖直接落地、其他物体遮挡、所有物体均未命中。
        for case in 0..3 {
            let mut scene = Scene::new(6, 4);
            let prism_id = if case == 2 {
                scene.add(Box::new(Ground::new(4.0).unwrap()))
            } else {
                scene.add(Box::new(Prism::new().unwrap()))
            };
            let calls = Rc::new(Cell::new(0));
            let ground_id = scene.add(Box::new(CountedGround {
                ground: Ground::new(if case == 2 { 5.0 } else { 2.0 }).unwrap(),
                calls: calls.clone(),
            }));
            if case == 1 {
                scene.add(Box::new(Ground::new(2.5).unwrap()));
            }
            let samples_per_pixel_axis = 2;
            let spatial_ray_count =
                scene.width * scene.height * samples_per_pixel_axis * samples_per_pixel_axis;
            for mode in [IorMode::Constant, IorMode::Dispersion] {
                calls.set(0);
                let image = render_spectral_caustics(
                    &scene,
                    prism_id,
                    ground_id,
                    samples_per_pixel_axis,
                    mode,
                )
                .unwrap();
                assert_eq!(calls.get(), spatial_ray_count);
                assert_eq!(image.pixels.iter().any(|xyz| xyz.y > 0.0), case == 0);

                calls.set(0);
                let images = render_refracted_ground_hits(
                    &scene,
                    prism_id,
                    ground_id,
                    samples_per_pixel_axis,
                    DEBUG_WAVELENGTHS_NM.map(Some),
                    mode,
                )
                .unwrap();
                assert_eq!(calls.get(), spatial_ray_count);
                for image in images {
                    assert_eq!(image.pixels.iter().any(|xyz| xyz.y > 0.0), case == 0);
                }
            }
        }
    }

    #[test]
    fn spectral_unblocked_white_preserves_xyz_flux_across_spatial_sample_counts() {
        // 独立用官方 CSV 的 10 nm 梯形求和得到的参考白，Y = 1。
        let white_xyz = Vector3::new(0.9501416822353593, 1.0, 1.087976622255359);
        for (width, height) in [(1, 1), (1, 4), (4, 1), (12, 8)] {
            let mut scene = Scene::new(width, height);
            let prism_id = scene.add(Box::new(Prism::new().unwrap()));
            // 测试地面在三棱镜上方、光源下方，所有光线直接落地。
            let ground_id = scene.add(Box::new(Ground::new(2.0).unwrap()));
            for samples in [1, 3] {
                let gray = render_ground_hits(&scene, ground_id, samples).unwrap();
                for mode in [IorMode::Constant, IorMode::Dispersion] {
                    let image =
                        render_spectral_caustics(&scene, prism_id, ground_id, samples, mode)
                            .unwrap();
                    assert_eq!((image.width, image.height), (width, height));
                    // 每个像素的空间分布应与同种子的灰度双线性累积完全对应。
                    for (pixel, gray_pixel) in image.pixels.iter().zip(&gray.pixels) {
                        assert!((pixel - white_xyz * gray_pixel.y).norm() < 1e-12);
                    }
                    let total = image
                        .pixels
                        .iter()
                        .fold(Vector3::zeros(), |sum, pixel| sum + pixel);
                    let expected = white_xyz * (0.5 * (width * height) as f64);
                    assert!((total - expected).norm() < 1e-10);
                }
            }
        }
    }

    #[test]
    fn spectral_accumulation_matches_independent_single_wavelength_renders_with_lost_flux() {
        let mut scene = Scene::new(32, 16);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        // 在光源与三棱镜之间遮住一部分光，确保真的测试了丢失通量。
        scene.add(Box::new(
            crate::triangle::Triangle::new(
                Vector3::new(-2.0, -2.0, 2.0),
                Vector3::new(0.0, -2.0, 2.0),
                Vector3::new(-2.0, 2.0, 2.0),
            )
            .unwrap(),
        ));
        let samples: Vec<_> = (380..=780)
            .step_by(10)
            .map(|nm| (nm as f64, if nm == 380 || nm == 780 { 5.0 } else { 10.0 }))
            .collect();
        let k = d65_normalization(&samples).unwrap();
        let actual =
            render_spectral_caustics(&scene, prism_id, ground_id, 2, IorMode::Dispersion).unwrap();
        let mut expected = vec![Vector3::zeros(); scene.width * scene.height];
        let mut lost_flux = false;
        for (nm, width_nm) in samples {
            // 保留每个波长独立首次求交的旧路径，检查合并直射光后 XYZ 仍一致。
            let single = render_landing_points(&scene, ground_id, 2, |ray| {
                take_photon(&scene, ray, prism_id, ground_id, || {
                    refractive_index(Some(nm), IorMode::Dispersion)
                })
            })
            .unwrap();
            let emitted_y = 0.5 * (scene.width * scene.height) as f64;
            let landed_y: f64 = single.pixels.iter().map(|pixel| pixel.y).sum();
            lost_flux |= landed_y < emitted_y - 1e-6;
            let spectral_xyz = wavelength_to_xyz(nm, d65_spd(nm).unwrap(), width_nm, k).unwrap();
            for (sum, pixel) in expected.iter_mut().zip(single.pixels) {
                *sum += spectral_xyz * pixel.y;
            }
        }
        assert!(lost_flux, "测试场景必须有未落地或落在接收区外的通量");
        for (pixel, reference) in actual.pixels.iter().zip(expected) {
            assert!((pixel - reference).norm() < 1e-12);
        }
    }

    #[test]
    fn spectral_constant_stays_white_and_dispersion_separates_colors_reproducibly() {
        let mut scene = Scene::new(128, 16);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let constant =
            render_spectral_caustics(&scene, prism_id, ground_id, 2, IorMode::Constant).unwrap();
        let dispersion =
            render_spectral_caustics(&scene, prism_id, ground_id, 2, IorMode::Dispersion).unwrap();
        let repeated =
            render_spectral_caustics(&scene, prism_id, ground_id, 2, IorMode::Dispersion).unwrap();
        assert_eq!(dispersion.pixels, repeated.pixels);
        assert_ne!(constant.pixels, dispersion.pixels);
        assert_eq!(constant.get(0, 0), dispersion.get(0, 0));
        let white_xyz = Vector3::new(0.9501416822353593, 1.0, 1.087976622255359);
        assert!(
            constant
                .pixels
                .iter()
                .all(|pixel| { (pixel - white_xyz * pixel.y).norm() < 1e-11 })
        );
        assert!(
            dispersion
                .pixels
                .iter()
                .any(|pixel| { (pixel - white_xyz * pixel.y).norm() > 1e-3 })
        );
        let emitted_y = 0.5 * (scene.width * scene.height) as f64;
        for image in [&constant, &dispersion] {
            assert!(
                image
                    .pixels
                    .iter()
                    .all(|pixel| { pixel.iter().all(|value| value.is_finite() && *value >= 0.0) })
            );
            let total_y: f64 = image.pixels.iter().map(|pixel| pixel.y).sum();
            assert!(total_y > 0.0 && total_y <= emitted_y + 1e-9);
        }
    }

    #[test]
    fn spectral_png_roundtrip_has_neutral_constant_mode_and_colored_dispersion()
    -> Result<(), Box<dyn Error>> {
        let mut scene = Scene::new(128, 16);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("spectral-png-{}-{timestamp}", std::process::id()));
        for (mode, filename) in [
            (IorMode::Constant, "constant.png"),
            (IorMode::Dispersion, "dispersion.png"),
        ] {
            let image = render_spectral_caustics(&scene, prism_id, ground_id, 2, mode)?;
            let path = directory.join(filename);
            save_png(&image, path.to_str().ok_or("临时路径必须是 UTF-8")?)?;
            let decoded = image::open(&path)?;
            assert_eq!(decoded.color(), image::ColorType::Rgb8);
            let rgb = decoded.into_rgb8();
            assert_eq!(rgb.dimensions(), (128, 16));
            let channel_spread = |pixel: &image::Rgb<u8>| {
                pixel.0.iter().max().unwrap() - pixel.0.iter().min().unwrap()
            };
            match mode {
                // 有限波长范围和 10 nm 积分使 D65 白点存在微小误差，允许 1 字节差异。
                IorMode::Constant => assert!(rgb.pixels().all(|pixel| channel_spread(pixel) <= 1)),
                IorMode::Dispersion => assert!(rgb.pixels().any(|pixel| channel_spread(pixel) > 2)),
            }
        }
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn spectral_render_rejects_invalid_ids_dimensions_and_sampling_overflow() {
        let mut scene = Scene::new(8, 4);
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            for (prism_id, ground_id, samples) in [
                (usize::MAX, ground_id, 1),
                (ground_id, ground_id, 1),
                (prism_id, usize::MAX, 1),
                (prism_id, ground_id, 0),
                (prism_id, ground_id, usize::MAX),
            ] {
                assert!(
                    render_spectral_caustics(&scene, prism_id, ground_id, samples, mode).is_err()
                );
            }
        }
        for (width, height) in [
            (0, 4),
            (4, 0),
            (usize::MAX, 2),
            (usize::MAX, 1),
            (1, usize::MAX),
        ] {
            scene.width = width;
            scene.height = height;
            assert!(
                render_spectral_caustics(&scene, prism_id, ground_id, 2, IorMode::Dispersion)
                    .is_err()
            );
        }
    }

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

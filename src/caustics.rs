//! 分层随机发射平行光，将地面落点累积为线性 XYZ。
//! 支持灰度落点调试，以及按 CIE/D65 光谱积分合成的彩色焦散。

use crate::{
    framebuffer::Framebuffer,
    iormode::IorMode,
    ray::Ray,
    rendersetting::RenderSetting,
    scene::{GroundReceiver, Scene, refractive_index, take_photon_from_hit},
    spectrum::{d65_normalization, d65_spd, wavelength_to_xyz},
    utils::bilinear_weights,
};
use nalgebra::Vector3;
use rand::{Rng, SeedableRng, rngs::StdRng};
use rand_chacha::ChaCha12Rng;
use rayon::prelude::*;
use std::{error::Error, ops::Range};

/// 三张落点调试图对应的波长，单位为 nm；每个波长分配 1/3 总通量。
pub const DEBUG_WAVELENGTHS_NM: [f64; 3] = [450.0, 550.0, 650.0];

/// 将落点映射到配置的接收区域；世界 +x 朝图片右侧，+y 朝图片上方。
/// 接收边界为半开区间，整数像素坐标对应像素中心。
pub fn ground_to_pixel(
    position: Vector3<f64>,
    width: usize,
    height: usize,
    receiver: &GroundReceiver,
) -> Option<(f64, f64)> {
    if width == 0
        || height == 0
        || !position.iter().all(|v| v.is_finite())
        || !(receiver.min_m.x..receiver.max_m.x).contains(&position.x)
        || !(receiver.min_m.y..receiver.max_m.y).contains(&position.y)
    {
        return None;
    }
    let span = receiver.max_m - receiver.min_m;
    let u = (position.x - receiver.min_m.x) / span.x;
    let v = (position.y - receiver.min_m.y) / span.y;
    Some((u * width as f64 - 0.5, (1.0 - v) * height as f64 - 0.5))
}

/// Day 2 的直接命中灰度图；未先命中地面的光线被遮挡，不继续追踪。
pub fn render_ground_hits(
    scene: &Scene,
    ground_id: usize,
    settings: &RenderSetting,
) -> Result<Framebuffer, Box<dyn Error>> {
    render_landing_points(scene, ground_id, settings, |ray| {
        let hit = scene.intersect(&ray)?;
        (hit.object_id == Some(ground_id)).then_some(hit.coords)
    })
}

/// 按渲染设置的波长节点追踪，落地后直接累积到同一张线性 XYZ 图。
/// 独立完成分层随机发射、空间通量归一化和双线性 splat，不生成逐波长灰度图。
/// D65 使用同一个 k；梯形积分两个端点取半宽度，丢失通量不重新分配。
/// 每条空间光线只查询一次首次交点；直接落地时将完整光谱合成后只 splat 一次。
/// Constant 的玻璃路径也只追踪和 splat 一次，Dispersion 才逐波长继续追踪。
/// Rayon 并行追踪固定采样块，各块使用独立缓冲，全部完成后按块顺序合并 XYZ。
pub fn render_spectral_caustics(
    scene: &Scene,
    prism_id: Option<usize>,
    ground_id: usize,
    settings: &RenderSetting,
) -> Result<Framebuffer, Box<dyn Error>> {
    settings.validate()?;
    let mode = settings.mode;
    let samples_per_pixel_axis = settings.samples_per_pixel_axis;
    if settings.width == 0 || settings.height == 0 || samples_per_pixel_axis == 0 {
        return Err("图像尺寸和每轴采样数必须大于零".into());
    }
    if prism_id.is_some_and(|id| id == ground_id || id >= scene.objects.len()) {
        return Err("三棱镜编号无效或与地面编号相同".into());
    }
    if ground_id >= scene.objects.len() {
        return Err("地面编号不在场景中".into());
    }

    let pixel_count = settings
        .width
        .checked_mul(settings.height)
        .ok_or("像素数量溢出")?;
    let samples_x = settings
        .width
        .checked_mul(samples_per_pixel_axis)
        .ok_or("横向采样数溢出")?;
    let samples_y = settings
        .height
        .checked_mul(samples_per_pixel_axis)
        .ok_or("纵向采样数溢出")?;
    let ray_count = samples_x.checked_mul(samples_y).ok_or("光线数量溢出")?;

    let span = scene.light.aperture_max_m - scene.light.aperture_min_m;
    let area = scene.receiver.area();
    let total_flux = scene.light.total_flux;
    let flux_per_ray = total_flux / ray_count as f64;
    let pixel_area = area / pixel_count as f64;
    let spatial_weight = flux_per_ray / pixel_area;

    let samples = settings.wavelength_samples()?;
    // 归一化只取决于完整发射光谱，在追踪/丢弃光子之前计算。
    let k = d65_normalization(&samples).ok_or("D65 光谱归一化失败")?;
    // 固定模式只计算一次折射率；色散模式没有统一的玻璃折射率。
    let constant_ior = match mode {
        IorMode::Constant => Some(refractive_index(None, mode).ok_or("折射率计算失败")?),
        IorMode::Dispersion => None,
    };
    // 完整白光只在渲染前积分一次；仅色散模式缓存逐波长的 (折射率, XYZ)。
    let mut direct_xyz = Vector3::zeros();
    let mut contributions = Vec::new();
    for (wavelength_nm, delta_lambda_nm) in samples {
        let spectral_xyz = wavelength_to_xyz(
            wavelength_nm,
            d65_spd(wavelength_nm).ok_or("D65 光谱查询失败")?,
            delta_lambda_nm,
            k,
        )
        .ok_or("波长到 XYZ 转换失败")?;

        // spectral_xyz 已含 Δλ，不再除以波长样本数；空间权重按全图发射数和接收面积归一化。
        let xyz = spectral_xyz * spatial_weight;
        direct_xyz += xyz;
        if mode == IorMode::Dispersion {
            let glass_ior = refractive_index(Some(wavelength_nm), mode).ok_or("折射率计算失败")?;
            contributions.push((glass_ior, xyz));
        }
    }

    let partials: Vec<Result<Framebuffer, &'static str>> = spectral_sample_blocks(
        samples_x,
        samples_y,
        settings.seed,
        settings.max_spectral_blocks,
    )
    .into_par_iter()
    .map(|(rows, mut rng)| {
        let mut framebuffer = Framebuffer::new(settings.width, settings.height);
        for sample_y in rows {
            for sample_x in 0..samples_x {
                // 同一空间光线的所有波长共享发射位置，每次仅消耗两个随机数。
                let origin_x = scene.light.aperture_min_m.x
                    + (sample_x as f64 + rng.random::<f64>()) / samples_x as f64 * span.x;
                let origin_y = scene.light.aperture_min_m.y
                    + (sample_y as f64 + rng.random::<f64>()) / samples_y as f64 * span.y;
                let ray = Ray::new(
                    Vector3::new(origin_x, origin_y, scene.light.height_m),
                    scene.light.direction,
                    0.0,
                )
                .ok_or("平行光线构造失败")?;

                let Some(first_hit) = scene.intersect(&ray) else {
                    continue;
                };
                if first_hit.object_id == Some(ground_id) {
                    splat_spectral_xyz(
                        &mut framebuffer,
                        first_hit.coords,
                        direct_xyz,
                        &scene.receiver,
                    )?;
                    continue;
                }
                if first_hit.object_id != prism_id {
                    continue;
                }
                let Some(prism_id) = prism_id else {
                    continue;
                };

                if let Some(glass_ior) = constant_ior {
                    // 固定折射率下所有波长光路相同，完整白光沿这一条路径落地。
                    if let Some(position) =
                        take_photon_from_hit(scene, ray, first_hit, prism_id, ground_id, glass_ior)
                    {
                        splat_spectral_xyz(
                            &mut framebuffer,
                            position,
                            direct_xyz,
                            &scene.receiver,
                        )?;
                    }
                    continue;
                }

                for &(glass_ior, xyz) in &contributions {
                    let Some(position) =
                        take_photon_from_hit(scene, ray, first_hit, prism_id, ground_id, glass_ior)
                    else {
                        continue;
                    };
                    // 落地即累积；丢失光子不重新分配通量。
                    splat_spectral_xyz(&mut framebuffer, position, xyz, &scene.receiver)?;
                }
            }
        }
        Ok(framebuffer)
    })
    .collect();

    // Vec 按输入块的顺序收集。所有任务完成后再依次合并，不按线程完成顺序归约。
    let mut framebuffer = Framebuffer::new(settings.width, settings.height);
    for partial in partials {
        for (index, xyz) in partial?.pixels.into_iter().enumerate() {
            // 各块已经使用全图 ray_count 归一化，这里直接相加，不除以任务或线程数。
            if !framebuffer.accumulate(index % settings.width, index / settings.width, xyz) {
                return Err("并行光谱 XYZ 合并失败".into());
            }
        }
    }
    Ok(framebuffer)
}

/// 直接定位每块在原有随机序列中的起点，不遍历或生成块之前的随机数。
/// 当前锁定的 rand 0.9.5 的 StdRng 使用 ChaCha12；显式 RNG 提供 set_word_pos。
fn spectral_sample_blocks(
    samples_x: usize,
    samples_y: usize,
    seed: u64,
    max_blocks: usize,
) -> Vec<(Range<usize>, ChaCha12Rng)> {
    let rows_per_block = samples_y.div_ceil(max_blocks);
    let mut blocks = Vec::new();
    for start in (0..samples_y).step_by(rows_per_block) {
        let end = start.saturating_add(rows_per_block).min(samples_y);
        let mut rng = ChaCha12Rng::seed_from_u64(seed);
        // 一个 f64 消耗两个 u32 word，每条光线的 X/Y 两个偏移合计消耗四个。
        // u128 避免大采样数乘以 4 时在 usize 中溢出。
        let first_sample = start as u128 * samples_x as u128;
        rng.set_word_pos(first_sample * 4);
        blocks.push((start..end, rng));
    }
    blocks
}

/// 把一份线性 XYZ 按地面落点分配到四邻域，接收区域外的通量直接丢弃。
fn splat_spectral_xyz(
    framebuffer: &mut Framebuffer,
    position: Vector3<f64>,
    xyz: Vector3<f64>,
    receiver: &GroundReceiver,
) -> Result<(), &'static str> {
    let Some((x, y)) = ground_to_pixel(position, framebuffer.width, framebuffer.height, receiver)
    else {
        return Ok(());
    };
    let neighbors = bilinear_weights(x, y, framebuffer.width, framebuffer.height)
        .ok_or("双线性权重计算失败")?;
    for (pixel_x, pixel_y, weight) in neighbors {
        if weight > 0.0 && !framebuffer.accumulate(pixel_x, pixel_y, xyz * weight) {
            return Err("光谱 XYZ 像素累积失败");
        }
    }
    Ok(())
}

/// 统一追踪固定折射率或 Cauchy 色散，按波长数组顺序返回 N 张灰度落点图。
/// 固定模式传 [None] 可生成一张图；色散模式要求每项均为 Some(有效波长 nm)。
/// 同一条空间光线复用于全部波长，每张图分配 1/N 通量；灰色不代表真实光谱颜色。
/// Constant 只追踪一次路径，再把同一落点交给 N 张图。
pub fn render_refracted_ground_hits<const N: usize>(
    scene: &Scene,
    prism_id: Option<usize>,
    ground_id: usize,
    settings: &RenderSetting,
    wavelengths_nm: [Option<f64>; N],
    mode: IorMode,
) -> Result<[Framebuffer; N], Box<dyn Error>> {
    if prism_id.is_some_and(|id| id == ground_id || id >= scene.objects.len()) {
        return Err("三棱镜编号无效或与地面编号相同".into());
    }
    let constant_ior = match mode {
        IorMode::Constant => Some(refractive_index(None, mode).ok_or("折射率计算失败")?),
        IorMode::Dispersion => None,
    };
    if mode == IorMode::Dispersion
        && wavelengths_nm
            .iter()
            .any(|&wavelength_nm| refractive_index(wavelength_nm, mode).is_none())
    {
        return Err("色散模式要求每项波长均为正有限值，且折射率计算不能溢出".into());
    }
    render_landing_point_sets(scene, ground_id, settings, |ray| {
        let Some(first_hit) = scene.intersect(&ray) else {
            return [None; N];
        };
        // N 张调试图复用直射落点，各自仍保留 1/N 通量。
        if first_hit.object_id == Some(ground_id) {
            return [Some(first_hit.coords); N];
        }
        if first_hit.object_id != prism_id {
            return [None; N];
        }
        let Some(prism_id) = prism_id else {
            return [None; N];
        };
        if let Some(glass_ior) = constant_ior {
            let position =
                take_photon_from_hit(scene, ray, first_hit, prism_id, ground_id, glass_ior);
            return [position; N];
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
    settings: &RenderSetting,
    landing_point: impl Fn(Ray) -> Option<Vector3<f64>>,
) -> Result<Framebuffer, Box<dyn Error>> {
    let [framebuffer] =
        render_landing_point_sets(scene, ground_id, settings, |ray| [landing_point(ray)])?;
    Ok(framebuffer)
}

/// 每条空间光线返回 N 个独立落点，每个落点组使用一张图及 1/N 通量。
/// 每个发射小格按每轴 samples_per_pixel_axis 份细分，每层独立随机取一点。
/// 固定种子保证可复现；每条光线都先消耗两个随机数，再进行命中查询。
fn render_landing_point_sets<const N: usize>(
    scene: &Scene,
    ground_id: usize,
    settings: &RenderSetting,
    landing_points: impl Fn(Ray) -> [Option<Vector3<f64>>; N],
) -> Result<[Framebuffer; N], Box<dyn Error>> {
    settings.validate()?;
    let samples_per_pixel_axis = settings.samples_per_pixel_axis;
    if N == 0 || settings.width == 0 || settings.height == 0 || samples_per_pixel_axis == 0 {
        return Err("图像尺寸、每轴采样数和落点组数必须大于零".into());
    }
    if ground_id >= scene.objects.len() {
        return Err("地面编号不在场景中".into());
    }

    let pixel_count = settings
        .width
        .checked_mul(settings.height)
        .ok_or("像素数量溢出")?;
    let samples_x = settings
        .width
        .checked_mul(samples_per_pixel_axis)
        .ok_or("横向采样数溢出")?;
    let samples_y = settings
        .height
        .checked_mul(samples_per_pixel_axis)
        .ok_or("纵向采样数溢出")?;
    let ray_count = samples_x.checked_mul(samples_y).ok_or("光线数量溢出")?;

    let span = scene.light.aperture_max_m - scene.light.aperture_min_m;
    let area = scene.receiver.area();
    // 总通量由光源配置决定，按全图发射数和接收像素面积归一化。
    let total_flux = scene.light.total_flux;
    let flux_per_ray = total_flux / ray_count as f64;
    let pixel_area = area / pixel_count as f64;
    // 沿用 render.rs 中的 D65 参考白，仅生成已知灰色，不代表已实现光谱积分。
    let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
    // 调试光源只含这 N 个等权离散分量，权重和为 1；尚不做连续光谱积分。
    let wavelength_weight = 1.0 / N as f64;
    let contribution = white_xyz * (flux_per_ray / pixel_area * wavelength_weight);
    let mut framebuffers =
        std::array::from_fn(|_| Framebuffer::new(settings.width, settings.height));
    let mut rng = StdRng::seed_from_u64(settings.seed);

    for sample_y in 0..samples_y {
        for sample_x in 0..samples_x {
            // 分层不变，只把固定的 0.5 偏移改为每轴独立的 [0, 1) 随机偏移。
            let origin_x = scene.light.aperture_min_m.x
                + (sample_x as f64 + rng.random::<f64>()) / samples_x as f64 * span.x;
            let origin_y = scene.light.aperture_min_m.y
                + (sample_y as f64 + rng.random::<f64>()) / samples_y as f64 * span.y;
            let ray = Ray::new(
                Vector3::new(origin_x, origin_y, scene.light.height_m),
                scene.light.direction,
                0.0,
            )
            .ok_or("平行光线构造失败")?;

            for (framebuffer, position) in framebuffers.iter_mut().zip(landing_points(ray)) {
                // 某个波长未落地时只丢弃它的通量，其余波长继续累积，权重不重新归一化。
                let Some(position) = position else {
                    continue;
                };
                if let Some((x, y)) =
                    ground_to_pixel(position, settings.width, settings.height, &scene.receiver)
                {
                    let neighbors = bilinear_weights(x, y, settings.width, settings.height)
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
    const GROUND_MIN: f64 = -2.0;
    const GROUND_MAX: f64 = 2.0;
    const LIGHT_HEIGHT: f64 = 3.0;
    const SAMPLING_SEED: u64 = 42;
    const MAX_SPECTRAL_BLOCKS: usize = 16;

    fn settings_for_size(width: usize, height: usize) -> RenderSetting {
        RenderSetting {
            width,
            height,
            ..RenderSetting::default()
        }
    }

    fn sample_settings(base: &RenderSetting, samples: usize, mode: IorMode) -> RenderSetting {
        RenderSetting {
            samples_per_pixel_axis: samples,
            mode,
            ..base.clone()
        }
    }

    use super::*;
    use crate::{ground::Ground, prism::Prism, render::save_png, scene::take_photon};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct CountedGround {
        ground: Ground,
        calls: Arc<AtomicUsize>,
    }

    impl crate::object::Hittable for CountedGround {
        fn intersect(&self, ray: &Ray) -> Option<crate::hit::Hit> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.ground.intersect(ray)
        }
    }

    #[test]
    fn spectral_blocks_cover_every_row_and_replay_the_original_random_sequence() {
        for (samples_x, samples_y) in [(1, 1), (7, 5), (3, 17), (5, 33)] {
            let blocks =
                spectral_sample_blocks(samples_x, samples_y, SAMPLING_SEED, MAX_SPECTRAL_BLOCKS);
            assert!(blocks.len() <= MAX_SPECTRAL_BLOCKS);
            let mut serial_rng = StdRng::seed_from_u64(SAMPLING_SEED);
            let mut next_row = 0;
            for (rows, mut rng) in blocks {
                assert_eq!(rows.start, next_row);
                assert!(rows.end > rows.start);
                next_row = rows.end;
                for _ in rows {
                    for _ in 0..samples_x {
                        assert_eq!(rng.random::<f64>(), serial_rng.random::<f64>());
                        assert_eq!(rng.random::<f64>(), serial_rng.random::<f64>());
                    }
                }
            }
            assert_eq!(next_row, samples_y);
        }
    }

    #[test]
    fn spectral_xyz_is_identical_across_rayon_thread_counts_and_repeated_runs() {
        let settings = settings_for_size(19, 17);
        let mut scene = Scene::new();
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let serial_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            let reference = serial_pool.install(|| {
                render_spectral_caustics(
                    &scene,
                    Some(prism_id),
                    ground_id,
                    &sample_settings(&settings, 2, mode),
                )
                .unwrap()
            });
            for threads in [2, 4, 2] {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap();
                let image = pool.install(|| {
                    render_spectral_caustics(
                        &scene,
                        Some(prism_id),
                        ground_id,
                        &sample_settings(&settings, 2, mode),
                    )
                    .unwrap()
                });
                assert_eq!(image.pixels, reference.pixels);
            }
        }
    }

    #[test]
    fn direct_blocked_and_missed_rays_query_the_scene_once_per_spatial_sample() {
        // 三种场景分别覆盖直接落地、其他物体遮挡、所有物体均未命中。
        for case in 0..3 {
            let settings = settings_for_size(6, 4);
            let mut scene = Scene::new();
            let prism_id = if case == 2 {
                scene.add(Box::new(Ground::new(4.0).unwrap()))
            } else {
                scene.add(Box::new(Prism::new().unwrap()))
            };
            let calls = Arc::new(AtomicUsize::new(0));
            let ground_id = scene.add(Box::new(CountedGround {
                ground: Ground::new(if case == 2 { 5.0 } else { 2.0 }).unwrap(),
                calls: calls.clone(),
            }));
            if case == 1 {
                scene.add(Box::new(Ground::new(2.5).unwrap()));
            }
            let samples_per_pixel_axis = 2;
            let spatial_ray_count =
                settings.width * settings.height * samples_per_pixel_axis * samples_per_pixel_axis;
            for mode in [IorMode::Constant, IorMode::Dispersion] {
                calls.store(0, Ordering::Relaxed);
                let image = render_spectral_caustics(
                    &scene,
                    Some(prism_id),
                    ground_id,
                    &sample_settings(&settings, samples_per_pixel_axis, mode),
                )
                .unwrap();
                assert_eq!(calls.load(Ordering::Relaxed), spatial_ray_count);
                assert_eq!(image.pixels.iter().any(|xyz| xyz.y > 0.0), case == 0);

                calls.store(0, Ordering::Relaxed);
                let images = render_refracted_ground_hits(
                    &scene,
                    Some(prism_id),
                    ground_id,
                    &sample_settings(&settings, samples_per_pixel_axis, mode),
                    DEBUG_WAVELENGTHS_NM.map(Some),
                    mode,
                )
                .unwrap();
                assert_eq!(calls.load(Ordering::Relaxed), spatial_ray_count);
                for image in images {
                    assert_eq!(image.pixels.iter().any(|xyz| xyz.y > 0.0), case == 0);
                }
            }
        }
    }

    #[test]
    fn constant_spectral_and_debug_renders_trace_one_path_per_spatial_sample() {
        let settings = settings_for_size(24, 12);
        let mut scene = Scene::new();
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let calls = Arc::new(AtomicUsize::new(0));
        let ground_id = scene.add(Box::new(CountedGround {
            ground: Ground::new(0.0).unwrap(),
            calls: calls.clone(),
        }));
        // 独立的单路径参考：每条空间光线只调用一次 take_photon。
        let reference = render_landing_points(
            &scene,
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
            |ray| take_photon(&scene, ray, prism_id, ground_id, || Some(1.52)),
        )
        .unwrap();
        let single_path_calls = calls.load(Ordering::Relaxed);
        assert!(single_path_calls > settings.width * settings.height * 4);

        calls.store(0, Ordering::Relaxed);
        let image = render_spectral_caustics(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
        )
        .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), single_path_calls);
        let white_xyz = Vector3::new(0.9501416822353593, 1.0, 1.087976622255359);
        for (pixel, gray_pixel) in image.pixels.iter().zip(reference.pixels) {
            assert!((pixel - white_xyz * gray_pixel.y).norm() < 1e-12);
        }
        let repeated = render_spectral_caustics(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
        )
        .unwrap();
        assert_eq!(image.pixels, repeated.pixels);

        calls.store(0, Ordering::Relaxed);
        let images = render_refracted_ground_hits(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
            [None, Some(450.0), Some(f64::NAN)],
            IorMode::Constant,
        )
        .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), single_path_calls);
        assert_eq!(images[0].pixels, images[1].pixels);
        assert_eq!(images[1].pixels, images[2].pixels);

        calls.store(0, Ordering::Relaxed);
        render_spectral_caustics(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Dispersion),
        )
        .unwrap();
        assert!(calls.load(Ordering::Relaxed) > single_path_calls);
    }

    #[test]
    fn spectral_unblocked_white_preserves_xyz_flux_across_spatial_sample_counts() {
        // 独立用官方 CSV 的 10 nm 梯形求和得到的参考白，Y = 1。
        let white_xyz = Vector3::new(0.9501416822353593, 1.0, 1.087976622255359);
        for (width, height) in [(1, 1), (1, 4), (4, 1), (12, 8)] {
            let settings = settings_for_size(width, height);
            let mut scene = Scene::new();
            let prism_id = scene.add(Box::new(Prism::new().unwrap()));
            // 测试地面在三棱镜上方、光源下方，所有光线直接落地。
            let ground_id = scene.add(Box::new(Ground::new(2.0).unwrap()));
            for samples in [1, 3] {
                let gray = render_ground_hits(
                    &scene,
                    ground_id,
                    &sample_settings(&settings, samples, IorMode::Constant),
                )
                .unwrap();
                for mode in [IorMode::Constant, IorMode::Dispersion] {
                    let image = render_spectral_caustics(
                        &scene,
                        Some(prism_id),
                        ground_id,
                        &sample_settings(&settings, samples, mode),
                    )
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
        let settings = settings_for_size(32, 16);
        let mut scene = Scene::new();
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
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            let actual = render_spectral_caustics(
                &scene,
                Some(prism_id),
                ground_id,
                &sample_settings(&settings, 2, mode),
            )
            .unwrap();
            let mut expected = vec![Vector3::zeros(); settings.width * settings.height];
            let mut lost_flux = false;
            for &(nm, width_nm) in &samples {
                // 保留逐波长独立追踪的旧路径，检查合并直射和固定光路后 XYZ 仍一致。
                let single = render_landing_points(
                    &scene,
                    ground_id,
                    &sample_settings(&settings, 2, IorMode::Constant),
                    |ray| {
                        take_photon(&scene, ray, prism_id, ground_id, || {
                            refractive_index(Some(nm), mode)
                        })
                    },
                )
                .unwrap();
                let emitted_y = 0.5 * (settings.width * settings.height) as f64;
                let landed_y: f64 = single.pixels.iter().map(|pixel| pixel.y).sum();
                lost_flux |= landed_y < emitted_y - 1e-6;
                let spectral_xyz =
                    wavelength_to_xyz(nm, d65_spd(nm).unwrap(), width_nm, k).unwrap();
                for (sum, pixel) in expected.iter_mut().zip(single.pixels) {
                    *sum += spectral_xyz * pixel.y;
                }
            }
            assert!(lost_flux, "测试场景必须有未落地或落在接收区外的通量");
            for (pixel, reference) in actual.pixels.iter().zip(expected) {
                assert!((pixel - reference).norm() < 1e-12);
            }
        }
    }

    #[test]
    fn spectral_constant_stays_white_and_dispersion_separates_colors_reproducibly() {
        let settings = settings_for_size(128, 16);
        let mut scene = Scene::new();
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let constant = render_spectral_caustics(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
        )
        .unwrap();
        let dispersion = render_spectral_caustics(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Dispersion),
        )
        .unwrap();
        let repeated = render_spectral_caustics(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Dispersion),
        )
        .unwrap();
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
        let emitted_y = 0.5 * (settings.width * settings.height) as f64;
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
        let settings = settings_for_size(128, 16);
        let mut scene = Scene::new();
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
            let image = render_spectral_caustics(
                &scene,
                Some(prism_id),
                ground_id,
                &sample_settings(&settings, 2, mode),
            )?;
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
        let mut settings = settings_for_size(8, 4);
        let mut scene = Scene::new();
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
                    render_spectral_caustics(
                        &scene,
                        Some(prism_id),
                        ground_id,
                        &sample_settings(&settings, samples, mode)
                    )
                    .is_err()
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
            settings.width = width;
            settings.height = height;
            assert!(
                render_spectral_caustics(
                    &scene,
                    Some(prism_id),
                    ground_id,
                    &sample_settings(&settings, 2, IorMode::Dispersion)
                )
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
            let actual = ground_to_pixel(position, 3, 2, &Scene::new().receiver).unwrap();
            assert!((actual.0 - expected.0).abs() < 1e-12);
            assert!((actual.1 - expected.1).abs() < 1e-12);
        }
        assert_eq!(
            ground_to_pixel(Vector3::new(-1.5, 1.0, 0.0), 4, 2, &Scene::new().receiver),
            Some((0.0, 0.0))
        );
        assert_eq!(
            ground_to_pixel(Vector3::new(1.5, -1.0, 0.0), 4, 2, &Scene::new().receiver),
            Some((3.0, 1.0))
        );
    }

    #[test]
    fn mapping_rejects_outside_non_finite_and_empty_inputs() {
        for value in [-2.01, 2.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                ground_to_pixel(Vector3::new(value, 0.0, 0.0), 3, 2, &Scene::new().receiver),
                None
            );
            assert_eq!(
                ground_to_pixel(Vector3::new(0.0, value, 0.0), 3, 2, &Scene::new().receiver),
                None
            );
        }
        assert_eq!(
            ground_to_pixel(
                Vector3::new(0.0, 0.0, f64::NAN),
                3,
                2,
                &Scene::new().receiver
            ),
            None
        );
        assert_eq!(
            ground_to_pixel(Vector3::zeros(), 0, 2, &Scene::new().receiver),
            None
        );
        assert_eq!(
            ground_to_pixel(Vector3::zeros(), 3, 0, &Scene::new().receiver),
            None
        );
    }

    #[test]
    fn unblocked_ground_preserves_total_energy_and_more_samples_reduce_noise() {
        let settings = settings_for_size(32, 16);
        let mut scene = Scene::new();
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let coarse = render_ground_hits(
            &scene,
            ground_id,
            &sample_settings(&settings, 1, IorMode::Constant),
        )
        .unwrap();
        let fine = render_ground_hits(
            &scene,
            ground_id,
            &sample_settings(&settings, 3, IorMode::Constant),
        )
        .unwrap();
        let expected_total = 0.5 * (settings.width * settings.height) as f64;
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

        let settings = settings_for_size(4, 3);
        let mut scene = Scene::new();
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let record = || {
            let origins = RefCell::new(Vec::new());
            let image = render_landing_points(
                &scene,
                ground_id,
                &sample_settings(&settings, 2, IorMode::Constant),
                |ray| {
                    origins.borrow_mut().push(ray.origin);
                    Some(Vector3::new(ray.origin.x, ray.origin.y, 0.0))
                },
            )
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
        let settings = settings_for_size(16, 8);
        let mut reference_scene = Scene::new();
        let reference_ground_id = reference_scene.add(Box::new(Ground::new(0.0).unwrap()));
        let reference = render_ground_hits(
            &reference_scene,
            reference_ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
        )
        .unwrap();
        let mut previous_image = None;
        for ground_first in [false, true] {
            let mut scene = Scene::new();
            if !ground_first {
                scene.add(Box::new(Prism::new().unwrap()));
            }
            let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
            if ground_first {
                scene.add(Box::new(Prism::new().unwrap()));
            }
            let framebuffer = render_ground_hits(
                &scene,
                ground_id,
                &sample_settings(&settings, 2, IorMode::Constant),
            )
            .unwrap();
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
        let settings = settings_for_size(4, 3);
        let mut scene = Scene::new();
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        // 此世界位置映射到连续像素 (1.25, 0.75)，让所有样本都落在这里。
        let position = Vector3::new(-0.25, 1.0 / 3.0, 0.0);
        let framebuffer = render_landing_points(
            &scene,
            ground_id,
            &sample_settings(&settings, 1, IorMode::Constant),
            |_| Some(position),
        )
        .unwrap();
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
            let settings = settings_for_size(width, height);
            let mut scene = Scene::new();
            let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
            let framebuffer = render_landing_points(
                &scene,
                ground_id,
                &sample_settings(&settings, 2, IorMode::Constant),
                |_| Some(Vector3::new(-2.0, -2.0, 0.0)),
            )
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

        let settings = settings_for_size(16, 8);
        let mut scene = Scene::new();
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
        let expected = white_xyz * (0.5 * (settings.width * settings.height) as f64);
        for samples in [1, 3] {
            let calls = Cell::new(0);
            let images = render_landing_point_sets(
                &scene,
                ground_id,
                &sample_settings(&settings, samples, IorMode::Constant),
                |ray| {
                    calls.set(calls.get() + 1);
                    [Some(Vector3::new(ray.origin.x, ray.origin.y, 0.0)); 3]
                },
            )
            .unwrap();
            assert_eq!(
                calls.get(),
                settings.width * settings.height * samples * samples
            );
            let reference = render_ground_hits(
                &scene,
                ground_id,
                &sample_settings(&settings, samples, IorMode::Constant),
            )
            .unwrap();
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
        let settings = settings_for_size(4, 3);
        let mut scene = Scene::new();
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        for samples in [1, 4] {
            let images = render_landing_point_sets(
                &scene,
                ground_id,
                &sample_settings(&settings, samples, IorMode::Constant),
                |_| {
                    [
                        None,
                        Some(Vector3::new(0.0, 0.0, 0.0)),
                        Some(Vector3::new(3.0, 0.0, 0.0)),
                    ]
                },
            )
            .unwrap();
            let totals = images.map(|image| image.pixels.iter().map(|pixel| pixel.y).sum::<f64>());
            assert_eq!(totals[0], 0.0);
            assert!((totals[1] - 2.0).abs() < 1e-12);
            assert_eq!(totals[2], 0.0);
        }
    }

    #[test]
    fn wavelength_images_match_single_wavelength_paths_and_repeat() {
        let settings = settings_for_size(64, 32);
        let mut scene = Scene::new();
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let images = render_refracted_ground_hits(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Dispersion),
            DEBUG_WAVELENGTHS_NM.map(Some),
            IorMode::Dispersion,
        )
        .unwrap();
        let repeated = render_refracted_ground_hits(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Dispersion),
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
            let single = render_landing_points(
                &scene,
                ground_id,
                &sample_settings(&settings, 2, IorMode::Constant),
                |ray| {
                    take_photon(&scene, ray, prism_id, ground_id, || {
                        refractive_index(Some(wavelength_nm), IorMode::Dispersion)
                    })
                },
            )
            .unwrap();
            for (pixel, single_pixel) in image.pixels.iter().zip(single.pixels) {
                assert!(pixel.iter().all(|value| value.is_finite() && *value >= 0.0));
                assert!((pixel * 3.0 - single_pixel).norm() < 1e-12);
            }
        }
        let coarse = render_refracted_ground_hits(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 1, IorMode::Dispersion),
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
        let emitted_y = 0.5 * (settings.width * settings.height) as f64;
        assert!(sum_y(&images) > 0.0 && sum_y(&images) <= emitted_y + 1e-10);
        assert!((sum_y(&images) - sum_y(&coarse)).abs() < emitted_y * 0.02);
    }

    #[test]
    fn unified_modes_share_sampling_and_constant_wavelengths_keep_total_flux() {
        let settings = settings_for_size(64, 32);
        let mut scene = Scene::new();
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let [single] = render_refracted_ground_hits(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
            [None],
            IorMode::Constant,
        )
        .unwrap();
        let constant = render_refracted_ground_hits(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
            DEBUG_WAVELENGTHS_NM.map(Some),
            IorMode::Constant,
        )
        .unwrap();
        let dispersion = render_refracted_ground_hits(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Dispersion),
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
        let settings = settings_for_size(8, 4);
        let mut scene = Scene::new();
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
                    Some(prism_id),
                    ground_id,
                    &sample_settings(&settings, 1, IorMode::Dispersion),
                    [Some(450.0), wavelength_nm],
                    IorMode::Dispersion
                )
                .is_err()
            );
            // 固定模式不读取波长，仍可使用同一个入口。
            assert!(
                render_refracted_ground_hits(
                    &scene,
                    Some(prism_id),
                    ground_id,
                    &sample_settings(&settings, 1, IorMode::Constant),
                    [wavelength_nm],
                    IorMode::Constant
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn invalid_render_settings_return_errors() {
        let mut settings = settings_for_size(8, 4);
        let mut scene = Scene::new();
        assert!(
            render_ground_hits(&scene, 0, &sample_settings(&settings, 1, IorMode::Constant))
                .is_err()
        );
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        assert!(
            render_ground_hits(
                &scene,
                ground_id,
                &sample_settings(&settings, 0, IorMode::Constant)
            )
            .is_err()
        );
        assert!(
            render_ground_hits(
                &scene,
                ground_id,
                &sample_settings(&settings, usize::MAX, IorMode::Constant)
            )
            .is_err()
        );
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
                        Some(prism_id),
                        ground_id,
                        &sample_settings(&settings, samples, mode),
                        wavelengths_nm,
                        mode
                    )
                    .is_err()
                );
            }
            assert!(
                render_refracted_ground_hits(
                    &scene,
                    Some(prism_id),
                    ground_id,
                    &sample_settings(&settings, 1, mode),
                    [],
                    mode
                )
                .is_err()
            );
        }
        assert!(
            render_landing_point_sets(
                &scene,
                ground_id,
                &sample_settings(&settings, 1, IorMode::Constant),
                |_| []
            )
            .is_err()
        );
        settings.width = 0;
        assert!(
            render_ground_hits(
                &scene,
                ground_id,
                &sample_settings(&settings, 1, IorMode::Constant)
            )
            .is_err()
        );
        assert!(
            render_refracted_ground_hits(
                &scene,
                Some(prism_id),
                ground_id,
                &sample_settings(&settings, 1, IorMode::Constant),
                [None],
                IorMode::Constant
            )
            .is_err()
        );
    }

    #[test]
    fn ground_hit_png_contains_gray_ground_and_black_shadow() -> Result<(), Box<dyn Error>> {
        let settings = settings_for_size(32, 16);
        let mut scene = Scene::new();
        scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let framebuffer = render_ground_hits(
            &scene,
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
        )?;
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
        let settings = settings_for_size(64, 32);
        let mut scene = Scene::new();
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let [refracted] = render_refracted_ground_hits(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
            [None],
            IorMode::Constant,
        )
        .unwrap();
        let mut reference_scene = Scene::new();
        let reference_ground_id = reference_scene.add(Box::new(Ground::new(0.0).unwrap()));
        let reference = render_ground_hits(
            &reference_scene,
            reference_ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
        )
        .unwrap();
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
        let emitted_y = 0.5 * (settings.width * settings.height) as f64;
        assert!(total_y > 0.0 && total_y <= emitted_y + 1e-12);
        let [coarse] = render_refracted_ground_hits(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 1, IorMode::Constant),
            [None],
            IorMode::Constant,
        )
        .unwrap();
        let coarse_y: f64 = coarse.pixels.iter().map(|pixel| pixel.y).sum();
        // 边界采样可能改变少量全反射分类，但四倍光线数不能使总能量接近四倍。
        assert!((total_y - coarse_y).abs() < emitted_y * 0.02);
    }

    #[test]
    fn refracted_png_preserves_grayscale_and_contains_brighter_landing_regions()
    -> Result<(), Box<dyn Error>> {
        let settings = settings_for_size(64, 32);
        let mut scene = Scene::new();
        let prism_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let [framebuffer] = render_refracted_ground_hits(
            &scene,
            Some(prism_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
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

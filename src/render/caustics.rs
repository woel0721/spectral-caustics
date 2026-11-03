//! 分层随机发射平行光，将地面落点累积为线性 XYZ。
//! CIE/D65 光谱积分与 Fresnel 加权的直射/焦散分量。

use super::{framebuffer::Framebuffer, utils::bilinear_weights};
use crate::{
    config::rendersetting::RenderSetting,
    geometry::ray::Ray,
    optics::{
        iormode::IorMode,
        spectrum::{d65_normalization, d65_spd, wavelength_to_xyz},
    },
    scene::{GroundReceiver, Scene, take_photon_from_hit},
};
use nalgebra::Vector3;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;
use rayon::prelude::*;
use std::{error::Error, ops::Range};

/// 同一批光源样本的两种落地贡献，均为未曝光的 XYZ 辐照度。
/// direct 用于光源通量检查；相机直射使用无限远太阳，不读取 direct 图。
pub struct SpectralLighting {
    pub direct: Framebuffer,
    pub caustics: Framebuffer,
    /// 生成图时的坐标范围，只限制焦散数据查询。
    pub receiver: GroundReceiver,
    /// 指向太阳的单位向量。
    pub light_wi: Vector3<f64>,
    /// 光束法向辐照度，使用与光子相同的 D65/波长积分和相对通量标尺。
    pub sun_irradiance_xyz: Vector3<f64>,
}

impl SpectralLighting {
    /// 地面总辐照度预览，在线性 XYZ 中相加。
    /// 此图不能再与解析直射叠加。
    pub fn combined(&self) -> Result<Framebuffer, Box<dyn Error>> {
        let a = &self.direct;
        let b = &self.caustics;
        if a.width == 0
            || a.height == 0
            || a.width != b.width
            || a.height != b.height
            || a.width.checked_mul(a.height) != Some(a.pixels.len())
            || a.pixels.len() != b.pixels.len()
        {
            return Err("拆分地面图的尺寸不匹配".into());
        }
        let mut result = Framebuffer::new(a.width, a.height);
        for (index, (direct, caustics)) in a.pixels.iter().zip(&b.pixels).enumerate() {
            if direct
                .iter()
                .chain(caustics.iter())
                .any(|v| !v.is_finite() || *v < 0.0)
                || !result.set(index % a.width, index / a.width, direct + caustics)
            {
                return Err("拆分地面 XYZ 合并失败".into());
            }
        }
        Ok(result)
    }
}

// 按首次落地/双界面透射分别累积，合并入口直接相加。
struct SpectralBuffers {
    direct: Framebuffer,
    caustics: Framebuffer,
}

impl SpectralBuffers {
    fn new(width: usize, height: usize) -> Self {
        Self {
            direct: Framebuffer::new(width, height),
            caustics: Framebuffer::new(width, height),
        }
    }

    fn target(&mut self, caustic: bool) -> &mut Framebuffer {
        if caustic {
            &mut self.caustics
        } else {
            &mut self.direct
        }
    }
}

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

/// 按渲染设置的波长节点追踪，落地后直接累积到同一张线性 XYZ 图。
/// 独立完成分层随机发射、空间通量归一化和双线性 splat，不生成逐波长灰度图。
/// D65 使用同一个 k；梯形积分两个端点取半宽度，丢失通量不重新分配。
/// 每条空间光线只查询一次首次交点；直接落地时将完整光谱合成后只 splat 一次。
/// Constant 的玻璃路径也只追踪和 splat 一次，Dispersion 才逐波长继续追踪。
/// Rayon 并行追踪固定采样块，各块使用独立缓冲，全部完成后按块顺序合并 XYZ。
pub fn render_spectral_caustics(
    scene: &Scene,
    caustics_object_id: Option<usize>,
    ground_id: usize,
    settings: &RenderSetting,
) -> Result<Framebuffer, Box<dyn Error>> {
    render_spectral_lighting(scene, caustics_object_id, ground_id, settings)?.combined()
}

/// 一次追踪拆分 Direct（首次命中地面）与 Caustics（经所选介质双界面后落地）。
/// 折射落点乘双界面 Fresnel 透射通量比例；解析太阳的强度不依赖 receiver。
pub fn render_spectral_lighting(
    scene: &Scene,
    caustics_object_id: Option<usize>,
    ground_id: usize,
    settings: &RenderSetting,
) -> Result<SpectralLighting, Box<dyn Error>> {
    let (light_wi, strength) = scene
        .light
        .sunlight()
        .ok_or("太阳光要求有效的向下方向、窗口与通量")?;
    let receiver = scene.receiver;
    let span = receiver.max_m - receiver.min_m;
    if receiver
        .min_m
        .iter()
        .chain(receiver.max_m.iter())
        .any(|v| !v.is_finite())
        || span.iter().any(|v| !v.is_finite() || *v <= 0.0)
        || !receiver.area().is_finite()
        || receiver.area() <= 0.0
    {
        return Err("焦散接收范围无效".into());
    }
    let (buffers, white_xyz) =
        render_spectral_buffers(scene, caustics_object_id, ground_id, settings)?;
    let sun_irradiance_xyz = white_xyz * strength;
    if sun_irradiance_xyz.iter().any(|v| !v.is_finite()) {
        return Err("太阳光 XYZ 强度溢出".into());
    }
    Ok(SpectralLighting {
        direct: buffers.direct,
        caustics: buffers.caustics,
        receiver,
        light_wi,
        sun_irradiance_xyz,
    })
}

fn render_spectral_buffers(
    scene: &Scene,
    caustics_object_id: Option<usize>,
    ground_id: usize,
    settings: &RenderSetting,
) -> Result<(SpectralBuffers, Vector3<f64>), Box<dyn Error>> {
    settings.validate()?;
    let mode = settings.mode;
    let samples_per_pixel_axis = settings.samples_per_pixel_axis;
    if settings.width == 0 || settings.height == 0 || samples_per_pixel_axis == 0 {
        return Err("图像尺寸和每轴采样数必须大于零".into());
    }
    if caustics_object_id.is_some_and(|id| id == ground_id || id >= scene.objects.len()) {
        return Err("焦散物体编号无效或与地面编号相同".into());
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
    // 显式选择的非介质仍作二值遮挡，不生成光谱焦散。
    let caustics_object_id = caustics_object_id.filter(|&id| scene.supports_ideal_caustics(id));
    let constant_ior = match mode {
        IorMode::Constant => caustics_object_id
            .map(|id| scene.photon_ior(id, None, mode).ok_or("材质折射率计算失败"))
            .transpose()?,
        IorMode::Dispersion => None,
    };
    // 完整白光只在渲染前积分一次；仅色散模式缓存逐波长的 (折射率, XYZ)。
    let mut direct_xyz = Vector3::zeros();
    let mut white_xyz = Vector3::zeros();
    let mut contributions = Vec::new();
    for (wavelength_nm, delta_lambda_nm) in samples {
        let spectral_xyz = wavelength_to_xyz(
            wavelength_nm,
            d65_spd(wavelength_nm).ok_or("D65 光谱查询失败")?,
            delta_lambda_nm,
            k,
        )
        .ok_or("波长到 XYZ 转换失败")?;
        white_xyz += spectral_xyz;

        // spectral_xyz 已含 Δλ，不再除以波长样本数；空间权重按全图发射数和接收面积归一化。
        let xyz = spectral_xyz * spatial_weight;
        direct_xyz += xyz;
        if let Some(id) = caustics_object_id.filter(|_| mode == IorMode::Dispersion) {
            let glass_ior = scene
                .photon_ior(id, Some(wavelength_nm), mode)
                .ok_or("材质光谱折射率计算失败")?;
            contributions.push((glass_ior, xyz));
        }
    }

    let partials: Vec<Result<SpectralBuffers, &'static str>> = spectral_sample_blocks(
        samples_x,
        samples_y,
        settings.seed,
        settings.max_spectral_blocks,
    )
    .into_par_iter()
    .map(|(rows, mut rng)| {
        let mut buffers = SpectralBuffers::new(settings.width, settings.height);
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
                )
                .ok_or("平行光线构造失败")?;

                let Some(first_hit) = scene.intersect(&ray) else {
                    continue;
                };
                if first_hit.object_id == Some(ground_id) {
                    splat_spectral_xyz(
                        buffers.target(false),
                        first_hit.coords,
                        direct_xyz,
                        &scene.receiver,
                    )?;
                    continue;
                }
                if first_hit.object_id != caustics_object_id {
                    continue;
                }
                let Some(caustics_object_id) = caustics_object_id else {
                    continue;
                };

                if let Some(glass_ior) = constant_ior {
                    // 固定折射率下所有波长光路相同，完整白光沿这一条路径落地。
                    if let Some(landing) = take_photon_from_hit(
                        scene,
                        ray,
                        first_hit,
                        caustics_object_id,
                        ground_id,
                        glass_ior,
                    ) {
                        splat_spectral_xyz(
                            buffers.target(true),
                            landing.position,
                            direct_xyz * landing.weight,
                            &scene.receiver,
                        )?;
                    }
                    continue;
                }

                for &(glass_ior, xyz) in &contributions {
                    let Some(landing) = take_photon_from_hit(
                        scene,
                        ray,
                        first_hit,
                        caustics_object_id,
                        ground_id,
                        glass_ior,
                    ) else {
                        continue;
                    };
                    // 落地即累积；丢失光子不重新分配通量。
                    splat_spectral_xyz(
                        buffers.target(true),
                        landing.position,
                        xyz * landing.weight,
                        &scene.receiver,
                    )?;
                }
            }
        }
        Ok(buffers)
    })
    .collect();

    // Vec 按输入块的顺序收集。所有任务完成后再依次合并，不按线程完成顺序归约。
    let mut buffers = SpectralBuffers::new(settings.width, settings.height);
    for partial in partials {
        let partial = partial?;
        merge_spectral_buffer(&mut buffers.direct, partial.direct)?;
        merge_spectral_buffer(&mut buffers.caustics, partial.caustics)?;
    }
    Ok((buffers, white_xyz))
}

fn merge_spectral_buffer(
    target: &mut Framebuffer,
    source: Framebuffer,
) -> Result<(), &'static str> {
    for (index, xyz) in source.pixels.into_iter().enumerate() {
        // 每块已按全图 ray_count 归一化，不除以任务或线程数。
        if !target.accumulate(index % target.width, index / target.width, xyz) {
            return Err("并行光谱 XYZ 合并失败");
        }
    }
    Ok(())
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
    if xyz.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err("光谱 XYZ 能量必须有限且非负");
    }
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

#[cfg(test)]
mod tests {
    const SAMPLING_SEED: u64 = 42;
    const MAX_SPECTRAL_BLOCKS: usize = 16;

    // 这些能量/落点参考值使用垂直光源，独立于展示场景的默认斜射光。
    fn vertical_light_scene() -> Scene {
        let mut scene = Scene::new();
        scene.light.direction = -Vector3::z();
        scene
    }

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
    use crate::scene::refractive_index;
    use crate::{
        geometry::{ground::Ground, prism::Prism},
        render::save_png_with_exposure,
        scene::{PhotonLanding, take_photon_landing},
    };
    use rand::rngs::StdRng;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn render_weighted_landing_points(
        scene: &Scene,
        _ground_id: usize,
        settings: &RenderSetting,
        landing: impl Fn(Ray) -> Option<PhotonLanding>,
    ) -> Result<Framebuffer, Box<dyn Error>> {
        let nx = settings.width * settings.samples_per_pixel_axis;
        let ny = settings.height * settings.samples_per_pixel_axis;
        let weight = scene.light.total_flux / (nx * ny) as f64
            * (settings.width * settings.height) as f64
            / scene.receiver.area();
        let span = scene.light.aperture_max_m - scene.light.aperture_min_m;
        let mut rng = StdRng::seed_from_u64(settings.seed);
        let mut image = Framebuffer::new(settings.width, settings.height);
        for y in 0..ny {
            for x in 0..nx {
                let origin = Vector3::new(
                    scene.light.aperture_min_m.x
                        + (x as f64 + rng.random::<f64>()) / nx as f64 * span.x,
                    scene.light.aperture_min_m.y
                        + (y as f64 + rng.random::<f64>()) / ny as f64 * span.y,
                    scene.light.height_m,
                );
                if let Some(p) = landing(Ray::new(origin, scene.light.direction).unwrap()) {
                    splat_spectral_xyz(
                        &mut image,
                        p.position,
                        Vector3::repeat(weight * p.weight),
                        &scene.receiver,
                    )?;
                }
            }
        }
        Ok(image)
    }

    struct CountedGround {
        ground: Ground,
        calls: Arc<AtomicUsize>,
    }

    impl crate::geometry::object::Hittable for CountedGround {
        fn intersect(&self, ray: &Ray) -> Option<crate::geometry::hit::Hit> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.ground.intersect(ray)
        }
    }

    #[test]
    fn fresnel_reduces_only_refracted_flux_without_renormalizing_survivors() {
        let settings = sample_settings(&settings_for_size(32, 24), 2, IorMode::Constant);
        let mut scene = Scene::new();
        let prism = scene.add(Box::new(Prism::new().unwrap()));
        let ground = scene.add(Box::new(Ground::new(0.0).unwrap()));
        // 独立发射参考将每条透射路径设为权重 1，用于检验 Fresnel 能量损失。
        let unweighted = render_weighted_landing_points(&scene, ground, &settings, |ray| {
            take_photon_landing(&scene, ray, prism, ground, || Some(1.52))
                .map(|p| PhotonLanding { weight: 1.0, ..p })
        })
        .unwrap();
        let direct = render_weighted_landing_points(&scene, ground, &settings, |ray| {
            let hit = scene.intersect(&ray)?;
            (hit.object_id == Some(ground)).then_some(PhotonLanding {
                position: hit.coords,
                weight: 1.0,
            })
        })
        .unwrap();
        let lighting = render_spectral_lighting(&scene, Some(prism), ground, &settings).unwrap();
        let mut before = 0.0;
        let mut after = 0.0;
        for (((raw, d), actual_d), actual_c) in unweighted
            .pixels
            .iter()
            .zip(&direct.pixels)
            .zip(&lighting.direct.pixels)
            .zip(&lighting.caustics.pixels)
        {
            assert!((actual_d.y - d.y).abs() < 1e-12);
            let raw_caustics = (raw.y - d.y).max(0.0);
            assert!(actual_c.y >= 0.0 && actual_c.y <= raw_caustics + 1e-12);
            before += raw_caustics;
            after += actual_c.y;
        }
        assert!(after > 0.0 && after < before * 0.99);
    }

    #[test]
    fn oblique_sun_and_photon_flux_share_the_horizontal_window_scale() {
        let mut scene = Scene::new();
        scene.light.direction = Vector3::new(-3.0, 0.0, -4.0);
        // 整个发射窗口的落点平移 -3*3/4=-2.25 米；接收窗口一起平移。
        scene.receiver.min_m.x -= 2.25;
        scene.receiver.max_m.x -= 2.25;
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let settings = settings_for_size(12, 8);
        let lighting = render_spectral_lighting(&scene, None, ground_id, &settings).unwrap();
        let photon_flux: f64 = lighting.direct.pixels.iter().map(|p| p.y).sum::<f64>()
            * scene.receiver.area()
            / (settings.width * settings.height) as f64;
        let analytic_e = lighting.sun_irradiance_xyz.y * lighting.light_wi.z;
        assert!((photon_flux - scene.light.total_flux).abs() < 1e-12);
        assert!((analytic_e - scene.light.total_flux / scene.receiver.area()).abs() < 1e-12);
        assert!((lighting.sun_irradiance_xyz.y - 0.625).abs() < 1e-12);
    }

    #[test]
    fn split_lighting_rejects_invalid_sun_receiver_and_merge_data() {
        let mut scene = Scene::new();
        let ground = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let settings = settings_for_size(2, 2);
        for direction in [
            Vector3::x(),
            Vector3::z(),
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
        ] {
            scene.light.direction = direction;
            assert!(render_spectral_lighting(&scene, None, ground, &settings).is_err());
        }
        scene.light.direction = -Vector3::z();
        let mut lighting = render_spectral_lighting(&scene, None, ground, &settings).unwrap();
        lighting.caustics.pixels[0].x = f64::NAN;
        assert!(lighting.combined().is_err());
        lighting.caustics.pixels.clear();
        assert!(lighting.combined().is_err());
        scene.receiver.max_m = scene.receiver.min_m;
        assert!(render_spectral_lighting(&scene, None, ground, &settings).is_err());
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
        let caustics_object_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let serial_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            let reference = serial_pool.install(|| {
                render_spectral_caustics(
                    &scene,
                    Some(caustics_object_id),
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
                        Some(caustics_object_id),
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
            let caustics_object_id = if case == 2 {
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
                    Some(caustics_object_id),
                    ground_id,
                    &sample_settings(&settings, samples_per_pixel_axis, mode),
                )
                .unwrap();
                assert_eq!(calls.load(Ordering::Relaxed), spatial_ray_count);
                assert_eq!(image.pixels.iter().any(|xyz| xyz.y > 0.0), case == 0);
            }
        }
    }

    #[test]
    fn constant_spectral_render_traces_one_path_per_spatial_sample() {
        let settings = settings_for_size(24, 12);
        let mut scene = Scene::new();
        let caustics_object_id = scene.add(Box::new(Prism::new().unwrap()));
        let calls = Arc::new(AtomicUsize::new(0));
        let ground_id = scene.add(Box::new(CountedGround {
            ground: Ground::new(0.0).unwrap(),
            calls: calls.clone(),
        }));
        // 独立的单路径参考：每条空间光线只调用一次 take_photon_landing。
        let reference = render_weighted_landing_points(
            &scene,
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
            |ray| take_photon_landing(&scene, ray, caustics_object_id, ground_id, || Some(1.52)),
        )
        .unwrap();
        let single_path_calls = calls.load(Ordering::Relaxed);
        assert!(single_path_calls > settings.width * settings.height * 4);

        calls.store(0, Ordering::Relaxed);
        let image = render_spectral_caustics(
            &scene,
            Some(caustics_object_id),
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
            Some(caustics_object_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
        )
        .unwrap();
        assert_eq!(image.pixels, repeated.pixels);

        calls.store(0, Ordering::Relaxed);
        render_spectral_caustics(
            &scene,
            Some(caustics_object_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Dispersion),
        )
        .unwrap();
        assert!(calls.load(Ordering::Relaxed) > single_path_calls);
    }

    #[test]
    fn spectral_unblocked_white_preserves_xyz_flux_across_spatial_sample_counts() {
        let white_xyz = Vector3::new(0.9501416822353593, 1.0, 1.087976622255359);
        for (width, height) in [(1, 1), (1, 4), (4, 1), (12, 8)] {
            let mut scene = vertical_light_scene();
            let ground = scene.add(Box::new(Ground::new(0.0).unwrap()));
            for samples in [1, 3] {
                for mode in [IorMode::Constant, IorMode::Dispersion] {
                    let settings =
                        sample_settings(&settings_for_size(width, height), samples, mode);
                    let image = render_spectral_caustics(&scene, None, ground, &settings).unwrap();
                    let flux: Vector3<f64> = image.pixels.iter().copied().sum::<Vector3<f64>>()
                        * scene.receiver.area()
                        / (width * height) as f64;
                    assert!((flux - white_xyz * scene.light.total_flux).norm() < 2e-12);
                }
            }
        }
    }

    #[test]
    fn spectral_accumulation_matches_independent_single_wavelength_renders_with_lost_flux() {
        let settings = settings_for_size(32, 16);
        let mut scene = Scene::new();
        let caustics_object_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        // 在光源与三棱镜之间遮住一部分光，确保真的测试了丢失通量。
        scene.add(Box::new(
            crate::geometry::triangle::Triangle::new(
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
                Some(caustics_object_id),
                ground_id,
                &sample_settings(&settings, 2, mode),
            )
            .unwrap();
            let mut expected = vec![Vector3::zeros(); settings.width * settings.height];
            let mut lost_flux = false;
            for &(nm, width_nm) in &samples {
                // 独立逐波长发射参考验证光谱积分及丢失通量；不复制生产路径循环。
                let single = render_weighted_landing_points(
                    &scene,
                    ground_id,
                    &sample_settings(&settings, 2, IorMode::Constant),
                    |ray| {
                        take_photon_landing(&scene, ray, caustics_object_id, ground_id, || {
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
        let caustics_object_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let constant = render_spectral_caustics(
            &scene,
            Some(caustics_object_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Constant),
        )
        .unwrap();
        let dispersion = render_spectral_caustics(
            &scene,
            Some(caustics_object_id),
            ground_id,
            &sample_settings(&settings, 2, IorMode::Dispersion),
        )
        .unwrap();
        let repeated = render_spectral_caustics(
            &scene,
            Some(caustics_object_id),
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
        let caustics_object_id = scene.add(Box::new(Prism::new().unwrap()));
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
                Some(caustics_object_id),
                ground_id,
                &sample_settings(&settings, 2, mode),
            )?;
            let path = directory.join(filename);
            save_png_with_exposure(&image, &path, 1.0)?;
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
        let caustics_object_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            for (caustics_object_id, ground_id, samples) in [
                (usize::MAX, ground_id, 1),
                (ground_id, ground_id, 1),
                (caustics_object_id, usize::MAX, 1),
                (caustics_object_id, ground_id, 0),
                (caustics_object_id, ground_id, usize::MAX),
            ] {
                assert!(
                    render_spectral_caustics(
                        &scene,
                        Some(caustics_object_id),
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
                    Some(caustics_object_id),
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
    fn spectral_splat_preserves_xyz_at_interior_edges_and_single_pixel_receivers() {
        let receiver = Scene::new().receiver;
        let xyz = Vector3::new(0.2, 0.7, 1.3);
        for (width, height) in [(4, 2), (1, 1), (1, 4), (4, 1)] {
            for position in [
                Vector3::zeros(),
                Vector3::new(-2.0, -2.0, 0.0),
                Vector3::new(2.0 - 1e-10, 2.0 - 1e-10, 0.0),
            ] {
                let mut image = Framebuffer::new(width, height);
                splat_spectral_xyz(&mut image, position, xyz, &receiver).unwrap();
                assert!((image.pixels.iter().copied().sum::<Vector3<f64>>() - xyz).norm() < 1e-14);
            }
        }
        let mut image = Framebuffer::new(2, 2);
        splat_spectral_xyz(&mut image, Vector3::zeros(), xyz, &receiver).unwrap();
        for pixel in image.pixels {
            assert!((pixel - xyz * 0.25).norm() < 1e-14);
        }
    }

    #[test]
    fn spectral_splat_drops_outside_receiver_and_rejects_invalid_energy() {
        let receiver = Scene::new().receiver;
        let mut image = Framebuffer::new(2, 2);
        splat_spectral_xyz(
            &mut image,
            Vector3::new(2.0, 0.0, 0.0),
            Vector3::repeat(1.0),
            &receiver,
        )
        .unwrap();
        assert!(image.pixels.iter().all(|p| *p == Vector3::zeros()));
        for xyz in [
            Vector3::repeat(f64::NAN),
            Vector3::repeat(-1.0),
            Vector3::repeat(f64::INFINITY),
        ] {
            assert!(splat_spectral_xyz(&mut image, Vector3::zeros(), xyz, &receiver).is_err());
        }
    }
}

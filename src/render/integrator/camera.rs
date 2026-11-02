//! 相机渲染入口、像素内 SPP 累积与 Rayon 像素并行。

use super::{
    direct::{camera_lighting_setup, direct_lighting},
    environment::environment_lighting,
    ground::{GroundLighting, bind_ground_lighting, bind_spectral_ground_lighting},
    path::trace_camera_path,
    rng::{CAMERA_STREAM, DIFFUSE_STREAM, ENVIRONMENT_SEED_OFFSET, ENVIRONMENT_STREAM, pixel_seed},
    types::{
        CameraLightingImages, CameraPathComponents, CameraPathRngs, CameraPathState,
        GroundLightingComponent,
    },
};
use crate::{
    render::{caustics::SpectralLighting, framebuffer::Framebuffer, utils::linear_srgb_to_xyz},
    scene::Scene,
};
use nalgebra::{Vector2, Vector3};
use rand::{Rng, SeedableRng, rngs::StdRng};
use rayon::prelude::*;
use std::{error::Error, mem::size_of};

/// primary 保存单图入口所选分量，或三图入口的 Combined；仅三图入口分配 split。
struct CameraBuffers {
    primary: Framebuffer,
    split: Option<(Framebuffer, Framebuffer)>,
}

/// 每像素从相机发射一条主光线，计算 L_o = V f_r L_light max(n · wi, 0)。
///
/// wi 指向光源（光源传播方向的反向），wo 指向相机；保留几何法线朝向。
/// 将 total_flux / 水平孔径面积作为白色方向光的相对强度 L_light，
/// 不做有限孔径裁切或方向投影校正；这不是绝对辐射度标定。
/// 对有直接光贡献的命中，沿法线偏移起点后向 wi 发射 shadow ray；
/// 平行光位于无穷远，沿途任意表面（包括玻璃）均令可见性 V = 0。
/// 漫反射命中采样方向相关环境并查询可见性；未命中查询同一环境/备用常量背景。
/// 本调试入口的环境 RNG 固定种子 42；玻璃入口使用调用方 seed 的独立环境流。
/// 未绑定材质的命中和理想玻璃为黑色；不追踪反射/折射或叠加地面焦散。
/// 返回未经曝光的线性 XYZ，交给 Render::save 统一曝光和 sRGB 编码。
pub fn render_camera_lit(
    scene: &Scene,
    width: usize,
    height: usize,
) -> Result<Framebuffer, Box<dyn Error>> {
    let (wi, light) = camera_lighting_setup(scene, width, height)?;
    let mut environment_rng = StdRng::seed_from_u64(42 ^ ENVIRONMENT_SEED_OFFSET);
    let mut framebuffer = Framebuffer::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let ray = scene
                .camera
                .ray_for_pixel(x, y, width, height)
                .ok_or("无法生成摄像机光线")?;
            let linear_rgb = match scene.intersect(&ray) {
                None => scene
                    .background_radiance(ray.direction)
                    .ok_or("背景辐亮度无效")?,
                Some(hit) => {
                    let Some(material) = hit.material_id.and_then(|id| scene.material(id)) else {
                        continue;
                    };
                    let scale = scene
                        .ground_reflectance_scale(&hit)
                        .ok_or("地面棋盘格坐标无效")?;
                    (direct_lighting(scene, &hit, material, -ray.direction, wi, light)?
                        + environment_lighting(
                            scene,
                            &hit,
                            material,
                            -ray.direction,
                            &mut environment_rng,
                        )?)
                        * scale
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
/// seed 为每个行优先像素派生独立 RNG；像素并行，像素内按样本（偏移、路径）顺序采样。
/// 路径最多经过八次玻璃散射，其余光照和截断规则见 trace_camera_path。
/// 返回未经曝光的线性 XYZ，交给 Render::save 统一曝光和 sRGB 编码。
pub fn render_camera_glass(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
) -> Result<Framebuffer, Box<dyn Error>> {
    render_camera_glass_impl(scene, width, height, spp, seed, None)
}

/// 使用光源通道生成的地面 XYZ 辐照度图渲染相机画面，先支持中性 Lambertian 地面。
///
/// irradiance 必须来自同一场景的接收区域且尚未曝光/编码；图像分辨率可与相机不同。
/// Scene.ground_id 标识唯一地面；地面太阳贡献由 E*rho/pi 替换原直接光照，区域外为零。
/// 天空贡献单独加一次，不受地面图接收区域限制。
/// 玻璃仍使用原 Fresnel 路径、八次上限、SPP 与抖动；其他表面继续使用直接光照。
/// 缺少地面/材质、非中性灰材质或非法图像数据返回错误。
pub fn render_camera_glass_with_ground(
    scene: &Scene,
    irradiance: &Framebuffer,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
) -> Result<Framebuffer, Box<dyn Error>> {
    let ground = bind_ground_lighting(scene, irradiance, &scene.receiver)?;
    render_camera_glass_impl(scene, width, height, spp, seed, Some(&ground))
}

/// 阶段 14：无限远解析太阳直射 + 经棱镜追踪的焦散，不叠加 sampled direct 图。
///
/// lighting 必须由同一场景/光源的 render_spectral_lighting（或 Render::render_lighting）生成。
/// 地面直射逐交点发射 shadow ray（玻璃也遮挡）；焦散仅查其生成时的 receiver，
/// 区域外只有焦散为零。保留中性灰、Fresnel 相机路径、八次散射、SPP 与抖动。
/// 天空照明/背景计入 Direct 和 Combined，Caustics 不计天空，保持分量可相加。
pub fn render_camera_glass_with_lighting(
    scene: &Scene,
    lighting: &SpectralLighting,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    component: GroundLightingComponent,
) -> Result<Framebuffer, Box<dyn Error>> {
    let ground = bind_spectral_ground_lighting(scene, lighting, component)?;
    render_camera_glass_impl(scene, width, height, spp, seed, Some(&ground))
}

/// 一次遍历像素和 SPP，同时估计 Direct 和 Caustics，均值相加得到 Combined。
/// Direct 包含环境背景、环境照明及解析太阳；Caustics 只包含焦散贴图的地面反射。
/// 子像素/玻璃 RNG 与独立环境 RNG 的种子和消耗顺序沿用单图入口。
pub fn render_camera_glass_components(
    scene: &Scene,
    lighting: &SpectralLighting,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
) -> Result<CameraLightingImages, Box<dyn Error>> {
    let ground = bind_spectral_ground_lighting(scene, lighting, GroundLightingComponent::Combined)?;
    let buffers = render_camera_buffers(scene, width, height, spp, seed, Some(&ground), true)?;
    let (direct, caustics) = buffers.split.expect("三图入口必须分配分量缓冲");
    Ok(CameraLightingImages {
        direct,
        caustics,
        combined: buffers.primary,
    })
}

fn render_camera_glass_impl(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    ground: Option<&GroundLighting<'_>>,
) -> Result<Framebuffer, Box<dyn Error>> {
    Ok(render_camera_buffers(scene, width, height, spp, seed, ground, false)?.primary)
}

fn render_camera_buffers(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    ground: Option<&GroundLighting<'_>>,
    split: bool,
) -> Result<CameraBuffers, Box<dyn Error>> {
    if spp == 0 {
        return Err("相机 SPP 必须大于零".into());
    }
    let (light_wi, light) = camera_lighting_setup(scene, width, height)?;
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(spp))
        .and_then(|paths| paths.checked_mul(scene.environment_samples))
        .and_then(|samples| {
            samples.checked_mul(if scene.max_diffuse_bounces > 1 {
                scene.max_diffuse_bounces + 1
            } else {
                1
            })
        })
        .ok_or("相机路径样本总数溢出")?;
    if split {
        width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(3 * size_of::<Vector3<f64>>()))
            .filter(|&bytes| bytes <= isize::MAX as usize)
            .ok_or("三张相机图的像素缓冲总大小溢出")?;
    }
    let mut buffers = CameraBuffers {
        primary: Framebuffer::new(width, height),
        split: split.then(|| {
            (
                Framebuffer::new(width, height),
                Framebuffer::new(width, height),
            )
        }),
    };
    let component = ground
        .and_then(|g| g.sun.as_ref())
        .map_or(GroundLightingComponent::Combined, |sun| sun.component);
    let render_pixel = |pixel_index: usize| -> Result<CameraPathComponents, String> {
        let x = pixel_index % width;
        let y = pixel_index / width;
        let mut rng = StdRng::seed_from_u64(pixel_seed(seed, pixel_index, CAMERA_STREAM));
        let mut environment_rng =
            StdRng::seed_from_u64(pixel_seed(seed, pixel_index, ENVIRONMENT_STREAM));
        let mut diffuse_rng = StdRng::seed_from_u64(pixel_seed(seed, pixel_index, DIFFUSE_STREAM));
        let mut mean = CameraPathComponents::default();
        // 像素内顺序累积，不做跨线程浮点归约，也不因输出分量重复追踪路径。
        for _ in 0..spp {
            let offset = Vector2::new(rng.random::<f64>(), rng.random::<f64>());
            let ray = scene
                .camera
                .ray_for_pixel_sample(x, y, width, height, offset)
                .ok_or("无法生成子像素摄像机光线")?;
            // 先缩放每项再相加，避免有限样本之和溢出而平均值仍有限。
            let mut state = CameraPathState::new(ray, scene.max_diffuse_bounces);
            trace_camera_path(
                scene,
                &mut state,
                &mut CameraPathRngs {
                    camera: &mut rng,
                    environment: &mut environment_rng,
                    diffuse: &mut diffuse_rng,
                },
                light_wi,
                light,
                ground,
            )
            .map_err(|error| error.to_string())?;
            mean.direct += state.radiance.direct / spp as f64;
            mean.caustics += state.radiance.caustics / spp as f64;
        }
        Ok(mean)
    };
    // 每个任务独占对应像素的可变引用；单图入口不分配 split 缓冲。
    let result: Result<(), String> = match &mut buffers.split {
        Some((direct, caustics)) => buffers
            .primary
            .pixels
            .par_iter_mut()
            .zip(direct.pixels.par_iter_mut())
            .zip(caustics.pixels.par_iter_mut())
            .enumerate()
            .try_for_each(|(index, ((primary, direct), caustics))| {
                let mean = render_pixel(index)?;
                set_camera_pixel(primary, mean.select(component))?;
                set_camera_pixel(direct, mean.direct)?;
                set_camera_pixel(caustics, mean.caustics)
            }),
        None => buffers
            .primary
            .pixels
            .par_iter_mut()
            .enumerate()
            .try_for_each(|(index, primary)| {
                set_camera_pixel(primary, render_pixel(index)?.select(component))
            }),
    };
    // Rayon 只传递 Send 的 String，离开并行迭代后恢复原公共错误接口。
    result.map_err(Box::<dyn Error>::from)?;
    Ok(buffers)
}

fn set_camera_pixel(pixel: &mut Vector3<f64>, rgb: Vector3<f64>) -> Result<(), String> {
    let xyz = linear_srgb_to_xyz(rgb).ok_or("相机样本平均颜色非法或计算溢出")?;
    *pixel = xyz;
    Ok(())
}

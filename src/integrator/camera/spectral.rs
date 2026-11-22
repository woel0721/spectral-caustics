use crate::integrator::camera::{
    RenderedPixel, UnusedRng,
    path::trace_camera_path,
    render_camera_pixels,
    transport::Caustics,
    types::{CameraPathRngs, CameraPathState},
};
// 单波长/四波长 Packet 相机：共享主射线和路径随机序列，独立标量路径与 XYZ 积分。

use super::lighting::camera_lighting_setup;
use crate::caustics::SpectralCausticsMap;
use crate::config::RenderJob;
use crate::config::render::RenderSetting;
use crate::config::render::spectral_sample_counts;
use crate::geometry::ray::Ray;
use crate::sampling::rng::CAMERA_STREAM;
use crate::sampling::rng::DIFFUSE_STREAM;
use crate::sampling::rng::ENVIRONMENT_STREAM;
use crate::sampling::rng::MEDIUM_STREAM;
use crate::sampling::rng::pixel_seed;
use crate::spectrum::cie_xyz;
use crate::spectrum::emission::WAVELENGTH_MAX_NM;
use crate::spectrum::emission::WAVELENGTH_MIN_NM;
use crate::spectrum::emission::WAVELENGTH_PDF;
use nalgebra::Vector2;
use nalgebra::Vector3;
use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::error::Error;

const WAVELENGTH_STREAM: u64 = 0xe703_7ed1_a0b4_28db;
pub(crate) const PREFIX_STREAM: u64 = 0x8ebc_6af0_9c88_c6e3;
const PACKET_SAMPLE_STREAM: u64 = 0x5899_65cc_7537_4cc3;
const PACKET_LANE_STREAM: u64 = 0x1d8e_4e27_c47d_124f;

pub(crate) struct ValidatedSpectralRender {
    pub(crate) spatial_spp: usize,
    pub(crate) total_paths: usize,
    pub(crate) sun_wi: Vector3<f64>,
    pub(crate) sun_strength: f64,
}

pub(crate) fn validate_spectral_render(
    scene: &RenderJob,
    settings: &RenderSetting,
) -> Result<ValidatedSpectralRender, Box<dyn Error + Send + Sync>> {
    #[cfg(feature = "diagnostics")]
    if scene
        .diagnostic_target
        .is_some_and(|id| id >= scene.scene.objects.len())
    {
        return Err("invalid diagnostic target".into());
    }
    let (spatial_spp, total_paths) =
        spectral_sample_counts(settings.camera_spp, settings.spectral_packet_size)?;
    let (sun_wi, sun_strength) =
        spectral_camera_setup(scene, settings.width, settings.height, total_paths)?;
    Ok(ValidatedSpectralRender {
        spatial_spp,
        total_paths,
        sun_wi,
        sun_strength,
    })
}

/// Integrate Hero/Packet radiance into three linear XYZ buffers.
pub fn integrate_spectral(
    scene: &RenderJob,
    settings: &RenderSetting,
    caustics: Option<&SpectralCausticsMap>,
) -> Result<super::CameraLightingImages, Box<dyn Error + Send + Sync>> {
    if let Some(map) = caustics {
        if Some(map.ground_id()) != scene.photon.ground_id
            || map.receiver() != &scene.photon.receiver
            || map.mode() != settings.mode
        {
            return Err("caustics map receiver or IOR mode mismatch".into());
        }
        let ground_material = scene
            .scene
            .objects
            .get(map.ground_id())
            .map(|o| scene.scene.material(o.material_id()))
            .ok_or("caustics map receiver ground invalid")?;
        if ground_material.irradiance_reflectance().is_none() {
            return Err("caustics receiver ground must be Lambertian".into());
        }
    }
    let (width, height, spp, packet_size, seed, ior_mode) = (
        settings.width,
        settings.height,
        settings.camera_spp,
        settings.spectral_packet_size,
        settings.seed,
        settings.mode,
    );
    let ValidatedSpectralRender {
        spatial_spp,
        total_paths,
        sun_wi,
        sun_strength,
    } = validate_spectral_render(scene, settings)?;
    #[cfg(feature = "diagnostics")]
    let debug = settings.diagnostics.path;
    let has_environment = scene.scene.environment.is_some();
    let has_medium = scene.scene.medium.is_some();
    let weight_denominator = WAVELENGTH_PDF * total_paths as f64;
    let caustics_map = caustics.map_or(Caustics::None, Caustics::Spectral);
    render_camera_pixels(
        width,
        height,
        #[cfg(feature = "diagnostics")]
        debug.then_some(scene.diagnostic_target),
        |index| -> Result<RenderedPixel, String> {
            #[cfg(feature = "diagnostics")]
            let mut statistics = debug.then(crate::diagnostics::PathDebugAccumulator::default);
            let mut mean = Vector3::zeros();
            let mut caustics_mean = Vector3::zeros();
            let mut unused_environment = UnusedRng;
            let mut unused_medium = UnusedRng;
            let trace_lane = |ray,
                              nm,
                              rngs: &mut CameraPathRngs<'_>,
                              mean: &mut Vector3<f64>,
                              caustics_mean: &mut Vector3<f64>,
                              #[cfg(feature = "diagnostics")] statistics: &mut Option<
                crate::diagnostics::PathDebugAccumulator,
            >|
             -> Result<(), String> {
                let mut state =
                    CameraPathState::spectral(ray, nm, scene.camera_options.max_diffuse_bounces)?;
                #[cfg(feature = "diagnostics")]
                {
                    state.debug_disable_ground_sun = settings.diagnostics.disable_ground_sun;
                }
                #[cfg(feature = "diagnostics")]
                let mut observation = crate::diagnostics::PathDebugSample::default();
                trace_camera_path(
                    scene,
                    &mut state,
                    rngs,
                    sun_wi,
                    sun_strength,
                    caustics_map,
                    ior_mode,
                    #[cfg(feature = "diagnostics")]
                    statistics.as_ref().map(|_| &mut observation),
                )
                .map_err(|e| e.to_string())?;
                #[cfg(feature = "diagnostics")]
                if let Some(stats) = statistics {
                    stats.record(&observation, scene.diagnostic_target)?;
                }
                // 每个细分层恰取一个样本，权重为 400/总路径数。
                // 不额外乘 Δλ，也不再除以 Packet 大小。
                let has_direct = state.radiance.direct != 0.0;
                let has_caustics = caustics.is_some() && state.radiance.caustics != 0.0;
                if has_direct || has_caustics {
                    let cmf = cie_xyz(nm).ok_or("CIE 查询失败")?;
                    if has_direct {
                        *mean += cmf * (state.radiance.direct / weight_denominator);
                    }
                    if has_caustics {
                        *caustics_mean += cmf * (state.radiance.caustics / weight_denominator);
                    }
                }
                Ok(())
            };
            if packet_size == 1 {
                let mut jitter = StdRng::seed_from_u64(pixel_seed(seed, index, CAMERA_STREAM));
                let mut medium = has_medium
                    .then(|| StdRng::seed_from_u64(pixel_seed(seed, index, MEDIUM_STREAM)));
                let mut scattering = StdRng::seed_from_u64(pixel_seed(seed, index, DIFFUSE_STREAM));
                let mut prefix = StdRng::seed_from_u64(pixel_seed(seed, index, PREFIX_STREAM));
                let mut environment = has_environment
                    .then(|| StdRng::seed_from_u64(pixel_seed(seed, index, ENVIRONMENT_STREAM)));
                let mut wavelength =
                    StdRng::seed_from_u64(pixel_seed(seed, index, WAVELENGTH_STREAM));
                for i in 0..spatial_spp {
                    let offset = Vector2::new(jitter.random::<f64>(), jitter.random::<f64>());
                    let ray = scene
                        .camera
                        .ray_for_pixel_sample(index % width, index / width, width, height, offset)
                        .ok_or("光谱相机子像素光线无效")?;
                    let u = wavelength.random::<f64>();
                    // 单波长模式把 380–780 nm 分成 spp 层，在第 i 层均匀取一个样本。
                    let nm = WAVELENGTH_MIN_NM
                        + (WAVELENGTH_MAX_NM - WAVELENGTH_MIN_NM) * ((i as f64 + u) / spp as f64);
                    trace_lane(
                        ray,
                        nm,
                        &mut CameraPathRngs {
                            camera: &mut prefix,
                            environment: match &mut environment {
                                Some(rng) => rng,
                                None => &mut unused_environment,
                            },
                            diffuse: &mut scattering,
                            medium: match &mut medium {
                                Some(rng) => rng,
                                None => &mut unused_medium,
                            },
                        },
                        &mut mean,
                        &mut caustics_mean,
                        #[cfg(feature = "diagnostics")]
                        &mut statistics,
                    )?;
                }
            } else {
                for i in 0..spatial_spp {
                    let (ray, common) = packet_primary_sample(
                        scene,
                        width,
                        height,
                        seed,
                        index,
                        i,
                        has_environment,
                        has_medium,
                    )?;
                    let mut common = Some(common);
                    for lane in 0..packet_size {
                        let mut wavelength = packet_rng(seed, index, i, lane, WAVELENGTH_STREAM);
                        let u = wavelength.random::<f64>();
                        let nm = packet_wavelength_nm(i, spatial_spp, lane, u);
                        // 波长层内抖动独立；路径流共享随机值，不共享可变游标。
                        let mut lane_rngs = if lane + 1 == packet_size {
                            common.take().expect("last lane consumes common RNGs")
                        } else {
                            common.as_ref().expect("common RNGs").clone()
                        };
                        trace_lane(
                            ray,
                            nm,
                            &mut lane_rngs
                                .as_camera_rngs(&mut unused_environment, &mut unused_medium),
                            &mut mean,
                            &mut caustics_mean,
                            #[cfg(feature = "diagnostics")]
                            &mut statistics,
                        )?;
                    }
                }
            }
            if mean
                .iter()
                .chain(caustics_mean.iter())
                .any(|v| !v.is_finite() || *v < 0.0)
                || (mean + caustics_mean).iter().any(|v| !v.is_finite())
            {
                return Err("光谱 XYZ 积分溢出".into());
            }
            Ok(RenderedPixel {
                direct: mean,
                caustics: caustics_mean,
                #[cfg(feature = "diagnostics")]
                statistics: statistics.map(crate::diagnostics::PathDebugAccumulator::finish),
            })
        },
    )
}

#[derive(Clone)]
pub(crate) struct PacketSampleRngs {
    prefix: StdRng,
    environment: Option<StdRng>,
    scattering: StdRng,
    medium: Option<StdRng>,
}

impl PacketSampleRngs {
    pub(crate) fn as_camera_rngs<'a>(
        &'a mut self,
        unused_environment: &'a mut UnusedRng,
        unused_medium: &'a mut UnusedRng,
    ) -> CameraPathRngs<'a> {
        CameraPathRngs {
            camera: &mut self.prefix,
            environment: match &mut self.environment {
                Some(rng) => rng,
                None => unused_environment,
            },
            diffuse: &mut self.scattering,
            medium: match &mut self.medium {
                Some(rng) => rng,
                None => unused_medium,
            },
        }
    }
}

/// 生成一个 Packet/固定波长空间样本的主射线和共享初始路径随机流（CRN）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn packet_primary_sample(
    scene: &RenderJob,
    width: usize,
    height: usize,
    seed: u64,
    pixel_index: usize,
    sample: usize,
    has_environment: bool,
    has_medium: bool,
) -> Result<(Ray, PacketSampleRngs), &'static str> {
    let mut jitter = packet_rng(seed, pixel_index, sample, 0, CAMERA_STREAM);
    let offset = Vector2::new(jitter.random::<f64>(), jitter.random::<f64>());
    let ray = scene
        .camera
        .ray_for_pixel_sample(
            pixel_index % width,
            pixel_index / width,
            width,
            height,
            offset,
        )
        .ok_or("光谱相机子像素光线无效")?;
    let rngs = PacketSampleRngs {
        prefix: packet_rng(seed, pixel_index, sample, 0, PREFIX_STREAM),
        environment: has_environment
            .then(|| packet_rng(seed, pixel_index, sample, 0, ENVIRONMENT_STREAM)),
        scattering: packet_rng(seed, pixel_index, sample, 0, DIFFUSE_STREAM),
        medium: has_medium.then(|| packet_rng(seed, pixel_index, sample, 0, MEDIUM_STREAM)),
    };
    Ok((ray, rngs))
}

/// 在光谱渲染前检查照明、工作量与材质的光谱能力，返回解析太阳的方向和法向辐照度。
///
/// 圆盘光源沿用照明初始化给出的零强度解析太阳项。
pub(crate) fn spectral_camera_setup(
    scene: &RenderJob,
    width: usize,
    height: usize,
    total_paths: usize,
) -> Result<(Vector3<f64>, f64), Box<dyn Error + Send + Sync>> {
    let sun = camera_lighting_setup(scene, width, height, total_paths)?;
    for object in &scene.scene.objects {
        let material = scene.scene.material(object.material_id());
        if !material.supports_spectral() {
            return Err("光谱相机暂不支持 GGX Metal 或其他无光谱采样接口的材质".into());
        }
    }
    Ok(sun)
}

/// 为某个 Packet 样本/波长路径建立可复现随机流，供像素并行与公共随机数采样。
///
/// 输入：seed、像素编号、空间样本编号、lane 和流标识；输出：派生的 StdRng。
/// 各路径类用独立 stream；需要公共随机数时传相同 lane 并复制 RNG 状态，不共享可变游标。
pub(crate) fn packet_rng(
    seed: u64,
    pixel: usize,
    sample: usize,
    lane: usize,
    stream: u64,
) -> StdRng {
    let pixel = pixel_seed(seed, pixel, stream);
    let sample = pixel_seed(pixel, sample, PACKET_SAMPLE_STREAM);
    StdRng::seed_from_u64(pixel_seed(sample, lane, PACKET_LANE_STREAM))
}

/// 在四波长 Packet 的一个细分层内生成波长，保证所有细分层各取一个样本。
///
/// 输入：空间样本编号 i、每像素空间样本数 S、lane∈0..4 和层内 u∈[0,1)，由调用方保证有效。
///
/// 输出：λ（nm）；公式 λ=380+100·[lane+(i+u)/S]。
/// 右端点用 next_down 夹住，避免浮点舍入把样本放到相邻层。
fn packet_wavelength_nm(sample: usize, spatial_spp: usize, lane: usize, u: f64) -> f64 {
    let band_width = (WAVELENGTH_MAX_NM - WAVELENGTH_MIN_NM) / 4.0;
    let band_min = WAVELENGTH_MIN_NM + band_width * lane as f64;
    let lower = band_min + band_width * (sample as f64 / spatial_spp as f64);
    let upper = band_min + band_width * ((sample + 1) as f64 / spatial_spp as f64);
    let nm = WAVELENGTH_MIN_NM
        + (WAVELENGTH_MAX_NM - WAVELENGTH_MIN_NM)
            * (lane as f64 + (sample as f64 + u) / spatial_spp as f64)
            / 4.0;
    // u<1 仍可能经舍入落在右端点；保持各细分层的左闭右开边界。
    nm.clamp(lower, upper.next_down().max(lower))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;
    use std::collections::HashSet;

    #[test]
    fn packet_wavelengths_follow_uniform_stratum_cdf_and_keep_open_right_edges() {
        for total in [4, 16, 512] {
            let spatial = total / 4;
            for u in [0.0, 0.125, 0.37, 0.5, 0.875, 1.0_f64.next_down()] {
                let mut visits = vec![0; total];
                for i in 0..spatial {
                    for lane in 0..4 {
                        let nm = packet_wavelength_nm(i, spatial, lane, u);
                        let stratum = lane * spatial + i;
                        let lower = 380.0 + 400.0 * stratum as f64 / total as f64;
                        let upper = 380.0 + 400.0 * (stratum + 1) as f64 / total as f64;
                        assert!(nm >= lower && nm < upper, "{total}, {i}, {lane}, {u}: {nm}");
                        // 用目标均匀分布的 CDF 反算分位点；只查区间检测不到 u² 等层内偏差。
                        let cdf = (nm - lower) / (upper - lower);
                        // 按层宽缩放波长舍入误差，也容纳右端点向下夹紧的误差。
                        let tolerance = 8.0 * f64::EPSILON * upper / (upper - lower);
                        assert!(
                            (cdf - u).abs() <= tolerance,
                            "uniform CDF: total={total}, sample={i}, lane={lane}, u={u}, cdf={cdf}"
                        );
                        // 用层边界归类，避免逆变换的舍入把 next_down(upper) 推回整数。
                        let found = (0..total)
                            .find(|&s| {
                                let a = 380.0 + 400.0 * s as f64 / total as f64;
                                let b = 380.0 + 400.0 * (s + 1) as f64 / total as f64;
                                nm >= a && nm < b
                            })
                            .unwrap();
                        visits[found] += 1;
                    }
                }
                assert!(visits.iter().all(|&n| n == 1));
            }
        }
    }

    #[test]
    fn packet_rngs_separate_all_coordinates_and_streams_without_consumption_coupling() {
        let mut values = HashSet::new();
        for seed in [42, 43] {
            for pixel in [0, 1] {
                for sample in [0, 1, 127] {
                    for lane in 0..4 {
                        for stream in [
                            WAVELENGTH_STREAM,
                            CAMERA_STREAM,
                            PREFIX_STREAM,
                            ENVIRONMENT_STREAM,
                            DIFFUSE_STREAM,
                            MEDIUM_STREAM,
                        ] {
                            let mut a = packet_rng(seed, pixel, sample, lane, stream);
                            let mut b = packet_rng(seed, pixel, sample, lane, stream);
                            let first = a.next_u64();
                            assert_eq!(first, b.next_u64());
                            assert!(values.insert(first));
                            // 一个 Lane 的深路径不能消费另一个 Lane 的状态。
                            let mut other = packet_rng(seed, pixel, sample, (lane + 1) % 4, stream);
                            for _ in 0..97 {
                                other.next_u64();
                            }
                            assert_eq!(a.next_u64(), b.next_u64());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn packet_medium_crn_matches_independent_scalar_lanes_after_multiple_scatters() {
        use crate::{
            camera::Camera,
            config::camera::CameraConfig,
            medium::{
                GridMedium, GridMediumConfig, HgAnisotropy, HomogeneousMedium,
                HomogeneousMediumConfig, Medium, MediumCoefficient,
            },
        };
        let mut job = RenderJob::new();
        job.scene.environment = None;
        job.scene.background_color = Vector3::zeros();
        job.camera = Camera::from_config(&CameraConfig {
            position: [0.0; 3],
            target: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 30.0,
        })
        .unwrap();
        for medium in [
            Box::new(
                HomogeneousMedium::try_from(&HomogeneousMediumConfig {
                    min_m: [-2.0; 3],
                    max_m: [2.0; 3],
                    sigma_a: 0.1.into(),
                    sigma_s: 0.9.into(),
                    g: 0.3.into(),
                })
                .unwrap(),
            ) as Box<dyn Medium>,
            Box::new(
                HomogeneousMedium::try_from(&HomogeneousMediumConfig {
                    min_m: [-2.0; 3],
                    max_m: [2.0; 3],
                    sigma_a: 0.1.into(),
                    sigma_s: MediumCoefficient::Curve {
                        samples: vec![[380.0, 4.0], [600.0, 0.0], [780.0, 0.0]],
                    },
                    g: 0.3.into(),
                })
                .unwrap(),
            ),
            Box::new(
                GridMedium::try_from(&GridMediumConfig {
                    min_m: [-2.0; 3],
                    max_m: [2.0; 3],
                    resolution: [16; 3],
                    densities: (0..4096)
                        .map(|i| {
                            [0.0, 0.2, 1.3, 2.0, 0.7, 1.8, 0.3, 1.1]
                                [(i % 16) / 8 + 2 * ((i / 16) % 16 / 8) + 4 * (i / 256 / 8)]
                        })
                        .collect(),
                    sigma_a_base: 0.1.into(),
                    sigma_s_base: 0.9.into(),
                    g: HgAnisotropy::Curve {
                        samples: vec![[380.0, -0.6], [780.0, 0.7]],
                    },
                })
                .unwrap(),
            ),
        ] {
            let chromatic = medium.is_wavelength_dependent();
            job.scene.medium = Some(medium);
            job.camera_options.max_volume_scatters = 4;
            let settings = RenderSetting {
                width: 1,
                height: 1,
                camera_spp: 16,
                spectral_packet_size: 4,
                seed: 42,
                ..Default::default()
            };
            let (sun_wi, sun) = job.scene.light.analytic_sun().unwrap().unwrap();
            let mut expected = Vector3::zeros();
            let mut seen_multiple = false;
            let mut seen_divergent_lanes = false;
            for i in 0..4 {
                let mut jitter = packet_rng(settings.seed, 0, i, 0, CAMERA_STREAM);
                let ray = job
                    .camera
                    .ray_for_pixel_sample(
                        0,
                        0,
                        1,
                        1,
                        Vector2::new(jitter.random(), jitter.random()),
                    )
                    .unwrap();
                let mut reference = None::<(usize, _, _, _, _)>;
                for lane in 0..4 {
                    let mut wavelength = packet_rng(settings.seed, 0, i, lane, WAVELENGTH_STREAM);
                    let nm = packet_wavelength_nm(i, 4, lane, wavelength.random());
                    let mut state = CameraPathState::spectral(ray, nm, 0).unwrap();
                    // Each scalar lane owns fresh streams. The real Packet must reset
                    // its medium cursor to the same state even after a long previous lane.
                    let mut medium = packet_rng(settings.seed, 0, i, 0, MEDIUM_STREAM);
                    trace_camera_path(
                        &job,
                        &mut state,
                        &mut CameraPathRngs {
                            camera: &mut packet_rng(settings.seed, 0, i, 0, PREFIX_STREAM),
                            environment: &mut packet_rng(
                                settings.seed,
                                0,
                                i,
                                0,
                                ENVIRONMENT_STREAM,
                            ),
                            diffuse: &mut packet_rng(settings.seed, 0, i, 0, DIFFUSE_STREAM),
                            medium: &mut medium,
                        },
                        sun_wi,
                        sun,
                        Caustics::None,
                        settings.mode,
                        #[cfg(feature = "diagnostics")]
                        None,
                    )
                    .unwrap();
                    let geometry = (
                        state.volume_scatters,
                        state.ray.origin,
                        state.ray.direction,
                        state.throughput,
                        medium.next_u64(),
                    );
                    if let Some(r) = reference {
                        if chromatic {
                            seen_divergent_lanes |= geometry.0 != r.0 || geometry.2 != r.2;
                        } else {
                            assert_eq!(geometry, r);
                        }
                    } else {
                        reference = Some(geometry);
                    }
                    seen_multiple |= state.volume_scatters > 1;
                    expected +=
                        cie_xyz(nm).unwrap() * (state.radiance.direct / (WAVELENGTH_PDF * 16.0));
                }
            }
            assert!(seen_multiple);
            assert_eq!(seen_divergent_lanes, chromatic);
            let actual = integrate_spectral(&job, &settings, None).unwrap();
            assert_eq!(actual.direct.pixels(), &[expected]);
            assert_eq!(actual.caustics.pixels(), &[Vector3::zeros()]);
            assert_eq!(actual.combined.pixels(), &[expected]);
        }
    }
}

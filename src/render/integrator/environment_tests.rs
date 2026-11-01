//! D2/D3 的受控 PDF、统计回归及可复现比较；旧估计器仅用于测试。

use super::*;
use super::{
    direct::visible_to_light,
    environment::{ENVIRONMENT_BSDF_FRACTION, environment_lighting, power_heuristic},
    rng::{CAMERA_STREAM, ENVIRONMENT_STREAM, pixel_seed},
};
use crate::{
    config::sceneconfig::EnvironmentMisHeuristic,
    geometry::{
        hit::Hit,
        ray::{INTERSECTION_EPSILON, Ray},
    },
    material::{BsdfValue, EvaluateMode, Material, MaterialSample, unit_vector},
    render::{framebuffer::Framebuffer, utils::linear_srgb_to_xyz},
    scene::Scene,
};
use nalgebra::{Vector2, Vector3};
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::{error::Error, f64::consts::PI};

use crate::{
    camera::Camera,
    config::{cameraconfig::CameraConfig, sceneconfig::EnvironmentConfig},
    environment::Environment,
    geometry::{ground::Ground, object::Hittable},
    material::lambertian::Lambertian,
};
use image::{Rgb, codecs::hdr::HdrEncoder};
use rand::RngCore;
use std::{
    collections::VecDeque,
    fs::{self, File},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

struct TemporaryHdr(PathBuf);

impl TemporaryHdr {
    fn new(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "environment-d2-{}-{stamp}-{}.hdr",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let pixels: Vec<_> = pixels.into_iter().map(Rgb).collect();
        HdrEncoder::new(File::create_new(&path).unwrap())
            .encode(&pixels, width, height)
            .unwrap();
        Self(path)
    }

    fn uniform(rgb: [f32; 3]) -> Self {
        Self::new(4, 2, vec![rgb; 8])
    }

    fn high_contrast() -> Self {
        let mut pixels = vec![[0.125; 3]; 64 * 32];
        // 上半球约 0.22% 球面积的 2x2 高亮块；其余区域保持暗灰。
        for row in 7..9 {
            for col in 31..33 {
                pixels[row * 64 + col] = [1024.0; 3];
            }
        }
        Self::new(64, 32, pixels)
    }

    fn scene(&self, rotation_degrees: f64) -> Scene {
        let mut scene = Scene::new();
        scene.environment = Some(
            Environment::try_from(&EnvironmentConfig {
                map_path: Some(self.0.clone()),
                rotation_degrees,
                ..Default::default()
            })
            .unwrap(),
        );
        scene
    }
}

impl Drop for TemporaryHdr {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[derive(Clone)]
struct FirstWordRng {
    first: Option<u64>,
    tail: StdRng,
}

impl FirstWordRng {
    fn new(first: u64, seed: u64) -> Self {
        Self {
            first: Some(first),
            tail: StdRng::seed_from_u64(seed),
        }
    }
}

impl rand::RngCore for FirstWordRng {
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
    fn next_u64(&mut self) -> u64 {
        self.first.take().unwrap_or_else(|| self.tail.next_u64())
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes()[..chunk.len()]);
        }
    }
}

struct WordRng(VecDeque<u64>);
impl rand::RngCore for WordRng {
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
    fn next_u64(&mut self) -> u64 {
        self.0.pop_front().expect("意外消耗额外 RNG")
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes()[..chunk.len()]);
        }
    }
}

struct FixedBsdf {
    wi: Vector3<f64>,
    pdf: f64,
    value: BsdfValue,
    samples: AtomicUsize,
}

impl FixedBsdf {
    fn new(wi: Vector3<f64>, pdf: f64) -> Self {
        Self {
            wi,
            pdf,
            value: BsdfValue::Rgb(Vector3::new(0.2, 0.4, 0.8)),
            samples: AtomicUsize::new(0),
        }
    }
}

impl Material for FixedBsdf {
    fn sample(
        &self,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: &mut dyn rand::RngCore,
    ) -> Option<MaterialSample> {
        self.samples.fetch_add(1, Ordering::Relaxed);
        Some(MaterialSample::Continuous { wi: self.wi })
    }
    fn pdf(&self, _: Vector3<f64>, _: Vector3<f64>, _: Vector3<f64>) -> f64 {
        self.pdf
    }
    fn evaluate(
        &self,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: EvaluateMode,
    ) -> BsdfValue {
        self.value
    }
}

#[test]
fn both_hdri_strategies_use_the_complete_mixture_pdf_in_world_space() {
    let hdr = TemporaryHdr::new(
        4,
        2,
        vec![
            [0.125; 3], [1.0; 3], [4.0; 3], [16.0; 3], [2.0; 3], [8.0; 3], [32.0; 3], [64.0; 3],
        ],
    );
    for rotation in [0.0, 37.0, 90.0] {
        let scene = hdr.scene(rotation);
        let environment = scene.environment.as_ref().unwrap();
        for first in [0, u64::MAX] {
            let mut rng = FirstWordRng::new(first, 42);
            let mut expected_rng = rng.clone();
            let bsdf_branch = expected_rng.random::<f64>() < 0.5;
            let wi = if bsdf_branch {
                Vector3::new(0.3, 0.4, 0.5).normalize()
            } else {
                environment
                    .sample_direction(&mut expected_rng)
                    .unwrap()
                    .direction
            };
            let material = FixedBsdf::new(wi, 0.37);
            let hit = Hit {
                normal: wi,
                ..Default::default()
            };
            let actual = environment_lighting(&scene, &hit, &material, wi, &mut rng).unwrap();
            let pdf_env = environment.pdf_direction(wi).unwrap();
            let pdf_mix = 0.5 * 0.37 + 0.5 * pdf_env;
            let numerator =
                Vector3::new(0.2, 0.4, 0.8).component_mul(&environment.radiance(wi).unwrap());
            assert!((actual - numerator / pdf_mix).norm() < 1e-11);
            let selected_pdf = if bsdf_branch { 0.37 } else { pdf_env };
            assert!((actual - numerator / selected_pdf).norm() > 1e-3);
            assert_eq!(
                material.samples.load(Ordering::Relaxed),
                usize::from(bsdf_branch)
            );
            assert_eq!(rng.next_u64(), expected_rng.next_u64());
        }
    }
}

#[test]
fn hdri_sampling_accepts_zero_bsdf_pdf_and_rejects_invalid_numeric_results() {
    let hdr = TemporaryHdr::uniform([8.0, 4.0, 2.0]);
    let mut scene = hdr.scene(37.0);
    let mut expected_rng = FirstWordRng::new(u64::MAX, 42);
    let _ = expected_rng.random::<f64>();
    let wi = scene
        .environment
        .as_ref()
        .unwrap()
        .sample_direction(&mut expected_rng)
        .unwrap()
        .direction;
    let hit = Hit {
        normal: wi,
        ..Default::default()
    };
    let mut material = FixedBsdf::new(Vector3::z(), 0.0);
    assert!(
        environment_lighting(
            &scene,
            &hit,
            &material,
            wi,
            &mut FirstWordRng::new(u64::MAX, 42)
        )
        .unwrap()
        .norm()
            > 0.0
    );
    for first in [0, u64::MAX] {
        for pdf in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            material.pdf = pdf;
            assert!(
                environment_lighting(
                    &scene,
                    &hit,
                    &material,
                    wi,
                    &mut FirstWordRng::new(first, 42)
                )
                .is_err()
            );
        }
    }
    material.pdf = 0.25;
    for value in [
        BsdfValue::Spectral(1.0),
        BsdfValue::Rgb(Vector3::repeat(f64::NAN)),
        BsdfValue::Rgb(Vector3::repeat(f64::INFINITY)),
        BsdfValue::Rgb(Vector3::repeat(f64::MAX)),
    ] {
        material.value = value;
        assert!(
            environment_lighting(
                &scene,
                &hit,
                &material,
                wi,
                &mut FirstWordRng::new(u64::MAX, 42)
            )
            .is_err()
        );
    }
    material.value = BsdfValue::Rgb(Vector3::repeat(1.0));
    for direction in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
    ] {
        material.wi = direction;
        assert!(
            environment_lighting(&scene, &hit, &material, wi, &mut FirstWordRng::new(0, 42))
                .is_err()
        );
    }
    scene.environment.as_mut().unwrap().intensity = f64::MAX;
    assert!(
        environment_lighting(
            &scene,
            &hit,
            &material,
            wi,
            &mut FirstWordRng::new(u64::MAX, 42)
        )
        .is_err()
    );
}

#[test]
fn back_hemisphere_and_blocked_hdri_samples_keep_the_total_denominator() {
    let hdr = TemporaryHdr::uniform([1.0; 3]);
    let mut scene = hdr.scene(0.0);
    scene.environment_samples = 2;
    let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
    let hit = Hit {
        normal: Vector3::z(),
        ..Default::default()
    };
    // BSDF 正上方 + HDRI 均匀分支近南极；p_bsdf=0 的背半球方向合法。
    let mut rng = WordRng([0, 0, u64::MAX, 0, u64::MAX].into());
    let actual = environment_lighting(&scene, &hit, &material, Vector3::z(), &mut rng).unwrap();
    assert!((actual - Vector3::repeat(0.64)).norm() < 1e-12);
    assert!(rng.0.is_empty());
    struct HalfPlane(Ground);
    impl Hittable for HalfPlane {
        fn intersect(&self, ray: &Ray) -> Option<Hit> {
            let hit = self.0.intersect(ray)?;
            (hit.coords.y > 0.0).then_some(hit)
        }
    }
    scene.add(Box::new(Ground::new(0.0).unwrap()));
    scene.add(Box::new(HalfPlane(Ground::new(1.0).unwrap())));
    // BSDF 的 -Y 方向可见，HDRI 的 +Y 方向被挡；两方向都在正半球。
    let mut rng = WordRng([1 << 63, 3 << 62, u64::MAX, 3 << 62, 1 << 62].into());
    let actual = environment_lighting(&scene, &hit, &material, Vector3::z(), &mut rng).unwrap();
    let cosine = 0.5_f64.sqrt();
    let expected = (0.8 / PI) * cosine / (cosine / PI + 1.0 / (4.0 * PI));
    assert!((actual - Vector3::repeat(expected)).norm() < 1e-12);
    assert!(rng.0.is_empty());
    scene.add(Box::new(Ground::new(2.0).unwrap()));
    assert_eq!(
        environment_lighting(
            &scene,
            &hit,
            &material,
            Vector3::z(),
            &mut StdRng::seed_from_u64(42)
        )
        .unwrap(),
        Vector3::zeros()
    );
}

// dff837c 的纯 BSDF 估计器；只用于 D2 回归与比较，不增加生产模式开关。
fn cosine_environment_lighting(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    rng: &mut dyn rand::RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let environment = scene.environment.as_ref().unwrap();
    let normal = unit_vector(hit.normal).unwrap();
    let wo = unit_vector(wo).unwrap();
    let mut rgb = Vector3::zeros();
    for _ in 0..scene.environment_samples {
        let Some(sample) = material.sample(wo, normal, rng) else {
            continue;
        };
        let MaterialSample::Continuous { wi } = sample else {
            return Err("需要连续 BSDF".into());
        };
        let wi = unit_vector(wi).ok_or("非法方向")?;
        let cosine = normal.dot(&wi).clamp(0.0, 1.0);
        let pdf = material.pdf(wi, wo, normal);
        if !pdf.is_finite() || pdf <= 0.0 || cosine <= 0.0 {
            return Err("非法 PDF/半球".into());
        }
        let BsdfValue::Rgb(f_r) = material.evaluate(wi, wo, normal, EvaluateMode::Rgb) else {
            return Err("需要 RGB BSDF".into());
        };
        let li = environment.radiance(wi).ok_or("非法辐亮度")?;
        let contribution = (f_r * (cosine / pdf)).component_mul(&li);
        if contribution.iter().any(|v| !v.is_finite()) {
            return Err("贡献溢出".into());
        }
        if visible_to_light(scene, hit.coords, normal, wi)? {
            rgb += contribution / scene.environment_samples as f64;
        }
    }
    if !rgb.iter().all(|v| v.is_finite()) {
        return Err("累积溢出".into());
    }
    Ok(rgb)
}

#[test]
fn gradient_environment_keeps_the_old_estimator_and_rng_state_exactly() {
    let mut scene = Scene::new();
    assert!(!scene.environment.as_ref().unwrap().has_map());
    let material = Lambertian::new(Vector3::new(0.2, 0.5, 0.8)).unwrap();
    for samples in [1, 7, 128] {
        scene.environment_samples = samples;
        for normal in [Vector3::z(), Vector3::x(), -Vector3::z()] {
            let hit = Hit {
                normal,
                ..Default::default()
            };
            for seed in 0..8 {
                let mut rng = StdRng::seed_from_u64(seed);
                let mut previous_rng = rng.clone();
                let previous =
                    cosine_environment_lighting(&scene, &hit, &material, normal, &mut previous_rng)
                        .unwrap();
                let current =
                    environment_lighting(&scene, &hit, &material, normal, &mut rng).unwrap();
                assert_eq!(current, previous);
                assert_eq!(rng.next_u64(), previous_rng.next_u64());
            }
        }
    }
}

#[test]
fn uniform_and_black_hdri_lighting_match_the_analytic_expectation() {
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let material = Lambertian::new(albedo).unwrap();
    for rgb in [[8.0, 4.0, 2.0], [0.0; 3]] {
        let hdr = TemporaryHdr::uniform(rgb);
        for rotation in [0.0, 37.0] {
            let mut scene = hdr.scene(rotation);
            assert!(scene.environment.as_ref().unwrap().has_map());
            scene.environment_samples = 4096;
            scene.environment.as_mut().unwrap().intensity = 0.25;
            let expected = albedo.component_mul(&Vector3::from(rgb).cast::<f64>()) * 0.25;
            for normal in [Vector3::z(), Vector3::x(), -Vector3::z()] {
                let hit = Hit {
                    normal,
                    ..Default::default()
                };
                let mut mean = Vector3::zeros();
                for seed in 0..8 {
                    let result = environment_lighting(
                        &scene,
                        &hit,
                        &material,
                        normal,
                        &mut StdRng::seed_from_u64(seed),
                    )
                    .unwrap();
                    let previous = cosine_environment_lighting(
                        &scene,
                        &hit,
                        &material,
                        normal,
                        &mut StdRng::seed_from_u64(seed),
                    )
                    .unwrap();
                    assert!((previous - expected).norm() < 1e-10);
                    assert!(result.iter().all(|v| v.is_finite()));
                    mean += result / 8.0;
                    if seed == 0 {
                        assert_eq!(
                            result,
                            environment_lighting(
                                &scene,
                                &hit,
                                &material,
                                normal,
                                &mut StdRng::seed_from_u64(seed)
                            )
                            .unwrap()
                        );
                    }
                }
                assert!((mean - expected).norm() < 0.015 * expected.norm() + 1e-12);
            }
        }
    }
}

// 与 PDF 无关的高样本数半球求积：dω=dφ dz，参考值使用原双线性 radiance。
fn reference_radiance(
    scene: &Scene,
    albedo: Vector3<f64>,
    phi_steps: usize,
    z_steps: usize,
) -> Vector3<f64> {
    let environment = scene.environment.as_ref().unwrap();
    let mut integral = Vector3::zeros();
    for row in 0..z_steps {
        let z = (row as f64 + 0.5) / z_steps as f64;
        let radius = (1.0 - z * z).sqrt();
        for col in 0..phi_steps {
            let phi = 2.0 * PI * (col as f64 + 0.5) / phi_steps as f64;
            let wi = Vector3::new(radius * phi.cos(), radius * phi.sin(), z);
            integral += environment.radiance(wi).unwrap() * z;
        }
    }
    integral.component_mul(&albedo) * (2.0 / (phi_steps * z_steps) as f64)
}

#[test]
fn high_contrast_hdri_mixture_reduces_error_over_independent_seeds() {
    let hdr = TemporaryHdr::high_contrast();
    let mut scene = hdr.scene(37.0);
    let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
    let hit = Hit {
        normal: Vector3::z(),
        ..Default::default()
    };
    let reference = reference_radiance(&scene, Vector3::repeat(0.8), 1024, 512).x;
    let coarse = reference_radiance(&scene, Vector3::repeat(0.8), 512, 256).x;
    assert!((reference - coarse).abs() < reference * 0.005);
    scene.environment_samples = 256;
    let mut mse = [0.0; 2];
    let mut mean = [0.0; 2];
    for seed in 0..64 {
        for (index, estimator) in [
            cosine_environment_lighting as Estimator,
            random_mixture_environment_lighting as Estimator,
        ]
        .into_iter()
        .enumerate()
        {
            let estimate = estimator(
                &scene,
                &hit,
                &material,
                Vector3::z(),
                &mut StdRng::seed_from_u64(seed),
            )
            .unwrap()
            .x;
            mse[index] += (estimate - reference).powi(2) / 64.0;
            mean[index] += estimate / 64.0;
        }
    }
    eprintln!(
        "D2 high-contrast: reference={reference:.8}, mean={mean:?}, RMSE={:?}, MSE ratio={:.4}",
        mse.map(f64::sqrt),
        mse[1] / mse[0]
    );
    assert!(mse[1] < mse[0] * 0.75, "MSE={mse:?}");
    for index in 0..2 {
        assert!(
            (mean[index] - reference).abs()
                < 5.0 * (mse[index] / 64.0).sqrt() + (reference - coarse).abs()
        );
    }
}

type Estimator = fn(
    &Scene,
    &Hit,
    &dyn Material,
    Vector3<f64>,
    &mut dyn rand::RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>>;

fn diffuse_scene(hdr: &TemporaryHdr) -> Scene {
    let mut scene = hdr.scene(37.0);
    scene.light.total_flux = 0.0;
    scene.ground_checkerboard = None;
    scene.camera = Camera::from_config(&CameraConfig {
        position: [0.0, 0.0, 2.0],
        target: [0.0; 3],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 35.0,
    })
    .unwrap();
    scene.add_with_material(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::new(0.8, 0.6, 0.4)).unwrap()),
    );
    scene
}

// 共同的纯漫反射相机循环：相同几何/相机抖动/逐像素种子/阴影/XYZ，仅替换估计器。
// 计时不含 HDR 加载、求积、PNG，不涵盖玻璃/太阳/光子；生产积分器没有比较开关。
fn render_diffuse(
    scene: &Scene,
    width: usize,
    height: usize,
    spp: usize,
    seed: u64,
    estimator: Estimator,
) -> Framebuffer {
    let mut image = Framebuffer::new(width, height);
    for (index, pixel) in image.pixels.iter_mut().enumerate() {
        let mut camera_rng = StdRng::seed_from_u64(pixel_seed(seed, index, CAMERA_STREAM));
        let mut environment_rng =
            StdRng::seed_from_u64(pixel_seed(seed, index, ENVIRONMENT_STREAM));
        let mut mean = Vector3::zeros();
        for _ in 0..spp {
            let offset = Vector2::new(camera_rng.random::<f64>(), camera_rng.random::<f64>());
            let ray = scene
                .camera
                .ray_for_pixel_sample(index % width, index / width, width, height, offset)
                .unwrap();
            let hit = scene.intersect(&ray).unwrap();
            let material = scene.material(hit.material_id.unwrap()).unwrap();
            mean += estimator(scene, &hit, material, -ray.direction, &mut environment_rng).unwrap()
                / spp as f64;
        }
        *pixel = linear_srgb_to_xyz(mean).unwrap();
    }
    image
}

#[test]
fn diffuse_comparison_harness_matches_the_production_camera() {
    let hdr = TemporaryHdr::high_contrast();
    let scene = diffuse_scene(&hdr);
    let harness = render_diffuse(&scene, 4, 3, 8, 42, environment_lighting);
    let production = render_camera_glass(&scene, 4, 3, 8, 42).unwrap();
    assert_eq!(harness.pixels, production.pixels);
}

#[test]
#[ignore = "release 比较：cargo test --release --locked compare_hdri_sampling -- --ignored --nocapture"]
fn compare_hdri_sampling() {
    const WIDTH: usize = 64;
    const HEIGHT: usize = 48;
    const SEEDS: usize = 8;
    println!(
        "D2 diffuse camera: {WIDTH}x{HEIGHT}, environment_samples=1, seeds=0..{SEEDS}, serial; metrics=linear XYZ Y"
    );
    for (case, hdr) in [
        ("uniform", TemporaryHdr::uniform([1.0; 3])),
        ("high-contrast", TemporaryHdr::high_contrast()),
    ] {
        let scene = diffuse_scene(&hdr);
        let reference = reference_radiance(&scene, Vector3::new(0.8, 0.6, 0.4), 2048, 1024);
        let coarse = reference_radiance(&scene, Vector3::new(0.8, 0.6, 0.4), 1024, 512);
        let reference_y = linear_srgb_to_xyz(reference).unwrap().y;
        assert!((reference - coarse).norm() < reference.norm() * 0.005);
        println!(
            "case={case}, reference_Y={reference_y:.8}, reference_grid=2048x1024, coarse_relative_delta={:.6}",
            (reference - coarse).norm() / reference.norm()
        );
        for spp in [32, 128] {
            let mut seconds = [0.0; 2];
            let mut squared_error = [0.0; 2];
            let mut means = [0.0; 2];
            let count = (WIDTH * HEIGHT * SEEDS) as f64;
            // 预热两条路径；交替测量顺序减小固定先后次序的影响。
            for estimator in [
                cosine_environment_lighting as Estimator,
                random_mixture_environment_lighting as Estimator,
            ] {
                let _ = render_diffuse(&scene, WIDTH, HEIGHT, 2, 100, estimator);
            }
            for seed in 0..SEEDS {
                for index in if seed % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let estimator = [
                        cosine_environment_lighting as Estimator,
                        random_mixture_environment_lighting as Estimator,
                    ][index];
                    let started = Instant::now();
                    let image = render_diffuse(&scene, WIDTH, HEIGHT, spp, seed as u64, estimator);
                    seconds[index] += started.elapsed().as_secs_f64();
                    for pixel in image.pixels {
                        squared_error[index] += (pixel.y - reference_y).powi(2) / count;
                        means[index] += pixel.y / count;
                    }
                }
            }
            for (index, name) in ["cosine", "mixture"].into_iter().enumerate() {
                println!(
                    "case={case}, spp={spp}, estimator={name}, total_seconds={:.6}, mean_frame_ms={:.6}, ns_per_direction={:.3}, mean_Y={:.8}, relative_RMSE={:.6}, MSE_times_seconds={:.8}",
                    seconds[index],
                    seconds[index] * 1000.0 / SEEDS as f64,
                    seconds[index] * 1e9 / (WIDTH * HEIGHT * SEEDS * spp) as f64,
                    means[index],
                    squared_error[index].sqrt() / reference_y,
                    squared_error[index] * seconds[index] / SEEDS as f64
                );
            }
            if squared_error[0] > reference_y.powi(2) * 1e-24 {
                println!(
                    "case={case}, spp={spp}, per_direction_cost_ratio={:.4}, MSE_ratio={:.4}, equal_error_time_ratio_estimate={:.4}",
                    seconds[1] / seconds[0],
                    squared_error[1] / squared_error[0],
                    seconds[1] * squared_error[1] / (seconds[0] * squared_error[0])
                );
            } else {
                println!(
                    "case={case}, spp={spp}, per_direction_cost_ratio={:.4}, cosine_noise=roundoff_only, equal_error_time_ratio_estimate=N/A",
                    seconds[1] / seconds[0]
                );
            }
        }
    }
}

// f12677f 的 D2 随机 50/50 估计器：独立冻结，仅用于回归和性能比较。
fn random_mixture_environment_lighting(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    rng: &mut dyn rand::RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let Some(environment) = &scene.environment else {
        return Ok(Vector3::zeros());
    };
    if material.is_delta() {
        return Ok(Vector3::zeros());
    }
    if scene.environment_samples == 0 {
        return Err("环境方向样本数必须大于零".into());
    }
    let normal = unit_vector(hit.normal).ok_or("环境采样的命中法线无效")?;
    let wo = unit_vector(wo).ok_or("环境采样的出射方向无效")?;
    let has_map = environment.has_map();
    let mut rgb = Vector3::zeros();
    for _ in 0..scene.environment_samples {
        // 短路判断让无 HDRI 的渐变天空不额外消耗策略选择随机数。
        let wi = if !has_map || rng.random::<f64>() < ENVIRONMENT_BSDF_FRACTION {
            let Some(sample) = material.sample(wo, normal, rng) else {
                continue;
            };
            let MaterialSample::Continuous { wi } = sample else {
                return Err("非 delta 材质的环境采样必须返回连续方向".into());
            };
            wi
        } else {
            environment
                .sample_direction(rng)
                .ok_or("HDRI 环境方向采样失败")?
                .direction
        };
        let wi = unit_vector(wi).ok_or("环境采样方向无效")?;
        let cosine = normal.dot(&wi).clamp(0.0, 1.0);
        let pdf_bsdf = material.pdf(wi, wo, normal);
        if !pdf_bsdf.is_finite() || pdf_bsdf < 0.0 {
            return Err("环境采样的 BSDF PDF 无效".into());
        }
        let pdf = if has_map {
            let pdf_env = environment
                .pdf_direction(wi)
                .ok_or("HDRI 环境 PDF 查询失败")?;
            if !pdf_env.is_finite() || pdf_env < 0.0 {
                return Err("HDRI 环境 PDF 无效".into());
            }
            ENVIRONMENT_BSDF_FRACTION * pdf_bsdf + (1.0 - ENVIRONMENT_BSDF_FRACTION) * pdf_env
        } else {
            pdf_bsdf
        };
        if !pdf.is_finite() || pdf <= 0.0 {
            return Err("环境采样的完整 PDF 必须有限且为正".into());
        }
        if cosine <= 0.0 {
            if has_map {
                // 全球 HDRI 样本可位于背半球，贡献为零，仍计入总样本数。
                continue;
            }
            return Err("环境采样的入射半球无效".into());
        }
        let BsdfValue::Rgb(f_r) = material.evaluate(wi, wo, normal, EvaluateMode::Rgb) else {
            return Err("材质 RGB 求值必须返回 RGB BSDF".into());
        };
        if f_r.iter().any(|value| !value.is_finite()) {
            return Err("环境采样的 BSDF 求值无效".into());
        }
        let li = environment.radiance(wi).ok_or("环境辐亮度无效")?;
        if li.iter().any(|value| !value.is_finite()) {
            return Err("环境辐亮度必须有限".into());
        }
        let contribution = (f_r * (cosine / pdf)).component_mul(&li);
        if contribution.iter().any(|value| !value.is_finite()) {
            return Err("环境采样贡献计算溢出".into());
        }
        if visible_to_light(scene, hit.coords, normal, wi)? {
            rgb += contribution / scene.environment_samples as f64;
        }
    }
    if !rgb.iter().all(|value| value.is_finite()) {
        return Err("天空照明颜色非法或计算溢出".into());
    }
    Ok(rgb)
}

mod power;
mod stratified;

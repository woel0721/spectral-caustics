//! Current environment PDF, MIS and independent numerical references.

use super::{
    direct::visible_to_light,
    environment::{environment_lighting, power_heuristic},
};
use crate::{
    config::sceneconfig::EnvironmentMisHeuristic,
    geometry::{
        hit::Hit,
        ray::{INTERSECTION_EPSILON, Ray},
    },
    material::{BsdfValue, EvaluateMode, Material, MaterialSample, unit_vector},
    scene::Scene,
};
use nalgebra::Vector3;
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::f64::consts::PI;

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
    time::{SystemTime, UNIX_EPOCH},
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
            "environment-test-{}-{stamp}-{}.hdr",
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

mod power;
mod stratified;

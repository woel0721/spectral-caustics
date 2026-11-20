use crate::geometry::hit::GeometryHit;
// Current environment PDF, MIS and independent numerical references.

use super::super::lighting::environment::environment_lighting;
use super::super::lighting::environment::power_heuristic;
use super::super::lighting::transmittance_to_light;
use crate::config::RenderJob;
use crate::config::scene::EnvironmentMisHeuristic;
use crate::geometry::hit::Hit;
use crate::geometry::ray::INTERSECTION_EPSILON;
use crate::geometry::ray::Ray;
use crate::material::BsdfSample;
use crate::material::Material;
use crate::material::unit_vector;
use nalgebra::Vector3;
use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::f64::consts::PI;

use crate::camera::Camera;
use crate::config::camera::CameraConfig;
use crate::config::scene::EnvironmentConfig;
use crate::environment::Environment;
use crate::geometry::ground::Ground;
use crate::geometry::object::Hittable;
use crate::material::lambertian::Lambertian;
use image::Rgb;
use image::codecs::hdr::HdrEncoder;
use rand::RngCore;
use std::collections::VecDeque;
use std::fs;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

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

    fn scene(&self, rotation_degrees: f64) -> RenderJob {
        let mut scene = RenderJob::new();
        scene.scene.environment = Some(
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
    value: Vector3<f64>,
    samples: AtomicUsize,
}

impl FixedBsdf {
    fn new(wi: Vector3<f64>, pdf: f64) -> Self {
        Self {
            wi,
            pdf,
            value: (Vector3::new(0.2, 0.4, 0.8)),
            samples: AtomicUsize::new(0),
        }
    }
}

impl Material for FixedBsdf {
    fn sample(
        &self,
        _: Vector3<f64>,
        normal: Vector3<f64>,
        _: &mut dyn rand::RngCore,
    ) -> Option<BsdfSample> {
        self.samples.fetch_add(1, Ordering::Relaxed);
        if self.pdf == 0.0 {
            return None;
        }
        let weight = self.value * (normal.dot(&self.wi).max(0.0) / self.pdf);
        Some(BsdfSample {
            wi: self.wi,
            weight,
            pdf: Some(self.pdf),
            event: crate::material::ScatterType::DiffuseReflection,
        })
    }
    fn pdf(&self, _: Vector3<f64>, _: Vector3<f64>, _: Vector3<f64>) -> f64 {
        self.pdf
    }
    fn evaluate_rgb(&self, _: Vector3<f64>, _: Vector3<f64>, _: Vector3<f64>) -> Vector3<f64> {
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
        let environment = scene.scene.environment.as_ref().unwrap();
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
                coords: nalgebra::Vector3::zeros(),
                distance: 1.0,
                object_id: 0,
                material_id: 0,
            };
            let actual = environment_lighting(
                &scene,
                &hit,
                &material,
                wi,
                &mut rng,
                crate::integrator::camera::transport::Rgb::default(),
                crate::optics::dispersion::IorMode::Constant,
                scene.scene.medium.as_deref(),
            )
            .unwrap();
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
        .scene
        .environment
        .as_ref()
        .unwrap()
        .sample_direction(&mut expected_rng)
        .unwrap()
        .direction;
    let hit = Hit {
        normal: wi,
        coords: nalgebra::Vector3::zeros(),
        distance: 1.0,
        object_id: 0,
        material_id: 0,
    };
    let mut material = FixedBsdf::new(Vector3::z(), 0.0);
    assert!(
        environment_lighting(
            &scene,
            &hit,
            &material,
            wi,
            &mut FirstWordRng::new(u64::MAX, 42),
            crate::integrator::camera::transport::Rgb::default(),
            crate::optics::dispersion::IorMode::Constant,
            scene.scene.medium.as_deref(),
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
                    &mut FirstWordRng::new(first, 42),
                    crate::integrator::camera::transport::Rgb::default(),
                    crate::optics::dispersion::IorMode::Constant,
                    scene.scene.medium.as_deref(),
                )
                .is_err()
            );
        }
    }
    material.pdf = 0.25;
    for value in [
        (Vector3::repeat(-1.0)),
        (Vector3::repeat(f64::NAN)),
        (Vector3::repeat(f64::INFINITY)),
        (Vector3::repeat(f64::MAX)),
    ] {
        material.value = value;
        assert!(
            environment_lighting(
                &scene,
                &hit,
                &material,
                wi,
                &mut FirstWordRng::new(u64::MAX, 42),
                crate::integrator::camera::transport::Rgb::default(),
                crate::optics::dispersion::IorMode::Constant,
                scene.scene.medium.as_deref(),
            )
            .is_err()
        );
    }
    material.value = Vector3::repeat(1.0);
    for direction in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
    ] {
        material.wi = direction;
        assert!(
            environment_lighting(
                &scene,
                &hit,
                &material,
                wi,
                &mut FirstWordRng::new(0, 42),
                crate::integrator::camera::transport::Rgb::default(),
                crate::optics::dispersion::IorMode::Constant,
                scene.scene.medium.as_deref(),
            )
            .is_err()
        );
    }
    scene
        .scene
        .environment
        .as_mut()
        .unwrap()
        .set_intensity(f64::MAX)
        .unwrap();
    assert!(
        environment_lighting(
            &scene,
            &hit,
            &material,
            wi,
            &mut FirstWordRng::new(u64::MAX, 42),
            crate::integrator::camera::transport::Rgb::default(),
            crate::optics::dispersion::IorMode::Constant,
            scene.scene.medium.as_deref(),
        )
        .is_err()
    );
}

#[test]
fn back_hemisphere_and_blocked_hdri_samples_keep_the_total_denominator() {
    let hdr = TemporaryHdr::uniform([1.0; 3]);
    let mut scene = hdr.scene(0.0);
    scene.camera_options.environment_samples = 2;
    let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
    let hit = Hit {
        normal: Vector3::z(),
        coords: nalgebra::Vector3::zeros(),
        distance: 1.0,
        object_id: 0,
        material_id: 0,
    };
    // BSDF 正上方 + HDRI 均匀分支近南极；p_bsdf=0 的背半球方向合法。
    let mut rng = WordRng([0, 0, u64::MAX, 0, u64::MAX].into());
    let actual = environment_lighting(
        &scene,
        &hit,
        &material,
        Vector3::z(),
        &mut rng,
        crate::integrator::camera::transport::Rgb::default(),
        crate::optics::dispersion::IorMode::Constant,
        scene.scene.medium.as_deref(),
    )
    .unwrap();
    assert!((actual - Vector3::repeat(0.64)).norm() < 1e-12);
    assert!(rng.0.is_empty());
    struct HalfPlane(Ground);
    impl Hittable for HalfPlane {
        fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
            let hit = self.0.intersect(ray)?;
            (hit.coords.y > 0.0).then_some(hit)
        }
    }
    scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    scene.scene.add(
        Box::new(HalfPlane(Ground::new(1.0).unwrap())),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    // BSDF 的 -Y 方向可见，HDRI 的 +Y 方向被挡；两方向都在正半球。
    let mut rng = WordRng([1 << 63, 3 << 62, u64::MAX, 3 << 62, 1 << 62].into());
    let actual = environment_lighting(
        &scene,
        &hit,
        &material,
        Vector3::z(),
        &mut rng,
        crate::integrator::camera::transport::Rgb::default(),
        crate::optics::dispersion::IorMode::Constant,
        scene.scene.medium.as_deref(),
    )
    .unwrap();
    let cosine = 0.5_f64.sqrt();
    let expected = (0.8 / PI) * cosine / (cosine / PI + 1.0 / (4.0 * PI));
    assert!((actual - Vector3::repeat(expected)).norm() < 1e-12);
    assert!(rng.0.is_empty());
    scene.scene.add(
        Box::new(Ground::new(2.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    assert_eq!(
        environment_lighting(
            &scene,
            &hit,
            &material,
            Vector3::z(),
            &mut StdRng::seed_from_u64(42),
            crate::integrator::camera::transport::Rgb::default(),
            crate::optics::dispersion::IorMode::Constant,
            scene.scene.medium.as_deref(),
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
            assert!(scene.scene.environment.as_ref().unwrap().has_map());
            scene.camera_options.environment_samples = 4096;
            scene
                .scene
                .environment
                .as_mut()
                .unwrap()
                .set_intensity(0.25)
                .unwrap();
            let expected = albedo.component_mul(&Vector3::from(rgb).cast::<f64>()) * 0.25;
            for normal in [Vector3::z(), Vector3::x(), -Vector3::z()] {
                let hit = Hit {
                    normal,
                    coords: nalgebra::Vector3::zeros(),
                    distance: 1.0,
                    object_id: 0,
                    material_id: 0,
                };
                let mut mean = Vector3::zeros();
                for seed in 0..8 {
                    let result = environment_lighting(
                        &scene,
                        &hit,
                        &material,
                        normal,
                        &mut StdRng::seed_from_u64(seed),
                        crate::integrator::camera::transport::Rgb::default(),
                        crate::optics::dispersion::IorMode::Constant,
                        scene.scene.medium.as_deref(),
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
                                &mut StdRng::seed_from_u64(seed),
                                crate::integrator::camera::transport::Rgb::default(),
                                crate::optics::dispersion::IorMode::Constant,
                                scene.scene.medium.as_deref(),
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
    scene: &RenderJob,
    albedo: Vector3<f64>,
    phi_steps: usize,
    z_steps: usize,
) -> Vector3<f64> {
    let environment = scene.scene.environment.as_ref().unwrap();
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

fn diffuse_scene(hdr: &TemporaryHdr) -> RenderJob {
    let mut scene = hdr.scene(37.0);
    scene.scene.light.as_parallel_mut().unwrap().total_flux = 0.0;
    scene.ground_checkerboard = None;
    scene.camera = Camera::from_config(&CameraConfig {
        position: [0.0, 0.0, 2.0],
        target: [0.0; 3],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 35.0,
    })
    .unwrap();
    scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::new(0.8, 0.6, 0.4)).unwrap()),
    );
    scene
}

#[test]
fn ggx_balance_and_power_match_hemisphere_integral_with_rejected_bsdf_quotas() {
    use crate::material::ggx_metal::GgxMetal;
    let hdr = TemporaryHdr::uniform([1.0, 2.0, 4.0]);
    let mut scene = hdr.scene(37.0);
    scene.camera_options.environment_samples = 3;
    let m = GgxMetal::new(
        Vector3::new(0.2, 0.8, 1.5),
        Vector3::new(3.0, 2.0, 1.0),
        0.65,
    )
    .unwrap();
    let wo = Vector3::new(0.8, 0.0, 0.6);
    let hit = Hit {
        normal: Vector3::z(),
        coords: nalgebra::Vector3::zeros(),
        distance: 1.0,
        object_id: 0,
        material_id: 0,
    };
    let mut reference = Vector3::zeros();
    for j in 0..128 {
        let c = (j as f64 + 0.5) / 128.0;
        for i in 0..256 {
            let angle = 2.0 * PI * (i as f64 + 0.5) / 256.0;
            let r = (1.0 - c * c).sqrt();
            let wi = Vector3::new(r * angle.cos(), r * angle.sin(), c);
            let f = m.evaluate_rgb(wi, wo, Vector3::z());
            reference +=
                f.component_mul(&Vector3::new(1.0, 2.0, 4.0)) * (c * 2.0 * PI / (128.0 * 256.0));
        }
    }
    for heuristic in [
        EnvironmentMisHeuristic::Balance,
        EnvironmentMisHeuristic::Power,
    ] {
        scene.camera_options.environment_mis_heuristic = heuristic;
        let mut sum = Vector3::zeros();
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..16384 {
            sum += environment_lighting(
                &scene,
                &hit,
                &m,
                wo,
                &mut rng,
                crate::integrator::camera::transport::Rgb::default(),
                crate::optics::dispersion::IorMode::Constant,
                scene.scene.medium.as_deref(),
            )
            .unwrap();
        }
        let mean = sum / 16384.0;
        assert!(
            (mean - reference).norm() < 0.04,
            "{heuristic:?}: {mean} vs {reference}"
        );
    }
}

mod power {
    use super::*;

    #[test]
    fn beta_two_weights_sum_to_one_and_stay_finite_at_extreme_scales() {
        for scale in [1e-300, 1.0, 1e300, f64::MAX / 4.0] {
            let (wb, fb) = power_heuristic(2.0 * scale, 3.0 * scale).unwrap();
            let (we, fe) = power_heuristic(3.0 * scale, 2.0 * scale).unwrap();
            assert!((wb - 4.0 / 13.0).abs() < 1e-14);
            assert!((we - 9.0 / 13.0).abs() < 1e-14);
            assert!((wb + we - 1.0).abs() < 1e-14);
            assert!((fb * (2.0 * scale) - wb).abs() < 1e-14);
            assert!((fe * (3.0 * scale) - we).abs() < 1e-14);
        }
        assert_eq!(power_heuristic(0.0, 1.0), Some((0.0, 0.0)));
        assert_eq!(power_heuristic(1.0, 0.0), Some((1.0, 1.0)));
        for (a, b) in [
            (0.0, 0.0),
            (-1.0, 1.0),
            (1.0, -1.0),
            (f64::NAN, 1.0),
            (1.0, f64::INFINITY),
        ] {
            assert!(power_heuristic(a, b).is_none());
        }
    }

    #[test]
    fn power_contributions_use_the_source_pdf_and_actual_odd_counts() {
        let hdr = TemporaryHdr::uniform([1.0; 3]);
        let mut scene = hdr.scene(0.0);
        scene.camera_options.environment_samples = 3;
        scene.camera_options.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
        let hit = Hit {
            normal: Vector3::z(),
            coords: nalgebra::Vector3::zeros(),
            distance: 1.0,
            object_id: 0,
            material_id: 0,
        };
        for pb in [0.0, 0.37] {
            let material = FixedBsdf::new(Vector3::z(), pb);
            let mut rng = WordRng([u64::MAX, 0, 0, u64::MAX, 0, 1 << 62].into());
            let actual = environment_lighting(
                &scene,
                &hit,
                &material,
                Vector3::z(),
                &mut rng,
                crate::integrator::camera::transport::Rgb::default(),
                crate::optics::dispersion::IorMode::Constant,
                scene.scene.medium.as_deref(),
            )
            .unwrap();
            let qb = pb; // n_b=1
            let qe = 2.0 / (4.0 * PI); // n_e=2
            let denominator = qb * qb + qe * qe;
            // BSDF cos=1；两个 HDRI 样本 cos=1、0.5；不额外除以 N。
            let expected = Vector3::new(0.2, 0.4, 0.8) * ((qb + 1.5 * qe) / denominator);
            assert!((actual - expected).norm() < 1e-12);
            assert_eq!(material.samples.load(Ordering::Relaxed), 1);
            assert!(rng.0.is_empty());
            if pb > 0.0 {
                assert!((actual - Vector3::new(0.2, 0.4, 0.8) * (2.5 / (qb + qe))).norm() > 0.1);
            }
        }
    }

    #[test]
    fn single_sample_and_gradient_share_direction_sampling_between_heuristics() {
        let hdr = TemporaryHdr::high_contrast();
        let mut scene = hdr.scene(37.0);
        let material = Lambertian::new(Vector3::new(0.2, 0.5, 0.8)).unwrap();
        let hit = Hit {
            normal: Vector3::z(),
            coords: nalgebra::Vector3::zeros(),
            distance: 1.0,
            object_id: 0,
            material_id: 0,
        };
        for total in [1, 2, 3, 4, 8] {
            scene.camera_options.environment_samples = total;
            for seed in 0..32 {
                let mut a = StdRng::seed_from_u64(seed);
                let mut b = a.clone();
                scene.camera_options.environment_mis_heuristic = EnvironmentMisHeuristic::Balance;
                let balance = environment_lighting(
                    &scene,
                    &hit,
                    &material,
                    Vector3::z(),
                    &mut a,
                    crate::integrator::camera::transport::Rgb::default(),
                    crate::optics::dispersion::IorMode::Constant,
                    scene.scene.medium.as_deref(),
                )
                .unwrap();
                scene.camera_options.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
                let power = environment_lighting(
                    &scene,
                    &hit,
                    &material,
                    Vector3::z(),
                    &mut b,
                    crate::integrator::camera::transport::Rgb::default(),
                    crate::optics::dispersion::IorMode::Constant,
                    scene.scene.medium.as_deref(),
                )
                .unwrap();
                assert_eq!(a.next_u64(), b.next_u64());
                if total == 1 {
                    assert_eq!(balance.map(f64::to_bits), power.map(f64::to_bits));
                }
            }
        }
        scene.scene.environment = RenderJob::new().scene.environment;
        for total in [1, 3, 8] {
            scene.camera_options.environment_samples = total;
            let mut a = StdRng::seed_from_u64(42);
            let mut b = a.clone();
            scene.camera_options.environment_mis_heuristic = EnvironmentMisHeuristic::Balance;
            let balance = environment_lighting(
                &scene,
                &hit,
                &material,
                Vector3::z(),
                &mut a,
                crate::integrator::camera::transport::Rgb::default(),
                crate::optics::dispersion::IorMode::Constant,
                scene.scene.medium.as_deref(),
            )
            .unwrap();
            scene.camera_options.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
            let power = environment_lighting(
                &scene,
                &hit,
                &material,
                Vector3::z(),
                &mut b,
                crate::integrator::camera::transport::Rgb::default(),
                crate::optics::dispersion::IorMode::Constant,
                scene.scene.medium.as_deref(),
            )
            .unwrap();
            assert_eq!(balance.map(f64::to_bits), power.map(f64::to_bits));
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn power_keeps_zero_back_hemisphere_and_occluded_quotas() {
        let hdr = TemporaryHdr::uniform([1.0; 3]);
        let mut scene = hdr.scene(0.0);
        scene.camera_options.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
        scene.camera_options.environment_samples = 2;
        let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
        let hit = Hit {
            normal: Vector3::z(),
            coords: nalgebra::Vector3::zeros(),
            distance: 1.0,
            object_id: 0,
            material_id: 0,
        };
        let mut rng = WordRng([0, 0, u64::MAX, 0, u64::MAX].into());
        let actual = environment_lighting(
            &scene,
            &hit,
            &material,
            Vector3::z(),
            &mut rng,
            crate::integrator::camera::transport::Rgb::default(),
            crate::optics::dispersion::IorMode::Constant,
            scene.scene.medium.as_deref(),
        )
        .unwrap();
        assert!((actual - Vector3::repeat(0.8 * 16.0 / 17.0)).norm() < 1e-12);
        assert!(rng.0.is_empty());
        scene.scene.add(
            Box::new(Ground::new(1.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        assert_eq!(
            environment_lighting(
                &scene,
                &hit,
                &material,
                Vector3::z(),
                &mut StdRng::seed_from_u64(42),
                crate::integrator::camera::transport::Rgb::default(),
                crate::optics::dispersion::IorMode::Constant,
                scene.scene.medium.as_deref(),
            )
            .unwrap(),
            Vector3::zeros()
        );
        let black = TemporaryHdr::uniform([0.0; 3]);
        let mut scene = black.scene(0.0);
        scene.camera_options.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
        for total in [2, 3, 8] {
            scene.camera_options.environment_samples = total;
            assert_eq!(
                environment_lighting(
                    &scene,
                    &hit,
                    &material,
                    Vector3::z(),
                    &mut StdRng::seed_from_u64(42),
                    crate::integrator::camera::transport::Rgb::default(),
                    crate::optics::dispersion::IorMode::Constant,
                    scene.scene.medium.as_deref(),
                )
                .unwrap(),
                Vector3::zeros()
            );
        }
    }

    #[test]
    fn power_estimates_converge_to_uniform_and_high_contrast_references() {
        for (label, hdr) in [
            ("uniform", TemporaryHdr::uniform([1.0; 3])),
            ("high", TemporaryHdr::high_contrast()),
        ] {
            let mut scene = hdr.scene(37.0);
            scene.camera_options.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
            let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
            let hit = Hit {
                normal: Vector3::z(),
                coords: nalgebra::Vector3::zeros(),
                distance: 1.0,
                object_id: 0,
                material_id: 0,
            };
            let reference = reference_radiance(&scene, Vector3::repeat(0.8), 1024, 512).x;
            for total in [2, 3, 8] {
                scene.camera_options.environment_samples = total;
                let mut mean = 0.0;
                let mut mse = 0.0;
                for seed in 0..64 {
                    let mut rng = StdRng::seed_from_u64(seed);
                    let mut value = 0.0;
                    for _ in 0..512 {
                        value += environment_lighting(
                            &scene,
                            &hit,
                            &material,
                            Vector3::z(),
                            &mut rng,
                            crate::integrator::camera::transport::Rgb::default(),
                            crate::optics::dispersion::IorMode::Constant,
                            scene.scene.medium.as_deref(),
                        )
                        .unwrap()
                        .x / 512.0;
                    }
                    mean += value / 64.0;
                    mse += (value - reference).powi(2) / 64.0;
                }
                eprintln!(
                    "Power {label}: N={total}, reference={reference:.8}, mean={mean:.8}, RMSE={:.8}",
                    mse.sqrt()
                );
                assert!((mean - reference).abs() < reference * 0.05);
                assert!((mean - reference).abs() < 5.0 * (mse / 64.0).sqrt() + reference * 1e-4);
            }
        }
    }
}

mod stratified {
    use crate::geometry::hit::GeometryHit;
    // Fixed-quota Balance MIS and independent integration references.

    use super::*;

    struct CountingRng {
        inner: StdRng,
        words: usize,
    }

    impl CountingRng {
        fn new(seed: u64) -> Self {
            Self {
                inner: StdRng::seed_from_u64(seed),
                words: 0,
            }
        }
    }

    impl RngCore for CountingRng {
        fn next_u32(&mut self) -> u32 {
            self.words += 1;
            self.inner.next_u32()
        }
        fn next_u64(&mut self) -> u64 {
            self.words += 2;
            self.inner.next_u64()
        }
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            for chunk in dest.chunks_mut(8) {
                chunk.copy_from_slice(&self.next_u64().to_le_bytes()[..chunk.len()]);
            }
        }
    }

    struct HalfPlane(Ground);

    impl Hittable for HalfPlane {
        fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
            let hit = self.0.intersect(ray)?;
            (hit.coords.y > 0.0).then_some(hit)
        }
    }

    #[test]
    fn fixed_strategy_counts_and_complete_count_weighted_denominator_match() {
        let hdr = TemporaryHdr::high_contrast();
        let mut scene = hdr.scene(37.0);
        let hit = Hit {
            normal: Vector3::z(),
            coords: nalgebra::Vector3::zeros(),
            distance: 1.0,
            object_id: 0,
            material_id: 0,
        };
        let f_r = Vector3::new(0.2, 0.4, 0.8);
        for (total, nb, ne) in [(2, 1, 1), (3, 1, 2), (4, 2, 2), (8, 4, 4)] {
            scene.camera_options.environment_samples = total;
            for seed in 0..16 {
                // FixedBsdf 不消耗 RNG；剩余随机数必须恰好对应 ne 次真实 HDRI 调用。
                let material = FixedBsdf::new(Vector3::z(), 0.37);
                let environment = scene.scene.environment.as_ref().unwrap();
                let mut expected_rng = CountingRng::new(seed);
                let mut expected = Vector3::zeros();
                let denominator =
                    |wi| nb as f64 * 0.37 + ne as f64 * environment.pdf_direction(wi).unwrap();
                for _ in 0..nb {
                    expected += f_r.component_mul(&environment.radiance(Vector3::z()).unwrap())
                        / denominator(Vector3::z());
                }
                for _ in 0..ne {
                    let wi = unit_vector(
                        environment
                            .sample_direction(&mut expected_rng)
                            .unwrap()
                            .direction,
                    )
                    .unwrap();
                    let cosine = wi.z.max(0.0);
                    expected += (f_r * (cosine / denominator(wi)))
                        .component_mul(&environment.radiance(wi).unwrap());
                }
                let mut rng = CountingRng::new(seed);
                let actual = environment_lighting(
                    &scene,
                    &hit,
                    &material,
                    Vector3::z(),
                    &mut rng,
                    crate::integrator::camera::transport::Rgb::default(),
                    crate::optics::dispersion::IorMode::Constant,
                    scene.scene.medium.as_deref(),
                )
                .unwrap();
                assert_eq!(material.samples.load(Ordering::Relaxed), nb);
                assert_eq!(rng.words, expected_rng.words, "N={total}, HDRI quota={ne}");
                assert_eq!(rng.next_u64(), expected_rng.next_u64());
                assert!(
                    (actual - expected).norm() < 1e-10,
                    "N={total}: {actual:?} != {expected:?}"
                );
            }
        }
    }

    #[test]
    fn odd_count_uses_one_bsdf_and_two_environment_pdfs_for_both_sources() {
        let hdr = TemporaryHdr::uniform([1.0; 3]);
        let mut scene = hdr.scene(0.0);
        scene.camera_options.environment_samples = 3;
        let material = FixedBsdf::new(Vector3::z(), 0.37);
        let hit = Hit {
            normal: Vector3::z(),
            coords: nalgebra::Vector3::zeros(),
            distance: 1.0,
            object_id: 0,
            material_id: 0,
        };
        // BSDF +Z；两个 HDRI 均匀分支分别取 z=1 和 z=0.5，没有策略选择随机数。
        let mut rng = WordRng([u64::MAX, 0, 0, u64::MAX, 0, 1 << 62].into());
        let actual = environment_lighting(
            &scene,
            &hit,
            &material,
            Vector3::z(),
            &mut rng,
            crate::integrator::camera::transport::Rgb::default(),
            crate::optics::dispersion::IorMode::Constant,
            scene.scene.medium.as_deref(),
        )
        .unwrap();
        let f_r = Vector3::new(0.2, 0.4, 0.8);
        let denominator = 0.37 + 2.0 / (4.0 * PI);
        let expected = f_r * (2.5 / denominator);
        assert!((actual - expected).norm() < 1e-12);
        assert!((actual - f_r * (2.5 / (1.5 * 0.37 + 1.5 / (4.0 * PI)))).norm() > 0.1);
        assert_eq!(material.samples.load(Ordering::Relaxed), 1);
        assert!(rng.0.is_empty());
    }

    #[test]
    fn zero_pdf_missing_and_zero_contribution_samples_do_not_reallocate_quotas() {
        let hdr = TemporaryHdr::uniform([1.0; 3]);
        let mut scene = hdr.scene(0.0);
        scene.camera_options.environment_samples = 3;
        let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
        let hit = Hit {
            normal: Vector3::z(),
            coords: nalgebra::Vector3::zeros(),
            distance: 1.0,
            object_id: 0,
            material_id: 0,
        };
        // 背面 wo 让材质合法返回 None、PDF=0、BSDF=0；两个 HDRI 配额仍全部执行。
        let mut rng = WordRng([u64::MAX, 0, 0, u64::MAX, 0, u64::MAX].into());
        let actual = environment_lighting(
            &scene,
            &hit,
            &material,
            -Vector3::z(),
            &mut rng,
            crate::integrator::camera::transport::Rgb::default(),
            crate::optics::dispersion::IorMode::Constant,
            scene.scene.medium.as_deref(),
        )
        .unwrap();
        assert_eq!(actual, Vector3::zeros());
        assert!(rng.0.is_empty());

        struct MissingFirst(FixedBsdf);
        impl Material for MissingFirst {
            fn sample(
                &self,
                wo: Vector3<f64>,
                normal: Vector3<f64>,
                rng: &mut dyn RngCore,
            ) -> Option<BsdfSample> {
                let sample = self.0.sample(wo, normal, rng);
                (self.0.samples.load(Ordering::Relaxed) > 1)
                    .then_some(sample)
                    .flatten()
            }
            fn pdf(&self, wi: Vector3<f64>, wo: Vector3<f64>, normal: Vector3<f64>) -> f64 {
                self.0.pdf(wi, wo, normal)
            }
            fn evaluate_rgb(
                &self,
                wi: Vector3<f64>,
                wo: Vector3<f64>,
                normal: Vector3<f64>,
            ) -> Vector3<f64> {
                self.0.evaluate_rgb(wi, wo, normal)
            }
        }
        scene.camera_options.environment_samples = 4;
        let material = MissingFirst(FixedBsdf::new(Vector3::z(), 0.25));
        // 第一个 BSDF None、第二个 BSDF 有效；HDRI 分别为正上方和背半球。
        let mut rng = WordRng([u64::MAX, 0, 0, u64::MAX, 0, u64::MAX].into());
        let actual = environment_lighting(
            &scene,
            &hit,
            &material,
            Vector3::z(),
            &mut rng,
            crate::integrator::camera::transport::Rgb::default(),
            crate::optics::dispersion::IorMode::Constant,
            scene.scene.medium.as_deref(),
        )
        .unwrap();
        let expected = Vector3::new(0.2, 0.4, 0.8) * (2.0 / (2.0 * 0.25 + 2.0 / (4.0 * PI)));
        assert!((actual - expected).norm() < 1e-12);
        assert_eq!(material.0.samples.load(Ordering::Relaxed), 2);
        assert!(rng.0.is_empty());
    }

    #[test]
    fn small_even_and_odd_quotas_preserve_uniform_hdr_brightness() {
        let hdr = TemporaryHdr::uniform([8.0, 4.0, 2.0]);
        let mut scene = hdr.scene(37.0);
        let albedo = Vector3::new(0.2, 0.5, 0.8);
        let material = Lambertian::new(albedo).unwrap();
        let hit = Hit {
            normal: Vector3::z(),
            coords: nalgebra::Vector3::zeros(),
            distance: 1.0,
            object_id: 0,
            material_id: 0,
        };
        let expected = albedo.component_mul(&Vector3::new(8.0, 4.0, 2.0));
        for total in [2, 3, 4, 8] {
            scene.camera_options.environment_samples = total;
            let mut mean = Vector3::zeros();
            for seed in 0..4096 {
                mean += environment_lighting(
                    &scene,
                    &hit,
                    &material,
                    Vector3::z(),
                    &mut StdRng::seed_from_u64(seed),
                    crate::integrator::camera::transport::Rgb::default(),
                    crate::optics::dispersion::IorMode::Constant,
                    scene.scene.medium.as_deref(),
                )
                .unwrap()
                    / 4096.0;
            }
            assert!(
                (mean - expected).norm() < expected.norm() * 0.02,
                "N={total}: {mean:?}"
            );
        }
    }

    #[test]
    fn count_weighted_contributions_reject_invalid_values_and_overflow() {
        let hdr = TemporaryHdr::uniform([1.0; 3]);
        let mut scene = hdr.scene(0.0);
        let hit = Hit {
            normal: Vector3::z(),
            coords: nalgebra::Vector3::zeros(),
            distance: 1.0,
            object_id: 0,
            material_id: 0,
        };
        for total in [2, 3, 8] {
            scene.camera_options.environment_samples = total;
            for pdf in [-1.0, f64::NAN, f64::INFINITY] {
                let material = FixedBsdf::new(Vector3::z(), pdf);
                assert!(
                    environment_lighting(
                        &scene,
                        &hit,
                        &material,
                        Vector3::z(),
                        &mut StdRng::seed_from_u64(42),
                        crate::integrator::camera::transport::Rgb::default(),
                        crate::optics::dispersion::IorMode::Constant,
                        scene.scene.medium.as_deref(),
                    )
                    .is_err()
                );
            }
            let mut material = FixedBsdf::new(Vector3::z(), 0.37);
            material.value = Vector3::repeat(f64::INFINITY);
            assert!(
                environment_lighting(
                    &scene,
                    &hit,
                    &material,
                    Vector3::z(),
                    &mut StdRng::seed_from_u64(42),
                    crate::integrator::camera::transport::Rgb::default(),
                    crate::optics::dispersion::IorMode::Constant,
                    scene.scene.medium.as_deref(),
                )
                .is_err()
            );
        }
        scene.camera_options.environment_samples = 8;
        // p_b 本身有限，但 n_b*p_b 溢出时必须拒绝。
        let material = FixedBsdf::new(Vector3::z(), f64::MAX);
        assert!(
            environment_lighting(
                &scene,
                &hit,
                &material,
                Vector3::z(),
                &mut StdRng::seed_from_u64(42),
                crate::integrator::camera::transport::Rgb::default(),
                crate::optics::dispersion::IorMode::Constant,
                scene.scene.medium.as_deref(),
            )
            .is_err()
        );
    }

    pub(super) fn reference_at(
        scene: &RenderJob,
        position: Vector3<f64>,
        albedo: Vector3<f64>,
        phi_steps: usize,
        z_steps: usize,
    ) -> Vector3<f64> {
        let environment = scene.scene.environment.as_ref().unwrap();
        let mut integral = Vector3::zeros();
        for row in 0..z_steps {
            let z = (row as f64 + 0.5) / z_steps as f64;
            let radius = (1.0 - z * z).sqrt();
            for col in 0..phi_steps {
                let phi = 2.0 * PI * (col as f64 + 0.5) / phi_steps as f64;
                let wi = Vector3::new(radius * phi.cos(), radius * phi.sin(), z);
                if transmittance_to_light(
                    scene,
                    position,
                    Vector3::z(),
                    wi,
                    scene.scene.medium.as_deref(),
                    None,
                )
                .unwrap()
                    > 0.0
                {
                    integral += environment.radiance(wi).unwrap() * z;
                }
            }
        }
        integral.component_mul(&albedo) * (2.0 / (phi_steps * z_steps) as f64)
    }

    #[test]
    fn high_contrast_stratified_estimates_converge_with_and_without_occlusion() {
        let hdr = TemporaryHdr::high_contrast();
        for occluded in [false, true] {
            let mut scene = hdr.scene(0.0);
            if occluded {
                scene.scene.add(
                    Box::new(HalfPlane(Ground::new(1.0).unwrap())),
                    Box::new(
                        crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(
                            0.8,
                        ))
                        .unwrap(),
                    ),
                );
            }
            let reference =
                reference_at(&scene, Vector3::zeros(), Vector3::repeat(0.8), 1024, 512).x;
            let coarse = reference_at(&scene, Vector3::zeros(), Vector3::repeat(0.8), 512, 256).x;
            assert!((reference - coarse).abs() < reference * 0.005);
            let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
            let hit = Hit {
                normal: Vector3::z(),
                coords: nalgebra::Vector3::zeros(),
                distance: 1.0,
                object_id: 0,
                material_id: 0,
            };
            for total in [2, 3, 4, 8] {
                scene.camera_options.environment_samples = total;
                let mut mse = [0.0; 2];
                let mut mean = 0.0;
                for seed in 0..64 {
                    let mut rng = StdRng::seed_from_u64(seed);
                    let mut estimate = 0.0;
                    for path in 0..512 {
                        let value = environment_lighting(
                            &scene,
                            &hit,
                            &material,
                            Vector3::z(),
                            &mut rng,
                            crate::integrator::camera::transport::Rgb::default(),
                            crate::optics::dispersion::IorMode::Constant,
                            scene.scene.medium.as_deref(),
                        )
                        .unwrap()
                        .x;
                        if path == 0 {
                            mse[0] += (value - reference).powi(2) / 64.0;
                        }
                        estimate += value / 512.0;
                    }
                    mse[1] += (estimate - reference).powi(2) / 64.0;
                    mean += estimate / 64.0;
                }
                eprintln!(
                    "D3 convergence: occluded={occluded}, N={total}, reference={reference:.8}, mean={mean:.8}, RMSE={:?}",
                    mse.map(f64::sqrt)
                );
                // 少量单次估计可能漏掉低概率大贡献，不预设其经验 MSE 下降比例。
                assert!(
                    (mean - reference).abs() < reference * 0.05,
                    "N={total}, mean={mean}, reference={reference}"
                );
                assert!(
                    (mean - reference).abs()
                        < 5.0 * (mse[1] / 64.0).sqrt() + (reference - coarse).abs()
                );
            }
        }
    }

    pub(super) fn partial_scene(hdr: &TemporaryHdr) -> RenderJob {
        let mut scene = diffuse_scene(hdr);
        // 相机位于遮挡平面下方，主光线不碰遮挡物；向上环境光只有部分可见。
        scene.camera = Camera::from_config(&CameraConfig {
            position: [0.0, 0.0, 0.5],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 35.0,
        })
        .unwrap();
        scene.scene.add(
            Box::new(HalfPlane(Ground::new(1.0).unwrap())),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        scene
    }

    #[test]
    fn partial_occlusion_reference_matches_independent_numerical_integration() {
        let hdr = TemporaryHdr::uniform([1.0; 3]);
        let scene = partial_scene(&hdr);
        let h = 1.0 - 10.0 * INTERSECTION_EPSILON;
        for y in [-0.15, 0.0, 0.15] {
            let position = Vector3::new(0.0, y, 0.0);
            let numerical = reference_at(&scene, position, Vector3::new(0.8, 0.6, 0.4), 1024, 512);
            let analytic = Vector3::new(0.8, 0.6, 0.4) * (0.5 * (1.0 - y / h.hypot(y)));
            assert!((numerical - analytic).norm() < analytic.norm() * 0.002);
        }
    }
}

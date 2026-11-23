// Current environment PDF, MIS and independent numerical references.

use super::super::lighting::environment::environment_lighting;
use super::super::lighting::environment::power_heuristic;
use super::super::lighting::transmittance_to_light;
use crate::config::RenderJob;
use crate::config::scene::EnvironmentMisHeuristic;
use crate::geometry::hit::Hit;
use crate::material::BsdfSample;
use crate::material::Material;
use crate::material::unit_vector;
use nalgebra::Vector3;
use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::f64::consts::PI;

use crate::geometry::ground::Ground;
use crate::material::lambertian::Lambertian;
use rand::RngCore;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::support::{HalfPlane, TemporaryHdr};

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
fn uniform_hdri_lighting_matches_analytic_expectation_across_quotas_normals_and_rotations() {
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let material = Lambertian::new(albedo).unwrap();
    let rgb = [8.0, 4.0, 2.0];
    let hdr = TemporaryHdr::uniform(rgb);
    for rotation in [0.0, 37.0] {
        let mut scene = hdr.scene(rotation);
        assert!(scene.scene.environment.as_ref().unwrap().has_map());
        scene.camera_options.environment_samples = 1024;
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
            let result = environment_lighting(
                &scene,
                &hit,
                &material,
                normal,
                &mut StdRng::seed_from_u64(42),
                crate::integrator::camera::transport::Rgb::default(),
                crate::optics::dispersion::IorMode::Constant,
                scene.scene.medium.as_deref(),
            )
            .unwrap();
            assert!(result.iter().all(|v| v.is_finite()));
            assert!((result - expected).norm() < 0.03 * expected.norm() + 1e-12);
        }
    }
    let mut scene = hdr.scene(37.0);
    let hit = Hit {
        normal: Vector3::z(),
        coords: nalgebra::Vector3::zeros(),
        distance: 1.0,
        object_id: 0,
        material_id: 0,
    };
    let expected = albedo.component_mul(&Vector3::from(rgb).cast::<f64>());
    for total in [1, 2, 3, 4, 8] {
        scene.camera_options.environment_samples = total;
        let mut rng = StdRng::seed_from_u64(100 + total as u64);
        let mut mean = Vector3::zeros();
        let trials = (2048 / total).max(256);
        for _ in 0..trials {
            mean += environment_lighting(
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
                / trials as f64;
        }
        assert!(
            (mean - expected).norm() < expected.norm() * 0.03,
            "N={total}: {mean:?}"
        );
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
    for j in 0..64 {
        let c = (j as f64 + 0.5) / 64.0;
        for i in 0..128 {
            let angle = 2.0 * PI * (i as f64 + 0.5) / 128.0;
            let r = (1.0 - c * c).sqrt();
            let wi = Vector3::new(r * angle.cos(), r * angle.sin(), c);
            let f = m.evaluate_rgb(wi, wo, Vector3::z());
            reference +=
                f.component_mul(&Vector3::new(1.0, 2.0, 4.0)) * (c * 2.0 * PI / (64.0 * 128.0));
        }
    }
    for heuristic in [
        EnvironmentMisHeuristic::Balance,
        EnvironmentMisHeuristic::Power,
    ] {
        scene.camera_options.environment_mis_heuristic = heuristic;
        let samples = 6144;
        let mut sum = Vector3::zeros();
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..samples {
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
        let mean = sum / samples as f64;
        assert!(
            (mean - reference).norm() < 0.05,
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
        for (uniform, hdr) in [
            (true, TemporaryHdr::uniform([1.0; 3])),
            (false, TemporaryHdr::high_contrast()),
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
            let reference = if uniform {
                0.8
            } else {
                reference_radiance(&scene, Vector3::repeat(0.8), 256, 128).x
            };
            let seeds = 32;
            let paths = 512;
            for total in [2, 3, 8] {
                scene.camera_options.environment_samples = total;
                let mut mean = 0.0;
                let mut mse = 0.0;
                for seed in 0..seeds {
                    let mut rng = StdRng::seed_from_u64(seed as u64);
                    let mut value = 0.0;
                    for _ in 0..paths {
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
                        .x / paths as f64;
                    }
                    mean += value / seeds as f64;
                    mse += (value - reference).powi(2) / seeds as f64;
                }
                assert!((mean - reference).abs() < reference * 0.05);
                assert!(
                    (mean - reference).abs() < 5.0 * (mse / seeds as f64).sqrt() + reference * 5e-3
                );
            }
        }
    }
}

mod stratified {
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
                    scene.scene.has_medium_interface(),
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
                reference_at(&scene, Vector3::zeros(), Vector3::repeat(0.8), 256, 128).x;
            let coarse = reference_at(&scene, Vector3::zeros(), Vector3::repeat(0.8), 128, 64).x;
            assert!((reference - coarse).abs() < reference * 0.01);
            let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
            let hit = Hit {
                normal: Vector3::z(),
                coords: nalgebra::Vector3::zeros(),
                distance: 1.0,
                object_id: 0,
                material_id: 0,
            };
            let seeds = 32;
            let paths = 512;
            for total in [2, 3, 8] {
                scene.camera_options.environment_samples = total;
                let mut mse = 0.0;
                let mut mean = 0.0;
                for seed in 0..seeds {
                    let mut rng = StdRng::seed_from_u64(seed as u64);
                    let mut estimate = 0.0;
                    for _ in 0..paths {
                        estimate += environment_lighting(
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
                        .x / paths as f64;
                    }
                    mse += (estimate - reference).powi(2) / seeds as f64;
                    mean += estimate / seeds as f64;
                }
                // 少量单次估计可能漏掉低概率大贡献，不预设其经验 MSE 下降比例。
                assert!(
                    (mean - reference).abs() < reference * 0.05,
                    "N={total}, mean={mean}, reference={reference}"
                );
                assert!(
                    (mean - reference).abs()
                        < 5.0 * (mse / seeds as f64).sqrt() + (reference - coarse).abs()
                );
            }
        }
    }
}

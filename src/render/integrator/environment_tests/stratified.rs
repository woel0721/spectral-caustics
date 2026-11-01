//! D3 固定配额 Balance MIS：冻结的 D2 估计器只用于比较。

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
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        let hit = self.0.intersect(ray)?;
        (hit.coords.y > 0.0).then_some(hit)
    }
}

#[test]
fn one_sample_matches_d2_bits_and_rng_for_every_environment_kind() {
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let material = Lambertian::new(albedo).unwrap();
    for hdr in [
        TemporaryHdr::uniform([0.0; 3]),
        TemporaryHdr::uniform([8.0, 4.0, 2.0]),
        TemporaryHdr::high_contrast(),
    ] {
        for rotation in [0.0, 37.0, 90.0] {
            let mut scene = hdr.scene(rotation);
            scene.add(Box::new(HalfPlane(Ground::new(1.0).unwrap())));
            for normal in [Vector3::z(), Vector3::x(), -Vector3::z()] {
                let hit = Hit {
                    normal,
                    ..Default::default()
                };
                for seed in 0..128 {
                    let mut rng = StdRng::seed_from_u64(seed);
                    let mut previous_rng = rng.clone();
                    let previous = random_mixture_environment_lighting(
                        &scene,
                        &hit,
                        &material,
                        normal,
                        &mut previous_rng,
                    )
                    .unwrap();
                    let actual =
                        environment_lighting(&scene, &hit, &material, normal, &mut rng).unwrap();
                    assert_eq!(actual.map(f64::to_bits), previous.map(f64::to_bits));
                    assert_eq!(rng.next_u64(), previous_rng.next_u64());
                }
            }
        }
        let scene = diffuse_scene(&hdr);
        let current = render_diffuse(&scene, 4, 3, 8, 42, environment_lighting);
        let old = render_diffuse(&scene, 4, 3, 8, 42, random_mixture_environment_lighting);
        for (a, b) in current.pixels.iter().zip(&old.pixels) {
            assert_eq!(a.map(f64::to_bits), b.map(f64::to_bits));
        }
    }
}

#[test]
fn fixed_strategy_counts_and_complete_count_weighted_denominator_match() {
    let hdr = TemporaryHdr::high_contrast();
    let mut scene = hdr.scene(37.0);
    let hit = Hit {
        normal: Vector3::z(),
        ..Default::default()
    };
    let f_r = Vector3::new(0.2, 0.4, 0.8);
    for (total, nb, ne) in [(2, 1, 1), (3, 1, 2), (4, 2, 2), (8, 4, 4)] {
        scene.environment_samples = total;
        for seed in 0..16 {
            // FixedBsdf 不消耗 RNG；剩余随机数必须恰好对应 ne 次真实 HDRI 调用。
            let material = FixedBsdf::new(Vector3::z(), 0.37);
            let environment = scene.environment.as_ref().unwrap();
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
            let actual =
                environment_lighting(&scene, &hit, &material, Vector3::z(), &mut rng).unwrap();
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
    scene.environment_samples = 3;
    let material = FixedBsdf::new(Vector3::z(), 0.37);
    let hit = Hit {
        normal: Vector3::z(),
        ..Default::default()
    };
    // BSDF +Z；两个 HDRI 均匀分支分别取 z=1 和 z=0.5，没有策略选择随机数。
    let mut rng = WordRng([u64::MAX, 0, 0, u64::MAX, 0, 1 << 62].into());
    let actual = environment_lighting(&scene, &hit, &material, Vector3::z(), &mut rng).unwrap();
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
    scene.environment_samples = 3;
    let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
    let hit = Hit {
        normal: Vector3::z(),
        ..Default::default()
    };
    // 背面 wo 让材质合法返回 None、PDF=0、BSDF=0；两个 HDRI 配额仍全部执行。
    let mut rng = WordRng([u64::MAX, 0, 0, u64::MAX, 0, u64::MAX].into());
    let actual = environment_lighting(&scene, &hit, &material, -Vector3::z(), &mut rng).unwrap();
    assert_eq!(actual, Vector3::zeros());
    assert!(rng.0.is_empty());

    struct MissingFirst(FixedBsdf);
    impl Material for MissingFirst {
        fn sample(
            &self,
            wo: Vector3<f64>,
            normal: Vector3<f64>,
            rng: &mut dyn RngCore,
        ) -> Option<MaterialSample> {
            let sample = self.0.sample(wo, normal, rng);
            (self.0.samples.load(Ordering::Relaxed) > 1)
                .then_some(sample)
                .flatten()
        }
        fn pdf(&self, wi: Vector3<f64>, wo: Vector3<f64>, normal: Vector3<f64>) -> f64 {
            self.0.pdf(wi, wo, normal)
        }
        fn evaluate(
            &self,
            wi: Vector3<f64>,
            wo: Vector3<f64>,
            normal: Vector3<f64>,
            mode: EvaluateMode,
        ) -> BsdfValue {
            self.0.evaluate(wi, wo, normal, mode)
        }
    }
    scene.environment_samples = 4;
    let material = MissingFirst(FixedBsdf::new(Vector3::z(), 0.25));
    // 第一个 BSDF None、第二个 BSDF 有效；HDRI 分别为正上方和背半球。
    let mut rng = WordRng([u64::MAX, 0, 0, u64::MAX, 0, u64::MAX].into());
    let actual = environment_lighting(&scene, &hit, &material, Vector3::z(), &mut rng).unwrap();
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
        ..Default::default()
    };
    let expected = albedo.component_mul(&Vector3::new(8.0, 4.0, 2.0));
    for total in [2, 3, 4, 8] {
        scene.environment_samples = total;
        let mut mean = Vector3::zeros();
        for seed in 0..4096 {
            mean += environment_lighting(
                &scene,
                &hit,
                &material,
                Vector3::z(),
                &mut StdRng::seed_from_u64(seed),
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
        ..Default::default()
    };
    for total in [2, 3, 8] {
        scene.environment_samples = total;
        for pdf in [-1.0, f64::NAN, f64::INFINITY] {
            let material = FixedBsdf::new(Vector3::z(), pdf);
            assert!(
                environment_lighting(
                    &scene,
                    &hit,
                    &material,
                    Vector3::z(),
                    &mut StdRng::seed_from_u64(42)
                )
                .is_err()
            );
        }
        let mut material = FixedBsdf::new(Vector3::z(), 0.37);
        material.value = BsdfValue::Rgb(Vector3::repeat(f64::INFINITY));
        assert!(
            environment_lighting(
                &scene,
                &hit,
                &material,
                Vector3::z(),
                &mut StdRng::seed_from_u64(42)
            )
            .is_err()
        );
    }
    scene.environment_samples = 8;
    // p_b 本身有限，但 n_b*p_b 溢出时必须拒绝。
    let material = FixedBsdf::new(Vector3::z(), f64::MAX);
    assert!(
        environment_lighting(
            &scene,
            &hit,
            &material,
            Vector3::z(),
            &mut StdRng::seed_from_u64(42)
        )
        .is_err()
    );
}

pub(super) fn reference_at(
    scene: &Scene,
    position: Vector3<f64>,
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
            if visible_to_light(scene, position, Vector3::z(), wi).unwrap() {
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
            scene.add(Box::new(HalfPlane(Ground::new(1.0).unwrap())));
        }
        let reference = reference_at(&scene, Vector3::zeros(), Vector3::repeat(0.8), 1024, 512).x;
        let coarse = reference_at(&scene, Vector3::zeros(), Vector3::repeat(0.8), 512, 256).x;
        assert!((reference - coarse).abs() < reference * 0.005);
        let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
        let hit = Hit {
            normal: Vector3::z(),
            ..Default::default()
        };
        for total in [2, 3, 4, 8] {
            scene.environment_samples = total;
            let mut mse = [0.0; 2];
            let mut mean = 0.0;
            for seed in 0..64 {
                let mut rng = StdRng::seed_from_u64(seed);
                let mut estimate = 0.0;
                for path in 0..512 {
                    let value =
                        environment_lighting(&scene, &hit, &material, Vector3::z(), &mut rng)
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

pub(super) fn partial_scene(hdr: &TemporaryHdr) -> Scene {
    let mut scene = diffuse_scene(hdr);
    // 相机位于遮挡平面下方，主光线不碰遮挡物；向上环境光只有部分可见。
    scene.camera = Camera::from_config(&CameraConfig {
        position: [0.0, 0.0, 0.5],
        target: [0.0; 3],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 35.0,
    })
    .unwrap();
    scene.add(Box::new(HalfPlane(Ground::new(1.0).unwrap())));
    scene
}

// 均匀天空、z=1 且 y>0 的半平面遮挡：可见辐照度比例为
// 0.5*(1-y/sqrt(h²+y²))。像素的 y 区间解析平均，包含亚像素抖动面积平均。
pub(super) fn partial_reference_image(scene: &Scene, width: usize, height: usize) -> Framebuffer {
    let mut image = Framebuffer::new(width, height);
    let ground = Ground::new(0.0).unwrap();
    let h = 1.0 - 10.0 * INTERSECTION_EPSILON;
    for row in 0..height {
        let edge_y = |offset_y| {
            let ray = scene
                .camera
                .ray_for_pixel_sample(0, row, width, height, Vector2::new(0.5, offset_y))
                .unwrap();
            ground.intersect(&ray).unwrap().coords.y
        };
        let y0 = edge_y(0.0);
        let y1 = edge_y(1.0 - f64::EPSILON);
        let factor = 0.5 * (1.0 - (y0 + y1) / (h.hypot(y0) + h.hypot(y1)));
        let xyz = linear_srgb_to_xyz(Vector3::new(0.8, 0.6, 0.4) * factor).unwrap();
        image.pixels[row * width..(row + 1) * width].fill(xyz);
    }
    image
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

#[test]
fn stratified_diffuse_harness_matches_production_camera_for_all_quotas() {
    let hdr = TemporaryHdr::high_contrast();
    let mut scene = diffuse_scene(&hdr);
    for total in [1, 2, 3, 4, 8] {
        scene.environment_samples = total;
        let harness = render_diffuse(&scene, 4, 3, 8, 42, environment_lighting);
        let production = render_camera_glass(&scene, 4, 3, 8, 42).unwrap();
        assert_eq!(harness.pixels, production.pixels);
    }
}

#[test]
#[ignore = "release 比较：cargo test --release --locked compare_stratified_sampling -- --ignored --nocapture"]
fn compare_stratified_sampling() {
    const WIDTH: usize = 32;
    const HEIGHT: usize = 24;
    const SPP: usize = 32;
    const SEEDS: usize = 16;
    println!(
        "D3 comparison: {WIDTH}x{HEIGHT}, spp={SPP}, seeds=0..{SEEDS}, serial, metrics=linear XYZ Y"
    );
    let cases = [
        ("uniform", TemporaryHdr::uniform([1.0; 3])),
        ("high-contrast", TemporaryHdr::high_contrast()),
        ("partial-uniform", TemporaryHdr::uniform([1.0; 3])),
    ];
    for (case, hdr) in cases {
        let mut scene = if case == "partial-uniform" {
            partial_scene(&hdr)
        } else {
            diffuse_scene(&hdr)
        };
        let reference = if case == "partial-uniform" {
            partial_reference_image(&scene, WIDTH, HEIGHT)
        } else {
            let fine = reference_radiance(&scene, Vector3::new(0.8, 0.6, 0.4), 2048, 1024);
            let coarse = reference_radiance(&scene, Vector3::new(0.8, 0.6, 0.4), 1024, 512);
            assert!((fine - coarse).norm() < fine.norm() * 0.005);
            println!(
                "case={case}, numerical_reference_relative_delta={:.8}",
                (fine - coarse).norm() / fine.norm()
            );
            let mut image = Framebuffer::new(WIDTH, HEIGHT);
            image.pixels.fill(linear_srgb_to_xyz(fine).unwrap());
            image
        };
        let reference_y =
            reference.pixels.iter().map(|p| p.y).sum::<f64>() / (WIDTH * HEIGHT) as f64;
        for total in [2, 4, 8] {
            scene.environment_samples = total;
            let estimators = [
                random_mixture_environment_lighting as Estimator,
                environment_lighting as Estimator,
            ];
            for estimator in estimators {
                let _ = render_diffuse(&scene, WIDTH, HEIGHT, 2, 100, estimator);
            }
            let mut seconds = [0.0; 2];
            let mut mse = [0.0; 2];
            let mut means = [0.0; 2];
            let count = (WIDTH * HEIGHT * SEEDS) as f64;
            for seed in 0..SEEDS {
                for index in if seed % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let started = Instant::now();
                    let image =
                        render_diffuse(&scene, WIDTH, HEIGHT, SPP, seed as u64, estimators[index]);
                    seconds[index] += started.elapsed().as_secs_f64();
                    for (pixel, expected) in image.pixels.iter().zip(&reference.pixels) {
                        mse[index] += (pixel.y - expected.y).powi(2) / count;
                        means[index] += pixel.y / count;
                    }
                }
            }
            for (index, name) in ["D2-random", "D3-stratified"].into_iter().enumerate() {
                println!(
                    "case={case}, N={total}, estimator={name}, reference_Y={reference_y:.8}, mean_Y={:.8}, bias_Y={:.8}, RMSE_Y={:.8}, relative_RMSE={:.8}, mean_frame_ms={:.6}, ns_per_direction={:.3}",
                    means[index],
                    means[index] - reference_y,
                    mse[index].sqrt(),
                    mse[index].sqrt() / reference_y,
                    seconds[index] * 1000.0 / SEEDS as f64,
                    seconds[index] * 1e9 / (WIDTH * HEIGHT * SPP * SEEDS * total) as f64
                );
            }
            println!(
                "case={case}, N={total}, time_ratio={:.6}, MSE_ratio={:.6}, equal_error_time_ratio_estimate={:.6}",
                seconds[1] / seconds[0],
                mse[1] / mse[0],
                seconds[1] * mse[1] / (seconds[0] * mse[0])
            );
        }
    }
}

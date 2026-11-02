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
    scene.environment_samples = 3;
    scene.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
    let hit = Hit {
        normal: Vector3::z(),
        ..Default::default()
    };
    for pb in [0.0, 0.37] {
        let material = FixedBsdf::new(Vector3::z(), pb);
        let mut rng = WordRng([u64::MAX, 0, 0, u64::MAX, 0, 1 << 62].into());
        let actual = environment_lighting(&scene, &hit, &material, Vector3::z(), &mut rng).unwrap();
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
fn power_preserves_d2_single_sample_gradient_and_direction_rng() {
    let hdr = TemporaryHdr::high_contrast();
    let mut scene = hdr.scene(37.0);
    let material = Lambertian::new(Vector3::new(0.2, 0.5, 0.8)).unwrap();
    let hit = Hit {
        normal: Vector3::z(),
        ..Default::default()
    };
    for total in [1, 2, 3, 4, 8] {
        scene.environment_samples = total;
        for seed in 0..32 {
            let mut a = StdRng::seed_from_u64(seed);
            let mut b = a.clone();
            scene.environment_mis_heuristic = EnvironmentMisHeuristic::Balance;
            let balance =
                environment_lighting(&scene, &hit, &material, Vector3::z(), &mut a).unwrap();
            scene.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
            let power =
                environment_lighting(&scene, &hit, &material, Vector3::z(), &mut b).unwrap();
            assert_eq!(a.next_u64(), b.next_u64());
            if total == 1 {
                assert_eq!(balance.map(f64::to_bits), power.map(f64::to_bits));
            }
        }
    }
    scene.environment = Scene::new().environment;
    for total in [1, 3, 8] {
        scene.environment_samples = total;
        let mut a = StdRng::seed_from_u64(42);
        let mut b = a.clone();
        scene.environment_mis_heuristic = EnvironmentMisHeuristic::Balance;
        let balance = environment_lighting(&scene, &hit, &material, Vector3::z(), &mut a).unwrap();
        scene.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
        let power = environment_lighting(&scene, &hit, &material, Vector3::z(), &mut b).unwrap();
        assert_eq!(balance.map(f64::to_bits), power.map(f64::to_bits));
        assert_eq!(a.next_u64(), b.next_u64());
    }
}

#[test]
fn power_keeps_zero_back_hemisphere_and_occluded_quotas() {
    let hdr = TemporaryHdr::uniform([1.0; 3]);
    let mut scene = hdr.scene(0.0);
    scene.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
    scene.environment_samples = 2;
    let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
    let hit = Hit {
        normal: Vector3::z(),
        ..Default::default()
    };
    let mut rng = WordRng([0, 0, u64::MAX, 0, u64::MAX].into());
    let actual = environment_lighting(&scene, &hit, &material, Vector3::z(), &mut rng).unwrap();
    assert!((actual - Vector3::repeat(0.8 * 16.0 / 17.0)).norm() < 1e-12);
    assert!(rng.0.is_empty());
    scene.add(Box::new(Ground::new(1.0).unwrap()));
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
    let black = TemporaryHdr::uniform([0.0; 3]);
    let mut scene = black.scene(0.0);
    scene.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
    for total in [2, 3, 8] {
        scene.environment_samples = total;
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
}

#[test]
fn power_estimates_converge_to_uniform_and_high_contrast_references() {
    for (label, hdr) in [
        ("uniform", TemporaryHdr::uniform([1.0; 3])),
        ("high", TemporaryHdr::high_contrast()),
    ] {
        let mut scene = hdr.scene(37.0);
        scene.environment_mis_heuristic = EnvironmentMisHeuristic::Power;
        let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
        let hit = Hit {
            normal: Vector3::z(),
            ..Default::default()
        };
        let reference = reference_radiance(&scene, Vector3::repeat(0.8), 1024, 512).x;
        for total in [2, 3, 8] {
            scene.environment_samples = total;
            let mut mean = 0.0;
            let mut mse = 0.0;
            for seed in 0..64 {
                let mut rng = StdRng::seed_from_u64(seed);
                let mut value = 0.0;
                for _ in 0..512 {
                    value += environment_lighting(&scene, &hit, &material, Vector3::z(), &mut rng)
                        .unwrap()
                        .x
                        / 512.0;
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

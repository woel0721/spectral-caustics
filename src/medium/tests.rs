use super::*;
use rand::{Rng, RngCore, SeedableRng, rngs::StdRng};
use std::f64::consts::PI;

fn medium(a: f64, s: f64) -> HomogeneousMedium {
    HomogeneousMedium::try_from(&HomogeneousMediumConfig {
        min_m: [-1.0; 3],
        max_m: [1.0; 3],
        sigma_a: a.into(),
        sigma_s: s.into(),
        g: 0.0,
    })
    .unwrap()
}

#[test]
fn intervals_clip_inside_outside_parallel_tangent_and_surface_segments() {
    let m = medium(0.2, 0.3);
    let ray = |o, d| Ray::new(o, d).unwrap();
    let r = ray(Vector3::new(0.0, 0.0, 3.0), -Vector3::z());
    assert_eq!(m.interval(&r, f64::INFINITY), Some((2.0, 4.0)));
    assert_eq!(m.interval(&r, 3.0), Some((2.0, 3.0)));
    assert_eq!(m.interval(&r, 2.0), None);
    assert_eq!(
        m.interval(&ray(Vector3::zeros(), Vector3::x()), 9.0),
        Some((0.0, 1.0))
    );
    assert_eq!(
        m.interval(&ray(Vector3::new(2.0, 0.0, 0.0), Vector3::z()), 9.0),
        None
    );
    assert_eq!(m.interval(&ray(Vector3::z(), Vector3::z()), 9.0), None);
    assert_eq!(
        m.interval(&ray(Vector3::new(1.0, 0.0, 0.0), Vector3::z()), 9.0),
        Some((0.0, 1.0))
    );
    let diagonal = ray(Vector3::repeat(-2.0), Vector3::repeat(1.0));
    let (a, b) = m.interval(&diagonal, 20.0).unwrap();
    assert!((b - a - 2.0 * 3.0_f64.sqrt()).abs() < 1e-12);
    assert!((m.transmittance_ray(&r, 3.0, 550.0) - (-0.5_f64).exp()).abs() < 1e-15);
}

#[test]
fn beer_lambert_and_configuration_validate_physical_parameters() {
    let m = medium(0.2, 0.3);
    assert_eq!(m.coefficients(550.0).sigma_a, 0.2);
    assert_eq!(m.coefficients(550.0).sigma_s, 0.3);
    assert_eq!(m.sigma_t_majorant(550.0), 0.5);
    assert_eq!(m.transmittance(0.0, 550.0), 1.0);
    assert_eq!(m.transmittance(2.0, 550.0), (-1.0_f64).exp());
    assert_eq!(m.transmittance(f64::INFINITY, 550.0), 0.0);
    assert_eq!(medium(0.0, 0.0).transmittance(f64::INFINITY, 550.0), 1.0);
    let c = HomogeneousMediumConfig {
        min_m: [-1.0; 3],
        max_m: [1.0; 3],
        sigma_a: 0.0.into(),
        sigma_s: 0.1.into(),
        g: 0.7,
    };
    for bad in [
        HomogeneousMediumConfig {
            sigma_a: (-0.1).into(),
            ..c.clone()
        },
        HomogeneousMediumConfig {
            sigma_s: f64::NAN.into(),
            ..c.clone()
        },
        HomogeneousMediumConfig {
            sigma_a: f64::MAX.into(),
            sigma_s: f64::MAX.into(),
            ..c.clone()
        },
        HomogeneousMediumConfig {
            g: 1.0,
            ..c.clone()
        },
        HomogeneousMediumConfig {
            g: -1.0,
            ..c.clone()
        },
        HomogeneousMediumConfig {
            min_m: [1.0; 3],
            ..c.clone()
        },
        HomogeneousMediumConfig {
            max_m: [f64::INFINITY; 3],
            ..c
        },
    ] {
        assert!(HomogeneousMedium::try_from(&bad).is_err());
    }
}

#[test]
fn homogeneous_curves_reject_bad_knots_and_combined_overflow() {
    let mut config = HomogeneousMediumConfig {
        min_m: [-1.0; 3],
        max_m: [1.0; 3],
        sigma_a: 0.2.into(),
        sigma_s: 0.1.into(),
        g: 0.0,
    };
    for samples in [
        vec![],
        vec![[381.0, 0.5], [780.0, 0.6]],
        vec![[380.0, 0.5], [779.0, 0.6]],
        vec![[380.0, 0.5], [380.0, 0.6], [780.0, 0.8]],
        vec![[380.0, 0.5], [510.0, 0.9], [480.0, 0.6], [780.0, 0.8]],
        vec![[380.0, 0.5], [f64::NAN, 0.6], [780.0, 0.8]],
        vec![[380.0, 0.5], [780.0, -0.1]],
        vec![[380.0, 0.5], [780.0, f64::INFINITY]],
    ] {
        config.sigma_a = MediumCoefficient::Curve { samples };
        assert!(HomogeneousMedium::try_from(&config).is_err());
    }
    config.sigma_a = MediumCoefficient::Curve {
        samples: vec![[380.0, 0.0], [500.0, f64::MAX], [780.0, 0.0]],
    };
    config.sigma_s = MediumCoefficient::Curve {
        samples: vec![[380.0, 0.0], [600.0, f64::MAX], [780.0, 0.0]],
    };
    assert!(HomogeneousMedium::try_from(&config).is_err());

    config.sigma_a = MediumCoefficient::Curve {
        samples: vec![[380.0, 0.5], [610.0, 0.5], [780.0, 0.5]],
    };
    config.sigma_s = 0.0.into();
    let flat = HomogeneousMedium::try_from(&config).unwrap();
    assert!(!flat.is_wavelength_dependent());
    assert_eq!(flat.sigma_t_majorant(380.0), 0.5);
    assert_eq!(flat.sigma_t_majorant(780.0), 0.5);
}

#[test]
fn free_flight_survival_density_and_implicit_absorption_conserve_probability() {
    let m = medium(0.2, 0.3);
    let r = Ray::new(Vector3::new(0.0, 0.0, 2.0), -Vector3::z()).unwrap();
    let mut rng = StdRng::seed_from_u64(42);
    let n = 150_000;
    let (mut escaped, mut scatter_weight, mut distance) = (0, 0.0, 0.0);
    for _ in 0..n {
        let s = m.sample_segment(&r, 3.0, 550.0, true, &mut rng).unwrap();
        if let Some(p) = s.position {
            scatter_weight += s.weight;
            distance += 1.0 - p.z;
        } else {
            escaped += 1;
            assert_eq!(s.weight, 1.0);
        }
    }
    let tr = m.transmittance(2.0, 550.0);
    assert!((escaped as f64 / n as f64 - tr).abs() < 0.004);
    assert!((scatter_weight / n as f64 - 0.6 * (1.0 - tr)).abs() < 0.003);
    let conditional_mean = 1.0 / m.sigma_t_majorant(550.0) - 2.0 * tr / (1.0 - tr);
    assert!((distance / (n - escaped) as f64 - conditional_mean).abs() < 0.008);
    let mut untouched = rng.clone();
    let capped = m.sample_segment(&r, 3.0, 550.0, false, &mut rng).unwrap();
    assert!(capped.position.is_none());
    assert_eq!(capped.weight, tr);
    let absorption = medium(0.5, 0.0)
        .sample_segment(&r, 3.0, 550.0, true, &mut rng)
        .unwrap();
    assert!(absorption.position.is_none());
    assert_eq!(absorption.weight, tr);
    let vacuum = medium(0.0, 0.0)
        .sample_segment(&r, 3.0, 550.0, true, &mut rng)
        .unwrap();
    assert!(vacuum.position.is_none());
    assert_eq!(vacuum.weight, 1.0);
    assert_eq!(rng.next_u64(), untouched.next_u64());
}

#[test]
fn hg_is_normalized_reciprocal_and_has_the_pbrt_forward_sign() {
    let wo = Vector3::z();
    for g in [-0.9, -0.7, -1e-8, 0.0, 1e-8, 0.7, 0.9] {
        let phase = HenyeyGreenstein::new(g).unwrap();
        let n = 40_000;
        let mut integral = 0.0;
        for i in 0..n {
            let c = -1.0 + 2.0 * (i as f64 + 0.5) / n as f64;
            let wi = Vector3::new((1.0 - c * c).sqrt(), 0.0, c);
            let p = phase.eval(wo, wi);
            assert_eq!(p, phase.pdf(wo, wi));
            assert_eq!(p, phase.eval(wi, wo));
            if g == 0.0 {
                assert_eq!(p, 1.0 / (4.0 * PI));
            }
            integral += p * 4.0 * PI / n as f64;
        }
        assert!((integral - 1.0).abs() < 2e-5, "g={g}: {integral}");
        if g > 0.0 {
            assert!(phase.eval(wo, -wo) > phase.eval(wo, wo));
        }
    }
}

#[test]
fn hg_samples_match_independent_pdf_bin_integrals_and_moments() {
    let wo = Vector3::new(0.2, -0.3, 0.9).normalize();
    for g in [-0.9, -0.7, -1e-8, 0.0, 1e-8, 0.7, 0.9] {
        let phase = HenyeyGreenstein::new(g).unwrap();
        let mut rng = StdRng::seed_from_u64(918);
        let n = 120_000;
        let mut bins = [0; 20];
        let (mut first, mut second) = (0.0, 0.0);
        for _ in 0..n {
            let s = phase.sample(wo, &mut rng);
            assert!((s.wi.norm() - 1.0).abs() < 1e-12);
            assert_eq!(s.p, s.pdf);
            assert_eq!(s.pdf, phase.eval(wo, s.wi));
            let c = wo.dot(&s.wi).clamp(-1.0, 1.0);
            bins[(((c + 1.0) * 10.0) as usize).min(19)] += 1;
            first += c;
            second += c * c;
        }
        assert!((first / n as f64 + g).abs() < 0.006);
        assert!((second / n as f64 - (1.0 + 2.0 * g * g) / 3.0).abs() < 0.005);
        for (b, count) in bins.iter().enumerate() {
            let mut expected = 0.0;
            for j in 0..2000 {
                let c = -1.0 + 0.1 * (b as f64 + (j as f64 + 0.5) / 2000.0);
                let wi = Vector3::new((1.0 - c * c).sqrt(), 0.0, c);
                expected += phase.pdf(Vector3::z(), wi) * 2.0 * PI * 0.1 / 2000.0;
            }
            assert!(
                (*count as f64 / n as f64 - expected).abs() < 0.006,
                "g={g}, bin={b}"
            );
        }
    }
}

#[test]
fn hg_inverse_cdf_keeps_the_zero_sample_endpoint_near_extreme_asymmetry() {
    struct ZeroRng;
    impl rand::RngCore for ZeroRng {
        fn next_u32(&mut self) -> u32 {
            0
        }
        fn next_u64(&mut self) -> u64 {
            0
        }
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            dest.fill(0);
        }
    }
    for g in [0.0, 0.7, -0.7, 1.0 - 1e-12, -1.0 + 1e-12] {
        let phase = HenyeyGreenstein::new(g).unwrap();
        let sample = phase.sample(Vector3::z(), &mut ZeroRng);
        let expected = if g < 0.0 { -Vector3::z() } else { Vector3::z() };
        assert!(
            (sample.wi - expected).norm() < 1e-12,
            "g={g}: {:?}",
            sample.wi
        );
        assert!(sample.pdf.is_finite() && sample.pdf > 0.0);
    }
}

fn config(resolution: [usize; 3], densities: Vec<f64>) -> GridMediumConfig {
    GridMediumConfig {
        min_m: [0.0; 3],
        max_m: [2.0; 3],
        resolution,
        densities,
        sigma_a_base: 0.2.into(),
        sigma_s_base: 0.3.into(),
        g: 0.4,
    }
}

fn ray(origin: [f64; 3], direction: [f64; 3]) -> Ray {
    Ray::new(origin.into(), direction.into()).unwrap()
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 2e-13, "{actual} != {expected}");
}

#[test]
fn grid_queries_use_world_coordinates_x_fastest_and_closed_outer_bounds() {
    let mut c = config([2, 3, 2], (0..12).map(|i| i as f64).collect());
    c.min_m = [-2.0, 1.0, 4.0];
    c.max_m = [2.0, 7.0, 6.0];
    let grid = GridMedium::try_from(&c).unwrap();
    for z in 0..2 {
        for y in 0..3 {
            for x in 0..2 {
                let p = Vector3::new(-1.0 + 2.0 * x as f64, 2.0 + 2.0 * y as f64, 4.5 + z as f64);
                let density = (x + 2 * (y + 3 * z)) as f64;
                assert_eq!(grid.density_at(p), density);
                let s = grid.coefficients_at(p, 550.0);
                assert_eq!(s.sigma_a, density * 0.2);
                assert_eq!(s.sigma_s, density * 0.3);
                assert!(s.sigma_t() <= grid.sigma_t_majorant(550.0));
            }
        }
    }
    assert_eq!(grid.density_at(Vector3::new(0.0, 3.0, 5.0)), 9.0);
    assert_eq!(grid.density_at(c.max_m.into()), 11.0);
    assert_eq!(grid.density_at(c.min_m.into()), 0.0);
    for p in [
        Vector3::new(-2.01, 3.0, 5.0),
        Vector3::new(0.0, 7.01, 5.0),
        Vector3::repeat(f64::NAN),
    ] {
        assert_eq!(grid.density_at(p), 0.0);
        assert_eq!(
            grid.coefficients_at(p, 550.0),
            MediumCoefficients::default()
        );
    }
    assert_eq!(grid.sigma_t_majorant(550.0), 11.0 * 0.2 + 11.0 * 0.3);
    let m: Box<dyn Medium> = Box::new(grid);
    assert_eq!(m.density_at(c.max_m.into()), 11.0);
    assert_eq!(
        m.coefficients_at(c.max_m.into(), 550.0).sigma_t(),
        m.sigma_t_majorant(550.0)
    );
    assert_eq!(m.phase().g(), 0.4);
}

#[test]
fn spectral_validation_rejects_rounding_overflow_between_finite_knots() {
    let curve = |a, b| MediumCoefficient::Curve {
        samples: vec![[380.0, a], [780.0, b]],
    };
    let near_max = f64::MAX * (17.0 / 32.0);
    let complement = f64::MAX - near_max;
    // Both knot extinctions are finite, but independent interpolation of these
    // curves makes their sum overflow at 449 nm.
    assert_eq!(near_max + complement, f64::MAX);
    let homogeneous = HomogeneousMediumConfig {
        min_m: [0.0; 3],
        max_m: [1.0; 3],
        sigma_a: curve(0.0, near_max),
        sigma_s: curve(f64::MAX, complement),
        g: 0.0,
    };
    let grid = GridMediumConfig {
        sigma_a_base: curve(0.0, near_max * 0.5),
        sigma_s_base: curve(f64::MAX * 0.5, complement * 0.5),
        ..config([1; 3], vec![2.0])
    };
    assert!(HomogeneousMedium::try_from(&homogeneous).is_err());
    assert!(GridMedium::try_from(&grid).is_err());
    assert_eq!(medium(f64::MAX, 0.0).sigma_t_majorant(449.0), f64::MAX);
}

#[test]
fn grid_validation_rejects_invalid_density_shape_bounds_and_scaled_overflow() {
    let c = config([2, 1, 1], vec![0.0, 1.0]);
    let mut invalid = vec![
        GridMediumConfig {
            resolution: [0, 1, 1],
            ..c.clone()
        },
        GridMediumConfig {
            resolution: [usize::MAX, 2, 1],
            ..c.clone()
        },
        GridMediumConfig {
            densities: vec![1.0],
            ..c.clone()
        },
        GridMediumConfig {
            min_m: [2.0; 3],
            ..c.clone()
        },
        GridMediumConfig {
            max_m: [f64::INFINITY; 3],
            ..c.clone()
        },
        GridMediumConfig {
            sigma_a_base: (-1.0).into(),
            ..c.clone()
        },
        GridMediumConfig {
            sigma_s_base: f64::NAN.into(),
            ..c.clone()
        },
        GridMediumConfig {
            sigma_a_base: f64::MAX.into(),
            sigma_s_base: f64::MAX.into(),
            ..c.clone()
        },
        GridMediumConfig {
            g: 1.0,
            ..c.clone()
        },
        GridMediumConfig {
            min_m: [1e16, 0.0, 0.0],
            max_m: [1e16 + 2.0, 2.0, 2.0],
            ..c.clone()
        },
        GridMediumConfig {
            densities: vec![0.0, f64::MAX],
            sigma_a_base: 1.0.into(),
            sigma_s_base: 1.0.into(),
            ..c.clone()
        },
        GridMediumConfig {
            densities: vec![0.0, 3.0],
            sigma_a_base: MediumCoefficient::Curve {
                samples: vec![[380.0, 0.1], [490.0, f64::MAX / 2.0], [780.0, 0.1]],
            },
            ..c.clone()
        },
    ];
    for density in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        invalid.push(GridMediumConfig {
            densities: vec![0.0, density],
            ..c.clone()
        });
    }
    for c in invalid {
        assert!(GridMedium::try_from(&c).is_err(), "{c:?}");
    }
}

#[test]
fn nonbinary_voxel_faces_and_their_adjacent_floats_have_consistent_density() {
    let mut c = config([3, 1, 1], vec![1.0, 2.0, 3.0]);
    c.min_m[0] = 0.1;
    c.max_m[0] = 2.2;
    let grid = GridMedium::try_from(&c).unwrap();
    for i in 1..3 {
        let face = c.min_m[0] + (c.max_m[0] - c.min_m[0]) * (i as f64 / 3.0);
        assert_eq!(
            grid.density_at(Vector3::new(face.next_down(), 1.0, 1.0)),
            i as f64
        );
        assert_eq!(
            grid.density_at(Vector3::new(face, 1.0, 1.0)),
            (i + 1) as f64
        );
        assert_eq!(
            grid.density_at(Vector3::new(face.next_up(), 1.0, 1.0)),
            (i + 1) as f64
        );
    }
}

#[test]
fn vacuum_cells_do_not_produce_nan_when_ray_exit_distance_overflows() {
    let mut c = config([2, 1, 1], vec![1.0, 0.0]);
    c.max_m[0] = f64::MAX;
    let sigma_a = 1.0 / f64::MAX;
    c.sigma_a_base = sigma_a.into();
    c.sigma_s_base = 0.0.into();
    let r = ray([-f64::MAX / 2.0, 1.0, 1.0], [1.0, 0.0, 0.0]);
    let grid = GridMedium::try_from(&c).unwrap();
    close(
        grid.transmittance_ray(&r, f64::INFINITY, 550.0),
        (-sigma_a * (f64::MAX / 2.0)).exp(),
    );
    c.densities.fill(0.0);
    assert_eq!(
        GridMedium::try_from(&c)
            .unwrap()
            .transmittance_ray(&r, f64::INFINITY, 550.0),
        1.0
    );
}

#[test]
fn two_layer_transmittance_matches_analytic_depth_in_both_directions_and_partial_segments() {
    let grid = GridMedium::try_from(&config([2, 1, 1], vec![1.0, 3.0])).unwrap();
    let forward = ray([-1.0, 1.0, 1.0], [1.0, 0.0, 0.0]);
    let reverse = ray([3.0, 1.0, 1.0], [-1.0, 0.0, 0.0]);
    close(
        grid.transmittance_ray(&forward, f64::INFINITY, 550.0),
        (-2.0_f64).exp(),
    );
    close(
        grid.transmittance_ray(&reverse, f64::INFINITY, 550.0),
        (-2.0_f64).exp(),
    );
    close(
        grid.transmittance_ray(&forward, 2.5, 550.0),
        (-1.25_f64).exp(),
    );
    close(
        grid.transmittance_ray(&reverse, 1.5, 550.0),
        (-0.75_f64).exp(),
    );
    for (d, tau) in [(1.0, 1.5_f64), (-1.0, 0.5_f64)] {
        close(
            grid.transmittance_ray(&ray([1.0; 3], [d, 0.0, 0.0]), 10.0, 550.0),
            (-tau).exp(),
        );
    }
    close(
        grid.transmittance_ray(&ray([0.5; 3], [1.0, 0.0, 0.0]), 0.25, 550.0),
        (-0.125_f64).exp(),
    );
}

#[test]
fn two_layer_spectral_transmittance_uses_both_curves_at_the_path_wavelength() {
    let mut c = config([2, 1, 1], vec![1.0, 3.0]);
    c.sigma_a_base = MediumCoefficient::Curve {
        samples: vec![[380.0, 0.2], [580.0, 0.6], [780.0, 0.2]],
    };
    c.sigma_s_base = MediumCoefficient::Curve {
        samples: vec![[380.0, 0.8], [480.0, 0.5], [680.0, 0.1], [780.0, 0.0]],
    };
    let grid = GridMedium::try_from(&c).unwrap();
    let forward = ray([-1.0, 1.0, 1.0], [1.0, 0.0, 0.0]);
    let reverse = ray([3.0, 1.0, 1.0], [-1.0, 0.0, 0.0]);
    for (nm, extinction) in [(480.0, 0.9_f64), (580.0, 0.9), (680.0, 0.5), (780.0, 0.2)] {
        for r in [forward, reverse] {
            close(
                grid.transmittance_ray(&r, f64::INFINITY, nm),
                (-4.0 * extinction).exp(),
            );
        }
        close(
            grid.transmittance_ray(&forward, 2.5, nm),
            (-2.5 * extinction).exp(),
        );
        close(
            grid.transmittance_ray(&reverse, 2.5, nm),
            (-3.5 * extinction).exp(),
        );
    }
}

#[test]
fn clipping_handles_misses_inside_parallel_boundary_tangent_and_zero_length() {
    let grid = GridMedium::try_from(&config([2, 1, 1], vec![1.0, 3.0])).unwrap();
    let r = ray([-1.0, 1.0, 1.0], [1.0, 0.0, 0.0]);
    assert_eq!(grid.interval(&r, 10.0), Some((1.0, 3.0)));
    assert_eq!(grid.interval(&r, 2.0), Some((1.0, 2.0)));
    for distance in [0.0, 0.5, 1.0] {
        assert_eq!(grid.transmittance_ray(&r, distance, 550.0), 1.0);
    }
    for r in [
        ray([-1.0, 3.0, 1.0], [1.0, 0.0, 0.0]),
        ray([2.0, 1.0, 1.0], [1.0, 0.0, 0.0]),
        ray([-1.0, 1.0, 1.0], [1.0, 1.0, 0.0]), // corner touch only
    ] {
        assert_eq!(grid.transmittance_ray(&r, 10.0, 550.0), 1.0);
    }
    // On max x face, parallel to it: last cell, same closed-box convention as homogeneous.
    close(
        grid.transmittance_ray(&ray([2.0, -1.0, 1.0], [0.0, 1.0, 0.0]), 10.0, 550.0),
        (-3.0_f64).exp(),
    );
}

#[test]
fn uniform_grid_degenerates_to_homogeneous_for_axes_edges_corners_and_oblique_rays() {
    let density = 1.7;
    for resolution in [[1, 1, 1], [2, 2, 2], [3, 4, 5]] {
        let mut c = config(resolution, vec![density; resolution.iter().product()]);
        c.min_m = [-1.2, -2.0, 0.3];
        c.max_m = [2.1, 3.0, 4.3];
        let grid = GridMedium::try_from(&c).unwrap();
        let homogeneous = HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: c.min_m,
            max_m: c.max_m,
            sigma_a: (density * 0.2).into(),
            sigma_s: (density * 0.3).into(),
            g: c.g,
        })
        .unwrap();
        for r in [
            ray([-3.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            ray([3.0, 0.0, 1.0], [-1.0, 0.0, 0.0]),
            ray([0.0, 0.0, 1.0], [0.0, -1.0, 0.0]),
            ray([0.0, 0.0, 1.0], [0.7, -1.3, 2.1]),
            Ray::new(
                c.min_m.into(),
                Vector3::from(c.max_m) - Vector3::from(c.min_m),
            )
            .unwrap(),
        ] {
            for distance in [0.0, 0.2, 2.0, 9.0, f64::INFINITY] {
                close(
                    grid.transmittance_ray(&r, distance, 550.0),
                    homogeneous.transmittance_ray(&r, distance, 550.0),
                );
            }
        }
    }
    // Different densities in all eight cells: exact corner crossings skip the six
    // cells touched at a point, giving lengths sqrt(3) in cells 0 and 7 only.
    let grid = GridMedium::try_from(&config(
        [2; 3],
        vec![1., 100., 100., 100., 100., 100., 100., 3.],
    ))
    .unwrap();
    for r in [ray([-1.0; 3], [1.0; 3]), ray([3.0; 3], [-1.0; 3])] {
        close(
            grid.transmittance_ray(&r, 20.0, 550.0),
            (-2.0 * 3.0_f64.sqrt()).exp(),
        );
    }
}

#[test]
fn dda_matches_independent_sum_of_individual_voxel_slab_integrals() {
    let mut c = config(
        [3, 4, 5],
        (0..60).map(|i| ((i * 17) % 23) as f64 / 7.0).collect(),
    );
    c.min_m = [-1.3, -0.7, 0.1];
    c.max_m = [2.2, 3.1, 4.8];
    let grid = GridMedium::try_from(&c).unwrap();
    let mut rng = StdRng::seed_from_u64(193);
    for _ in 0..800 {
        let r = Ray::new(
            Vector3::from_fn(|_, _| rng.random_range(-4.0..6.0)),
            Vector3::from_fn(|_, _| rng.random_range(-1.0..1.0)),
        )
        .unwrap();
        let distance = rng.random_range(0.0..15.0);
        let mut tau = 0.0;
        for z in 0..5 {
            for y in 0..4 {
                for x in 0..3 {
                    let cell = [x, y, z];
                    let bounds = |offset: usize| {
                        std::array::from_fn(|i| {
                            c.min_m[i]
                                + (c.max_m[i] - c.min_m[i])
                                    * ((cell[i] + offset) as f64 / c.resolution[i] as f64)
                        })
                    };
                    let density = c.densities[x + 3 * (y + 4 * z)];
                    let voxel = HomogeneousMedium::try_from(&HomogeneousMediumConfig {
                        min_m: bounds(0),
                        max_m: bounds(1),
                        sigma_a: (density * 0.2).into(),
                        sigma_s: (density * 0.3).into(),
                        g: c.g,
                    })
                    .unwrap();
                    if let Some((a, b)) = voxel.interval(&r, distance) {
                        tau += voxel.sigma_t_majorant(550.0) * (b - a);
                    }
                }
            }
        }
        close(grid.transmittance_ray(&r, distance, 550.0), (-tau).exp());
    }
}

#[test]
fn absorption_vacuum_and_budget_zero_use_exact_tr_without_rng() {
    let mut c = config([2, 1, 1], vec![1.0, 3.0]);
    c.sigma_a_base = 0.5.into();
    c.sigma_s_base = 0.0.into();
    let r = ray([-1.0, 1.0, 1.0], [1.0, 0.0, 0.0]);
    let mut rng = StdRng::seed_from_u64(42);
    let mut untouched = rng.clone();
    let absorption: Box<dyn Medium> = Box::new(GridMedium::try_from(&c).unwrap());
    for allow in [false, true] {
        let s = absorption
            .sample_segment(&r, 10.0, 550.0, allow, &mut rng)
            .unwrap();
        assert!(s.position.is_none());
        close(s.weight, (-2.0_f64).exp());
    }
    c.sigma_a_base = 0.2.into();
    c.sigma_s_base = 0.3.into();
    let scattering: Box<dyn Medium> = Box::new(GridMedium::try_from(&c).unwrap());
    close(
        scattering
            .sample_segment(&r, 10.0, 550.0, false, &mut rng)
            .unwrap()
            .weight,
        (-2.0_f64).exp(),
    );
    c.densities.fill(0.0);
    let vacuum: Box<dyn Medium> = Box::new(GridMedium::try_from(&c).unwrap());
    assert_eq!(vacuum.sigma_t_majorant(550.0), 0.0);
    assert_eq!(
        vacuum
            .sample_segment(&r, f64::INFINITY, 550.0, true, &mut rng)
            .unwrap()
            .weight,
        1.0
    );
    c.sigma_a_base = 0.0.into();
    c.sigma_s_base = 0.0.into();
    c.densities.fill(3.0);
    assert_eq!(
        GridMedium::try_from(&c)
            .unwrap()
            .transmittance_ray(&r, f64::INFINITY, 550.0),
        1.0
    );
    assert_eq!(rng.next_u64(), untouched.next_u64());
}

#[test]
fn delta_tracking_collision_cdf_and_weights_match_piecewise_optical_depth() {
    let mut c = config([3, 1, 1], vec![0.0, 1.0, 3.0]);
    c.max_m[0] = 3.0;
    c.sigma_a_base = 0.25.into();
    c.sigma_s_base = 0.75.into();
    let grid = GridMedium::try_from(&c).unwrap();
    // Independent integrated optical depths at distances from each ray's entry.
    // Include vacuum, reverse traversal, a start inside a cell and a surface clip.
    let cases = [
        (
            ray([-1.0, 1.0, 1.0], [1.0, 0.0, 0.0]),
            10.0,
            vec![
                (1.0, 0.0_f64),
                (1.5, 0.5),
                (2.0, 1.0),
                (2.5, 2.5),
                (3.0, 4.0),
            ],
        ),
        (
            ray([4.0, 1.0, 1.0], [-1.0, 0.0, 0.0]),
            10.0,
            vec![(0.5, 1.5), (1.0, 3.0), (1.5, 3.5), (3.0, 4.0)],
        ),
        (
            ray([1.5, 1.0, 1.0], [1.0, 0.0, 0.0]),
            10.0,
            vec![(0.25, 0.25), (0.5, 0.5), (1.0, 2.0), (1.5, 3.5)],
        ),
        (
            ray([-1.0, 1.0, 1.0], [1.0, 0.0, 0.0]),
            3.5,
            vec![(1.5, 0.5), (2.0, 1.0), (2.5, 2.5)],
        ),
    ];
    let n = 100_000;
    let mut rng = StdRng::seed_from_u64(391);
    for (r, limit, references) in cases {
        let (enter, exit) = grid.interval(&r, limit).unwrap();
        let mut counts = vec![0; references.len()];
        let mut escaped = 0;
        let mut weights = 0.0;
        for _ in 0..n {
            let s = grid
                .sample_segment(&r, limit, 550.0, true, &mut rng)
                .unwrap();
            if let Some(p) = s.position {
                let t = (p - r.origin).dot(&r.direction);
                assert!(t >= enter && t < exit);
                assert!(grid.density_at(p) > 0.0);
                assert_eq!(s.weight, 0.75);
                weights += s.weight;
                for (count, &(distance, _)) in counts.iter_mut().zip(&references) {
                    *count += usize::from(t - enter < distance);
                }
            } else {
                escaped += 1;
                assert_eq!(s.weight, 1.0); // Survival PDF already includes Tr.
            }
        }
        for (count, &(_, tau)) in counts.iter().zip(&references) {
            let expected = 1.0 - (-tau).exp();
            let observed = *count as f64 / n as f64;
            let tolerance = 6.0 * (expected * (1.0 - expected) / n as f64).sqrt() + 1e-4;
            assert!(
                (observed - expected).abs() < tolerance,
                "{observed} vs {expected}"
            );
        }
        let tr = (-references.last().unwrap().1).exp();
        assert!((escaped as f64 / n as f64 - tr).abs() < 0.005);
        assert!((weights / n as f64 - 0.75 * (1.0 - tr)).abs() < 0.004);
    }
}

#[test]
fn delta_tracking_uniform_grid_and_homogeneous_have_the_same_distance_statistics() {
    let mut c = config([3, 4, 5], vec![1.7; 60]);
    c.min_m = [-1.0; 3];
    c.max_m = [1.0; 3];
    let grid = GridMedium::try_from(&c).unwrap();
    let homogeneous = HomogeneousMedium::try_from(&HomogeneousMediumConfig {
        min_m: c.min_m,
        max_m: c.max_m,
        sigma_a: (1.7 * 0.2).into(),
        sigma_s: (1.7 * 0.3).into(),
        g: c.g,
    })
    .unwrap();
    let r = ray([-2.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
    let n = 100_000;
    let tr = (-1.7_f64).exp();
    let expected_distance = 1.0 / 0.85 - 2.0 * tr / (1.0 - tr);
    for m in [&grid as &dyn Medium, &homogeneous as &dyn Medium] {
        let mut rng = StdRng::seed_from_u64(77);
        let (mut events, mut distance) = (0, 0.0);
        for _ in 0..n {
            let s = m.sample_segment(&r, 10.0, 550.0, true, &mut rng).unwrap();
            if let Some(p) = s.position {
                events += 1;
                distance += p.x + 1.0;
                close(s.weight, 0.6);
            }
        }
        assert!((events as f64 / n as f64 - (1.0 - tr)).abs() < 0.005);
        assert!((distance / events as f64 - expected_distance).abs() < 0.008);
    }
}

#[test]
fn delta_tracking_missed_and_empty_segments_do_not_consume_rng() {
    let grid = GridMedium::try_from(&config([2, 1, 1], vec![0.0, 3.0])).unwrap();
    let mut rng = StdRng::seed_from_u64(9);
    let mut untouched = rng.clone();
    for (r, limit) in [
        (ray([-1.0, 3.0, 1.0], [1.0, 0.0, 0.0]), 10.0),
        (ray([-1.0, 1.0, 1.0], [1.0, 0.0, 0.0]), 1.0),
        (ray([1.0; 3], [1.0, 0.0, 0.0]), 0.0),
        (ray([2.0, 1.0, 1.0], [1.0, 0.0, 0.0]), 10.0),
    ] {
        let s = grid
            .sample_segment(&r, limit, 550.0, true, &mut rng)
            .unwrap();
        assert!(s.position.is_none());
        assert_eq!(s.weight, 1.0);
    }
    assert_eq!(rng.next_u64(), untouched.next_u64());
}

#[test]
fn delta_tracking_reports_unrepresentable_distance_instead_of_stalling() {
    let mut c = config([1; 3], vec![1.0]);
    c.sigma_a_base = 0.0.into();
    c.sigma_s_base = 1e300.into();
    let grid = GridMedium::try_from(&c).unwrap();
    let r = ray([-1.0, 1.0, 1.0], [1.0, 0.0, 0.0]);
    assert!(
        grid.sample_segment(&r, 10.0, 550.0, true, &mut StdRng::seed_from_u64(1))
            .unwrap_err()
            .contains("cannot advance")
    );
    c.max_m[0] = f64::MAX;
    c.sigma_s_base = (1.0 / f64::MAX).into();
    let grid = GridMedium::try_from(&c).unwrap();
    let r = ray([-f64::MAX / 2.0, 1.0, 1.0], [1.0, 0.0, 0.0]);
    assert!(
        grid.sample_segment(
            &r,
            f64::INFINITY,
            550.0,
            true,
            &mut StdRng::seed_from_u64(1)
        )
        .unwrap_err()
        .contains("finite ray distances")
    );
}

#[test]
fn homogeneous_trait_dispatch_preserves_queries_and_free_flight_rng() {
    let homogeneous = HomogeneousMedium::try_from(&HomogeneousMediumConfig {
        min_m: [0.0; 3],
        max_m: [2.0; 3],
        sigma_a: 0.2.into(),
        sigma_s: 0.3.into(),
        g: 0.7,
    })
    .unwrap();
    let m: &dyn Medium = &homogeneous;
    assert_eq!(m.density_at(Vector3::repeat(1.0)), 1.0);
    assert_eq!(m.density_at(Vector3::repeat(3.0)), 0.0);
    assert_eq!(
        m.coefficients_at(Vector3::repeat(1.0), 550.0).sigma_t(),
        m.sigma_t_majorant(550.0)
    );
    let mut a = StdRng::seed_from_u64(34);
    let mut b = a.clone();
    let r = ray([-1.0, 1.0, 1.0], [1.0, 0.0, 0.0]);
    for _ in 0..128 {
        let expected = homogeneous
            .sample_segment(&r, 10.0, 550.0, true, &mut a)
            .unwrap();
        let actual = m.sample_segment(&r, 10.0, 550.0, true, &mut b).unwrap();
        assert_eq!(actual.position, expected.position);
        assert_eq!(actual.weight, expected.weight);
    }
    assert_eq!(a.next_u64(), b.next_u64());
}

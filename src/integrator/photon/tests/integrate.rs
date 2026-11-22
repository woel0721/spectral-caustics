use super::super::emission::{
    PhotonPlan, block_batch_size_for_threads, event_rng_from_base, spectral_sample_blocks,
};
use super::super::integrate::{integrate_spectral, integrate_xyz, irradiance_layer};
use crate::caustics::map::ground_splat_weights;
use crate::caustics::map::ground_to_pixel;
use crate::config::RenderJob;
use crate::config::render::RenderSetting;
use crate::film::Framebuffer;
use crate::geometry::ray::Ray;
use crate::optics::dispersion::IorMode;
use nalgebra::Vector3;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;
use std::error::Error;

const SAMPLING_SEED: u64 = 42;
const MAX_SPECTRAL_BLOCKS: usize = 16;

// 这些能量/落点参考值使用垂直光源，独立于展示场景的默认斜射光。
fn vertical_light_scene() -> RenderJob {
    let mut scene = RenderJob::new();
    scene.scene.light.as_parallel_mut().unwrap().direction = -Vector3::z();
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

use crate::geometry::ground::Ground;
use crate::geometry::prism::Prism;
use crate::output::save_png_with_exposure;
use rand::rngs::StdRng;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[test]
fn emission_blocks_seek_both_parallel_and_disk_random_dimensions_exactly() {
    for dimensions in [2, 4] {
        for max_blocks in [1, 2, 7] {
            let mut reference = StdRng::seed_from_u64(SAMPLING_SEED);
            for (rows, mut rng) in
                spectral_sample_blocks(3, 9, SAMPLING_SEED, max_blocks, dimensions)
            {
                for _ in rows {
                    for _ in 0..3 {
                        for _ in 0..dimensions {
                            assert_eq!(reference.random::<f64>(), rng.random::<f64>());
                        }
                    }
                }
            }
        }
    }
}

struct CountedGround {
    ground: Ground,
    calls: Arc<AtomicUsize>,
}

impl crate::geometry::object::Hittable for CountedGround {
    fn intersect(&self, ray: &Ray) -> Option<crate::geometry::hit::GeometryHit> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.ground.intersect(ray)
    }
}

#[test]
fn constant_ior_spectral_absorption_reuses_geometry_for_xyz_and_atlas() {
    use crate::geometry::sphere::Sphere;
    use crate::material::{dielectric::Dielectric, lambertian::Lambertian};
    use crate::medium::{HomogeneousMedium, HomogeneousMediumConfig, MediumCoefficient};

    let mut job = vertical_light_scene();
    let target = job.scene.add(
        Box::new(Sphere::new(Vector3::z(), 0.8).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let ground = job.scene.add(
        Box::new(CountedGround {
            ground: Ground::new(0.0).unwrap(),
            calls: calls.clone(),
        }),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    job.photon.ground_id = Some(ground);
    job.select_caustics_object(target).unwrap();
    let mut settings = RenderSetting {
        width: 10,
        height: 8,
        samples_per_pixel_axis: 2,
        mode: IorMode::Constant,
        wavelength_step_nm: 100.0,
        ..Default::default()
    };
    let clear = integrate_xyz(&job, Some(target), ground, &settings).unwrap();
    assert!(clear.caustics.pixels().iter().any(|p| p.y > 0.0));
    let spatial_samples = settings.width * settings.height * settings.samples_per_pixel_axis.pow(2);
    let clear_queries = calls.swap(0, Ordering::Relaxed);
    assert!(clear_queries > spatial_samples);

    job.scene.medium = Some(Box::new(
        HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: [-10.0; 3],
            max_m: [10.0; 3],
            sigma_a: MediumCoefficient::Curve {
                samples: vec![[380.0, 1.0], [780.0, 0.1]],
            },
            sigma_s: 0.0.into(),
            g: 0.0.into(),
        })
        .unwrap(),
    ));
    for step in [100.0, 10.0] {
        settings.wavelength_step_nm = step;
        let xyz = integrate_xyz(&job, Some(target), ground, &settings).unwrap();
        assert!(xyz.caustics.pixels().iter().any(|p| p.y > 0.0));
        assert_eq!(calls.swap(0, Ordering::Relaxed), clear_queries);
        let atlas = integrate_spectral(&job, &settings).unwrap().unwrap();
        assert_eq!(
            atlas.wavelength_samples().len(),
            (400.0 / step) as usize + 1
        );
        assert_eq!(calls.swap(0, Ordering::Relaxed), clear_queries);
        let direct_only = integrate_xyz(&job, None, ground, &settings).unwrap();
        assert!(
            direct_only
                .direct
                .unwrap()
                .pixels()
                .iter()
                .any(|p| p.y > 0.0)
        );
        assert_eq!(calls.swap(0, Ordering::Relaxed), spatial_samples);
    }
}

#[test]
fn oblique_sun_and_photon_flux_share_the_horizontal_window_scale() {
    let mut scene = RenderJob::new();
    scene.scene.light.as_parallel_mut().unwrap().direction = Vector3::new(-3.0, 0.0, -4.0);
    // 整个发射窗口的落点平移 -3*3/4=-2.25 米；接收窗口一起平移。
    scene.photon.receiver.min_m.x -= 2.25;
    scene.photon.receiver.max_m.x -= 2.25;
    let ground_id = scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    let settings = settings_for_size(12, 8);
    let lighting = integrate_xyz(&scene, None, ground_id, &settings).unwrap();
    let photon_flux: f64 = lighting
        .direct
        .as_ref()
        .unwrap()
        .pixels
        .iter()
        .map(|p| p.y)
        .sum::<f64>()
        * scene.photon.receiver.area()
        / (settings.width * settings.height) as f64;
    let analytic_e = lighting.sun_irradiance_xyz.y * lighting.light_wi.z;
    assert!((photon_flux - scene.scene.light.as_parallel().unwrap().total_flux).abs() < 1e-12);
    assert!(
        (analytic_e
            - scene.scene.light.as_parallel().unwrap().total_flux / scene.photon.receiver.area())
        .abs()
            < 1e-12
    );
    assert!((lighting.sun_irradiance_xyz.y - 0.625).abs() < 1e-12);
}

#[test]
fn split_lighting_rejects_invalid_sun_receiver_and_merge_data() {
    let mut scene = RenderJob::new();
    let ground = scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    let settings = settings_for_size(2, 2);
    for direction in [
        Vector3::x(),
        Vector3::z(),
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
    ] {
        scene.scene.light.as_parallel_mut().unwrap().direction = direction;
        assert!(integrate_xyz(&scene, None, ground, &settings).is_err());
    }
    scene.scene.light.as_parallel_mut().unwrap().direction = -Vector3::z();
    let mut lighting = integrate_xyz(&scene, None, ground, &settings).unwrap();
    lighting.caustics.pixels[0].x = f64::NAN;
    assert!(lighting.combined().is_err());
    lighting.caustics = Framebuffer::new(1, 1).unwrap();
    assert!(lighting.combined().is_err());
    scene.photon.receiver.max_m = scene.photon.receiver.min_m;
    assert!(integrate_xyz(&scene, None, ground, &settings).is_err());
}

#[test]
fn spectral_blocks_cover_every_row_and_replay_the_original_random_sequence() {
    for (samples_x, samples_y) in [(1, 1), (7, 5), (3, 17), (5, 33)] {
        let blocks =
            spectral_sample_blocks(samples_x, samples_y, SAMPLING_SEED, MAX_SPECTRAL_BLOCKS, 2);
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
    let mut scene = RenderJob::new();
    let caustics_object_id = scene.scene.add(
        Box::new(Prism::new().unwrap()),
        Box::new(
            crate::material::dielectric::Dielectric::with_dispersion(
                1.52,
                Some(crate::optics::dispersion::Cauchy::default()),
            )
            .unwrap(),
        ),
    );
    let ground_id = scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    let serial_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        let reference = serial_pool.install(|| {
            crate::integrator::photon::integrate_xyz(
                &scene,
                Some(caustics_object_id),
                ground_id,
                &sample_settings(&settings, 2, mode),
            )
            .and_then(|images| images.combined())
            .unwrap()
        });
        for threads in [2, 4, 2] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let image = pool.install(|| {
                crate::integrator::photon::integrate_xyz(
                    &scene,
                    Some(caustics_object_id),
                    ground_id,
                    &sample_settings(&settings, 2, mode),
                )
                .and_then(|images| images.combined())
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
        let mut scene = RenderJob::new();
        let caustics_object_id = if case == 2 {
            scene.scene.add(
                Box::new(Ground::new(4.0).unwrap()),
                Box::new(
                    crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                        .unwrap(),
                ),
            )
        } else {
            scene.scene.add(
                Box::new(Prism::new().unwrap()),
                Box::new(
                    crate::material::dielectric::Dielectric::with_dispersion(
                        1.52,
                        Some(crate::optics::dispersion::Cauchy::default()),
                    )
                    .unwrap(),
                ),
            )
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let ground_id = scene.scene.add(
            Box::new(CountedGround {
                ground: Ground::new(if case == 2 { 5.0 } else { 2.0 }).unwrap(),
                calls: calls.clone(),
            }),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        if case == 1 {
            scene.scene.add(
                Box::new(Ground::new(2.5).unwrap()),
                Box::new(
                    crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                        .unwrap(),
                ),
            );
        }
        let samples_per_pixel_axis = 2;
        let spatial_ray_count =
            settings.width * settings.height * samples_per_pixel_axis * samples_per_pixel_axis;
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            calls.store(0, Ordering::Relaxed);
            let image = crate::integrator::photon::integrate_xyz(
                &scene,
                Some(caustics_object_id),
                ground_id,
                &sample_settings(&settings, samples_per_pixel_axis, mode),
            )
            .and_then(|images| images.combined())
            .unwrap();
            assert_eq!(calls.load(Ordering::Relaxed), spatial_ray_count);
            assert_eq!(image.pixels.iter().any(|xyz| xyz.y > 0.0), case == 0);
        }
    }
}

#[test]
fn spectral_unblocked_white_preserves_xyz_flux_across_spatial_sample_counts() {
    let white_xyz = Vector3::new(0.9501416822353593, 1.0, 1.087976622255359);
    for (width, height) in [(1, 1), (1, 4), (4, 1), (12, 8)] {
        let mut scene = vertical_light_scene();
        let ground = scene.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        for samples in [1, 3] {
            for mode in [IorMode::Constant, IorMode::Dispersion] {
                let settings = sample_settings(&settings_for_size(width, height), samples, mode);
                let image =
                    crate::integrator::photon::integrate_xyz(&scene, None, ground, &settings)
                        .and_then(|images| images.combined())
                        .unwrap();
                let flux: Vector3<f64> = image.pixels.iter().copied().sum::<Vector3<f64>>()
                    * scene.photon.receiver.area()
                    / (width * height) as f64;
                assert!(
                    (flux - white_xyz * scene.scene.light.as_parallel().unwrap().total_flux).norm()
                        < 2e-12
                );
            }
        }
    }
}

#[test]
fn spectral_constant_stays_white_and_dispersion_separates_colors_reproducibly() {
    let settings = settings_for_size(128, 16);
    let mut scene = RenderJob::new();
    let caustics_object_id = scene.scene.add(
        Box::new(Prism::new().unwrap()),
        Box::new(
            crate::material::dielectric::Dielectric::with_dispersion(
                1.52,
                Some(crate::optics::dispersion::Cauchy::default()),
            )
            .unwrap(),
        ),
    );
    let ground_id = scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    let constant = crate::integrator::photon::integrate_xyz(
        &scene,
        Some(caustics_object_id),
        ground_id,
        &sample_settings(&settings, 2, IorMode::Constant),
    )
    .and_then(|images| images.combined())
    .unwrap();
    let dispersion = crate::integrator::photon::integrate_xyz(
        &scene,
        Some(caustics_object_id),
        ground_id,
        &sample_settings(&settings, 2, IorMode::Dispersion),
    )
    .and_then(|images| images.combined())
    .unwrap();
    let repeated = crate::integrator::photon::integrate_xyz(
        &scene,
        Some(caustics_object_id),
        ground_id,
        &sample_settings(&settings, 2, IorMode::Dispersion),
    )
    .and_then(|images| images.combined())
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
-> Result<(), Box<dyn Error + Send + Sync>> {
    let settings = settings_for_size(128, 16);
    let mut scene = RenderJob::new();
    let caustics_object_id = scene.scene.add(
        Box::new(Prism::new().unwrap()),
        Box::new(
            crate::material::dielectric::Dielectric::with_dispersion(
                1.52,
                Some(crate::optics::dispersion::Cauchy::default()),
            )
            .unwrap(),
        ),
    );
    let ground_id = scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("spectral-png-{}-{timestamp}", std::process::id()));
    for (mode, filename) in [
        (IorMode::Constant, "constant.png"),
        (IorMode::Dispersion, "dispersion.png"),
    ] {
        let image = crate::integrator::photon::integrate_xyz(
            &scene,
            Some(caustics_object_id),
            ground_id,
            &sample_settings(&settings, 2, mode),
        )
        .and_then(|images| images.combined())?;
        let path = directory.join(filename);
        save_png_with_exposure(&image, &path, 1.0)?;
        let decoded = image::open(&path)?;
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        let rgb = decoded.into_rgb8();
        assert_eq!(rgb.dimensions(), (128, 16));
        let channel_spread =
            |pixel: &image::Rgb<u8>| pixel.0.iter().max().unwrap() - pixel.0.iter().min().unwrap();
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
    let mut scene = RenderJob::new();
    let caustics_object_id = scene.scene.add(
        Box::new(Prism::new().unwrap()),
        Box::new(
            crate::material::dielectric::Dielectric::with_dispersion(
                1.52,
                Some(crate::optics::dispersion::Cauchy::default()),
            )
            .unwrap(),
        ),
    );
    let ground_id = scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        for (caustics_object_id, ground_id, samples) in [
            (usize::MAX, ground_id, 1),
            (ground_id, ground_id, 1),
            (caustics_object_id, usize::MAX, 1),
            (caustics_object_id, ground_id, 0),
            (caustics_object_id, ground_id, usize::MAX),
        ] {
            assert!(
                crate::integrator::photon::integrate_xyz(
                    &scene,
                    Some(caustics_object_id),
                    ground_id,
                    &sample_settings(&settings, samples, mode)
                )
                .and_then(|images| images.combined())
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
            crate::integrator::photon::integrate_xyz(
                &scene,
                Some(caustics_object_id),
                ground_id,
                &sample_settings(&settings, 2, IorMode::Dispersion)
            )
            .and_then(|images| images.combined())
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
        let actual = ground_to_pixel(position, 3, 2, &RenderJob::new().photon.receiver).unwrap();
        assert!((actual.0 - expected.0).abs() < 1e-12);
        assert!((actual.1 - expected.1).abs() < 1e-12);
    }
    assert_eq!(
        ground_to_pixel(
            Vector3::new(-1.5, 1.0, 0.0),
            4,
            2,
            &RenderJob::new().photon.receiver
        ),
        Some((0.0, 0.0))
    );
    assert_eq!(
        ground_to_pixel(
            Vector3::new(1.5, -1.0, 0.0),
            4,
            2,
            &RenderJob::new().photon.receiver
        ),
        Some((3.0, 1.0))
    );
}

#[test]
fn mapping_rejects_outside_non_finite_and_empty_inputs() {
    for value in [-2.01, 2.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            ground_to_pixel(
                Vector3::new(value, 0.0, 0.0),
                3,
                2,
                &RenderJob::new().photon.receiver
            ),
            None
        );
        assert_eq!(
            ground_to_pixel(
                Vector3::new(0.0, value, 0.0),
                3,
                2,
                &RenderJob::new().photon.receiver
            ),
            None
        );
    }
    assert_eq!(
        ground_to_pixel(
            Vector3::new(0.0, 0.0, f64::NAN),
            3,
            2,
            &RenderJob::new().photon.receiver
        ),
        None
    );
    assert_eq!(
        ground_to_pixel(Vector3::zeros(), 0, 2, &RenderJob::new().photon.receiver),
        None
    );
    assert_eq!(
        ground_to_pixel(Vector3::zeros(), 3, 0, &RenderJob::new().photon.receiver),
        None
    );
}
#[test]
fn deposition_weights_conserve_flux_at_interior_edges_and_single_pixel_receivers() {
    let receiver = RenderJob::new().photon.receiver;
    for (width, height) in [(4, 2), (1, 1), (1, 4), (4, 1)] {
        for position in [
            Vector3::zeros(),
            Vector3::new(-2.0, -2.0, 0.0),
            Vector3::new(2.0 - 1e-10, 2.0 - 1e-10, 0.0),
        ] {
            let weights = ground_splat_weights(position, width, height, &receiver)
                .unwrap()
                .unwrap();
            assert!((weights.iter().map(|&(_, _, w)| w).sum::<f64>() - 1.0).abs() < 1e-14);
            assert!(weights.iter().all(|&(x, y, _)| x < width && y < height));
        }
    }
    let outside = ground_splat_weights(Vector3::new(2.0, 0.0, 0.0), 2, 2, &receiver).unwrap();
    assert!(outside.is_none());
}

#[test]
fn event_rng_from_base_matches_fresh_seeded_chacha12_per_photon() {
    let base = ChaCha12Rng::seed_from_u64(SAMPLING_SEED);
    for index in [0, 1, 7, 1024, 999_999] {
        let mut reused = event_rng_from_base(&base, index);
        let mut fresh = ChaCha12Rng::seed_from_u64(SAMPLING_SEED);
        fresh.set_stream((index as u64) ^ 0x7068_6f74_6f6e_0001);
        for _ in 0..16 {
            assert_eq!(reused.random::<u64>(), fresh.random::<u64>());
        }
    }
}

#[test]
fn non_dispersive_glass_under_dispersion_mode_traces_single_pass_like_constant() {
    use crate::geometry::sphere::Sphere;
    use crate::material::{dielectric::Dielectric, lambertian::Lambertian};
    let mut job = vertical_light_scene();
    let target = job.scene.add(
        Box::new(Sphere::new(Vector3::z(), 0.8).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let ground = job.scene.add(
        Box::new(CountedGround {
            ground: Ground::new(0.0).unwrap(),
            calls: calls.clone(),
        }),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    job.photon.ground_id = Some(ground);
    job.select_caustics_object(target).unwrap();
    let mut settings = RenderSetting {
        width: 8,
        height: 6,
        samples_per_pixel_axis: 2,
        mode: IorMode::Constant,
        wavelength_step_nm: 10.0,
        ..Default::default()
    };
    let constant_xyz = integrate_xyz(&job, Some(target), ground, &settings).unwrap();
    let constant_calls = calls.swap(0, Ordering::Relaxed);
    settings.mode = IorMode::Dispersion;
    let dispersion_xyz = integrate_xyz(&job, Some(target), ground, &settings).unwrap();
    let dispersion_calls = calls.swap(0, Ordering::Relaxed);
    assert_eq!(dispersion_calls, constant_calls);
    assert_eq!(
        dispersion_xyz.caustics.pixels(),
        constant_xyz.caustics.pixels()
    );
}

#[test]
fn block_batch_size_respects_threads_block_count_and_memory_budget() {
    assert_eq!(block_batch_size_for_threads(0, 512 * 512, 1, 8), 0);
    // No temporary image buffers -> run all blocks without batching.
    assert_eq!(block_batch_size_for_threads(64, 4096 * 4096, 0, 4), 64);
    // 512x512 * 8 B = 2 MiB/block -> 64 MiB budget allows 32 blocks; capped by threads/total.
    assert_eq!(block_batch_size_for_threads(32, 512 * 512, 1, 1), 1);
    assert_eq!(block_batch_size_for_threads(32, 512 * 512, 1, 16), 16);
    assert_eq!(block_batch_size_for_threads(3, 512 * 512, 1, 16), 3);
    // 2048x2048 * 8 B = 32 MiB/block -> 64 MiB budget caps single-buffer batch at 2, dual at 1.
    assert_eq!(block_batch_size_for_threads(64, 2048 * 2048, 1, 16), 2);
    assert_eq!(block_batch_size_for_threads(64, 2048 * 2048, 2, 16), 1);
    // Extreme pixel count or zero thread hint degrades cleanly to 1 without overflow.
    assert_eq!(block_batch_size_for_threads(16, usize::MAX, 2, 8), 1);
    assert_eq!(block_batch_size_for_threads(16, 64, 1, 0), 1);
}

#[test]
fn batched_emission_and_parallel_irradiance_layer_are_bit_identical_across_threads() {
    use crate::geometry::sphere::Sphere;
    use crate::material::{dielectric::Dielectric, lambertian::Lambertian};
    use crate::medium::{HomogeneousMedium, HomogeneousMediumConfig, MediumCoefficient};

    let mut job = vertical_light_scene();
    let target = job.scene.add(
        Box::new(Sphere::new(Vector3::z(), 0.8).unwrap()),
        Box::new(
            Dielectric::with_dispersion(1.52, Some(crate::optics::dispersion::Cauchy::default()))
                .unwrap(),
        ),
    );
    let ground = job.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    job.photon.ground_id = Some(ground);
    job.select_caustics_object(target).unwrap();

    // 16 blocks with 1 / 2 / 4 threads exercise 16 / 8 / 4 batched reductions and scratch reuse.
    let constant_settings = RenderSetting {
        width: 12,
        height: 16,
        samples_per_pixel_axis: 2,
        mode: IorMode::Constant,
        max_spectral_blocks: 16,
        ..Default::default()
    };
    let bits_xyz = |pixels: &[Vector3<f64>]| -> Vec<[u64; 3]> {
        pixels
            .iter()
            .map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])
            .collect()
    };
    let bits_f64 = |values: &[f64]| -> Vec<u64> { values.iter().map(|v| v.to_bits()).collect() };

    let mut ref_xyz_direct = None;
    let mut ref_xyz_caustics = None;
    let mut ref_atlas_layer = None;
    for threads in [1, 2, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let xyz = pool
            .install(|| integrate_xyz(&job, Some(target), ground, &constant_settings))
            .unwrap();
        let direct_bits = bits_xyz(xyz.direct.as_ref().unwrap().pixels());
        let caustics_bits = bits_xyz(xyz.caustics.pixels());
        assert!(
            xyz.direct
                .as_ref()
                .unwrap()
                .pixels()
                .iter()
                .any(|p| p.y > 0.0)
        );
        assert!(xyz.caustics.pixels().iter().any(|p| p.y > 0.0));
        if let Some(expected) = &ref_xyz_direct {
            assert_eq!(&direct_bits, expected);
        } else {
            ref_xyz_direct = Some(direct_bits);
        }
        if let Some(expected) = &ref_xyz_caustics {
            assert_eq!(&caustics_bits, expected);
        } else {
            ref_xyz_caustics = Some(caustics_bits);
        }

        let atlas = pool
            .install(|| integrate_spectral(&job, &constant_settings))
            .unwrap()
            .unwrap();
        let layer_bits = bits_f64(atlas.layer_irradiance(0).unwrap());
        assert!(atlas.layer_irradiance(0).unwrap().iter().any(|&v| v > 0.0));
        if let Some(expected) = &ref_atlas_layer {
            assert_eq!(&layer_bits, expected);
        } else {
            ref_atlas_layer = Some(layer_bits);
        }
    }

    // Exercise parallel=true vs parallel=false in irradiance_layer across 1/2/4 threads
    // when both spectral caustics and spectral direct hits are active.
    job.scene.medium = Some(Box::new(
        HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: [-10.0; 3],
            max_m: [10.0; 3],
            sigma_a: MediumCoefficient::Curve {
                samples: vec![[380.0, 0.8], [780.0, 0.1]],
            },
            sigma_s: 0.0.into(),
            g: 0.0.into(),
        })
        .unwrap(),
    ));
    let spectral_settings = RenderSetting {
        mode: IorMode::Dispersion,
        ..constant_settings.clone()
    };
    let plan = PhotonPlan::new(&job, &spectral_settings, Some(target), ground, true).unwrap();
    assert!(plan.fixed_direct.is_none());
    assert!(plan.fixed_caustics.is_none());
    let serial =
        irradiance_layer(&job, &spectral_settings, &plan, 550.0, true, true, false).unwrap();
    let serial_caustics = bits_f64(serial.caustics.as_deref().unwrap());
    let serial_direct = bits_f64(serial.direct.as_deref().unwrap());
    assert!(serial.caustics.as_deref().unwrap().iter().any(|&v| v > 0.0));
    assert!(serial.direct.as_deref().unwrap().iter().any(|&v| v > 0.0));
    for threads in [1, 2, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let parallel = pool
            .install(|| irradiance_layer(&job, &spectral_settings, &plan, 550.0, true, true, true))
            .unwrap();
        assert_eq!(
            bits_f64(parallel.caustics.as_deref().unwrap()),
            serial_caustics
        );
        assert_eq!(bits_f64(parallel.direct.as_deref().unwrap()), serial_direct);
    }

    // Single-pass caustics with zero hits keeps fixed_caustics lazily None and yields zero atlas.
    let mut missed = vertical_light_scene();
    let far_target = missed.scene.add(
        Box::new(Sphere::new(Vector3::new(100.0, 0.0, 1.0), 0.5).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let missed_ground = missed.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    missed.photon.ground_id = Some(missed_ground);
    missed.select_caustics_object(far_target).unwrap();
    let missed_plan = PhotonPlan::new(
        &missed,
        &constant_settings,
        Some(far_target),
        missed_ground,
        false,
    )
    .unwrap();
    assert!(missed_plan.fixed_direct.is_none());
    assert!(missed_plan.fixed_caustics.is_none());
    let missed_atlas = integrate_spectral(&missed, &constant_settings)
        .unwrap()
        .unwrap();
    assert!(
        missed_atlas
            .layer_irradiance(0)
            .unwrap()
            .iter()
            .all(|&v| v == 0.0)
    );
}

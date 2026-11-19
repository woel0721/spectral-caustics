use nalgebra::{Vector2, Vector3};
use spectral_caustics::caustics::SpectralCausticsMap;
use spectral_caustics::config::Config;
use spectral_caustics::config::RenderJob;
use spectral_caustics::config::render::{CameraMode, RenderSetting};
use spectral_caustics::config::scene::EnvironmentMisHeuristic;
use spectral_caustics::film::Framebuffer;
use spectral_caustics::geometry::{ground::Ground, vertical_prism::VerticalPrism};
use spectral_caustics::integrator::photon::integrate_xyz;
use spectral_caustics::material::{dielectric::Dielectric, lambertian::Lambertian};
use spectral_caustics::medium::{
    GridMediumConfig, HomogeneousMedium, HomogeneousMediumConfig, Medium, MediumConfig,
};
use spectral_caustics::optics::dispersion::{Cauchy, IorMode};
use spectral_caustics::renderer::Renderer;
use spectral_caustics::scene::MediumInterface;
use spectral_caustics::spectrum::cie_xyz;
use spectral_caustics::spectrum::d65_normalization;
use spectral_caustics::spectrum::d65_spd;
use spectral_caustics::spectrum::emission::d65_normalization_continuous;

fn config(source: &str) -> Config {
    let mut c = Config::from_json(source).unwrap();
    c.render.width = 16;
    c.render.height = 12;
    c.render.samples_per_pixel_axis = 2;
    c.render.camera_spp = 8;
    c.render.wavelength_step_nm = 100.0;
    c.render.photon_max_events = 16;
    c.render.thread_count = Some(1);
    if let Some(e) = &mut c.scene.environment {
        e.map_path = None;
    }
    c
}

fn same(a: &Framebuffer, b: &Framebuffer) {
    assert_eq!((a.width(), a.height()), (b.width(), b.height()));
    for (a, b) in a.pixels().iter().zip(b.pixels()) {
        assert_eq!(a.map(f64::to_bits), b.map(f64::to_bits));
    }
}

fn map(scene: &RenderJob, settings: &RenderSetting, threads: usize) -> SpectralCausticsMap {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| {
            spectral_caustics::integrator::photon::integrate_spectral(scene, settings)
                .unwrap()
                .unwrap()
        })
}

fn two_layer_photon_scene() -> (Config, GridMediumConfig, f64) {
    let c = config(include_str!("../configs/sphere.json"));
    // Both layers are below the emitter and above the sphere and camera. Every
    // landing crosses them once along the incident beam [0.45, 0, -1].
    let grid = GridMediumConfig {
        min_m: [-10.0, -10.0, 2.7],
        max_m: [10.0, 10.0, 2.9],
        resolution: [1, 1, 2],
        densities: vec![1.0, 3.0],
        sigma_a_base: 0.2,
        sigma_s_base: 0.3,
        g: 0.7,
    };
    let tr = (-0.5 * (0.1 + 0.3) * 1.0_f64.hypot(0.45)).exp();
    (c, grid, tr)
}

fn assert_frame_attenuated(clear: &Framebuffer, fog: &Framebuffer, tr: f64, tolerance: f64) {
    assert!(clear.pixels().iter().any(|p| p.y > 0.0));
    for (clear, fog) in clear.pixels().iter().zip(fog.pixels()) {
        assert!((clear * tr - fog).norm() <= tolerance * clear.norm());
    }
}

#[test]
fn photon_xyz_atlas_and_direct_preview_apply_the_same_analytic_extinction() {
    let (mut c, grid, tr) = two_layer_photon_scene();
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        c.scene.medium = None;
        let clear = c.build().unwrap();
        let target = clear.photon.caustics_object_id;
        let ground = clear.photon.ground_id.unwrap();
        let xyz = |job| integrate_xyz(job, target, ground, &c.render).unwrap();
        let clear_xyz = xyz(&clear);
        let clear_atlas = map(&clear, &c.render, 1);
        let mut vacuum = grid.clone();
        vacuum.densities.fill(0.0);
        c.scene.medium = Some(MediumConfig::Grid(vacuum));
        let vacuum = c.build().unwrap();
        let vacuum_xyz = xyz(&vacuum);
        let vacuum_atlas = map(&vacuum, &c.render, 2);
        same(&clear_xyz.caustics, &vacuum_xyz.caustics);
        same(
            clear_xyz.direct.as_ref().unwrap(),
            vacuum_xyz.direct.as_ref().unwrap(),
        );

        c.scene.medium = Some(MediumConfig::Grid(grid.clone()));
        let fog = c.build().unwrap();
        let fog_xyz = xyz(&fog);
        assert_frame_attenuated(&clear_xyz.caustics, &fog_xyz.caustics, tr, 2e-12);
        assert_frame_attenuated(
            clear_xyz.direct.as_ref().unwrap(),
            fog_xyz.direct.as_ref().unwrap(),
            tr,
            2e-12,
        );
        assert_eq!(clear_xyz.sun_irradiance_xyz, fog_xyz.sun_irradiance_xyz);
        let fog_atlas = map(&fog, &c.render, 4);
        for (layer, _) in clear_atlas.wavelength_samples().iter().enumerate() {
            let clear = clear_atlas.layer_irradiance(layer).unwrap();
            let vacuum = vacuum_atlas.layer_irradiance(layer).unwrap();
            let fog = fog_atlas.layer_irradiance(layer).unwrap();
            // The source spectrum can have zero density at individual nodes.
            for ((&a, &b), &e) in clear.iter().zip(vacuum).zip(fog) {
                assert_eq!(a.to_bits(), b.to_bits());
                assert!((a * tr - e).abs() <= 2e-12 * a);
            }
        }
    }
}

#[test]
fn bound_five_micron_plate_attenuates_xyz_atlas_and_direct_from_each_launch_side() {
    let bottom = 1.0;
    let top = bottom + 5e-6;
    let sigma_exterior = 0.2;
    let sigma_interior = 140_000.0;
    let absorption = |sigma_a| -> Box<dyn Medium> {
        Box::new(
            HomogeneousMedium::try_from(&HomogeneousMediumConfig {
                min_m: [-10.0; 3],
                max_m: [10.0; 3],
                sigma_a,
                sigma_s: 0.0,
                g: 0.0,
            })
            .unwrap(),
        )
    };
    let build = |height_m, ground_height, absorbing: bool| {
        let mut job = RenderJob::new();
        let light = job.scene.light.as_parallel_mut().unwrap();
        light.direction = -Vector3::z();
        light.height_m = height_m;
        let glass = Dielectric::with_dispersion(1.5, Some(Cauchy::default())).unwrap();
        let plate = VerticalPrism::new(
            [
                Vector2::new(-1.0, -1.0),
                Vector2::new(1.0, -1.0),
                Vector2::new(0.0, 1.0),
            ],
            bottom,
            top,
        )
        .unwrap();
        let target = job.scene.add(Box::new(plate), Box::new(glass));
        let ground = job.scene.add(
            Box::new(Ground::new(ground_height).unwrap()),
            Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
        );
        job.photon.ground_id = Some(ground);
        job.select_caustics_object(target).unwrap();
        job.scene
            .bind_medium_interface(
                target,
                MediumInterface {
                    interior: absorbing.then(|| absorption(sigma_interior)),
                },
            )
            .unwrap();
        job.scene.medium = absorbing.then(|| absorption(sigma_exterior));
        job
    };

    for (height, max_events) in [(2.0, 2), ((bottom + top) / 2.0, 1)] {
        let clear = build(height, 0.0, false);
        let absorbing = build(height, 0.0, true);
        // At normal incidence and this event budget, photons landing through the
        // glass make one pass. Rays missing the plate go straight to the ground.
        let glass_length = height.min(top) - bottom;
        let caustics_tr =
            (-sigma_exterior * (height - glass_length) - sigma_interior * glass_length).exp();
        let direct_tr = (-sigma_exterior * height).exp();
        // World-space hit rounding at metre scale is amplified by the large sigma.
        let tolerance = 1e-10;
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            let settings = RenderSetting {
                width: 16,
                height: 12,
                samples_per_pixel_axis: 2,
                wavelength_step_nm: 100.0,
                photon_max_events: max_events,
                mode,
                ..RenderSetting::default()
            };
            let xyz = |job: &RenderJob| {
                integrate_xyz(
                    job,
                    job.photon.caustics_object_id,
                    job.photon.ground_id.unwrap(),
                    &settings,
                )
                .unwrap()
            };
            let clear_xyz = xyz(&clear);
            let step_xyz = xyz(&absorbing);
            assert_frame_attenuated(
                &clear_xyz.caustics,
                &step_xyz.caustics,
                caustics_tr,
                tolerance,
            );
            assert_frame_attenuated(
                clear_xyz.direct.as_ref().unwrap(),
                step_xyz.direct.as_ref().unwrap(),
                direct_tr,
                tolerance,
            );

            let clear_map = map(&clear, &settings, 1);
            let step_map = map(&absorbing, &settings, 1);
            let parallel_map = map(&absorbing, &settings, 4);
            let mut reweighted = vec![Vector3::zeros(); step_xyz.caustics.pixels().len()];
            for (i, &(nm, width)) in step_map.wavelength_samples().iter().enumerate() {
                let clear = clear_map.layer_irradiance(i).unwrap();
                let step = step_map.layer_irradiance(i).unwrap();
                let parallel = parallel_map.layer_irradiance(i).unwrap();
                for (((&clear, &step), &parallel), xyz) in
                    clear.iter().zip(step).zip(parallel).zip(&mut reweighted)
                {
                    assert!((step - clear * caustics_tr).abs() <= tolerance * clear);
                    assert_eq!(step.to_bits(), parallel.to_bits());
                    *xyz += cie_xyz(nm).unwrap() * (step * width);
                }
            }
            for (integrated, expected) in reweighted.iter().zip(step_xyz.caustics.pixels()) {
                assert!((integrated - expected).norm() <= 2e-12 * expected.norm());
            }
        }
    }

    // The source and receiver share the interior of a glass plate used only
    // as a medium boundary. The direct path reaches the receiver before a cap.
    let height = bottom + 4e-6;
    let ground_height = bottom + 1e-6;
    let settings = RenderSetting {
        width: 4,
        height: 4,
        samples_per_pixel_axis: 1,
        ..RenderSetting::default()
    };
    let direct = |absorbing| {
        let mut job = build(height, ground_height, absorbing);
        let light = job.scene.light.as_parallel_mut().unwrap();
        light.aperture_min_m = Vector2::repeat(-0.1);
        light.aperture_max_m = Vector2::repeat(0.1);
        integrate_xyz(&job, None, job.photon.ground_id.unwrap(), &settings)
            .unwrap()
            .direct
            .unwrap()
    };
    let interior_tr = (-sigma_interior * (height - ground_height)).exp();
    assert_frame_attenuated(&direct(false), &direct(true), interior_tr, 1e-10);
}

#[test]
fn attenuated_caustics_repeat_bitwise_in_rgb_hero_and_packet() {
    let (mut c, grid, tr) = two_layer_photon_scene();
    c.scene.max_volume_scatters = 0;
    c.scene.max_diffuse_bounces = 0;
    let clear = c.build().unwrap();
    c.scene.medium = Some(MediumConfig::Grid(grid));
    let fog = c.build().unwrap();
    for (mode, packet) in [
        (CameraMode::Rgb, 1),
        (CameraMode::Spectral, 1),
        (CameraMode::Spectral, 4),
    ] {
        c.render.camera_mode = mode;
        c.render.spectral_packet_size = packet;
        c.render.thread_count = Some(1);
        let renderer = Renderer::new(c.render.clone()).unwrap();
        let clear = renderer.render(&clear).unwrap();
        let reference = renderer.render(&fog).unwrap();
        assert_frame_attenuated(&clear.caustics, &reference.caustics, tr, 2e-12);
        for ((d, a), combined) in reference
            .direct
            .pixels()
            .iter()
            .zip(reference.caustics.pixels())
            .zip(reference.combined.pixels())
        {
            assert_eq!(d + a, *combined);
        }
        for threads in [2, 4] {
            c.render.thread_count = Some(threads);
            let actual = Renderer::new(c.render.clone())
                .unwrap()
                .render(&fog)
                .unwrap();
            same(&reference.direct, &actual.direct);
            same(&reference.caustics, &actual.caustics);
            same(&reference.combined, &actual.combined);
        }
    }
}

#[test]
fn forward_event_budget_rejects_invalid_configuration() {
    for photon_max_events in [0, 1025, usize::MAX] {
        assert!(
            RenderSetting {
                photon_max_events,
                ..RenderSetting::default()
            }
            .validate()
            .is_err()
        );
    }
    let mut value = serde_json::to_value(RenderSetting::default()).unwrap();
    value["photon_trace_mode"] = serde_json::json!("two_transmission");
    assert!(serde_json::from_value::<RenderSetting>(value).is_err());
}

#[test]
fn forward_xyz_and_scalar_maps_share_paths_conserve_flux_and_repeat_across_threads() {
    for source in [
        include_str!("../configs/sphere.json"),
        include_str!("../configs/vertical_prism_spectral_caustics.json"),
        include_str!("../configs/vertical_prism_area_light.json"),
    ] {
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            let mut c = config(source);
            c.render.mode = mode;
            let scene = c.build().unwrap();
            let lighting = spectral_caustics::integrator::photon::integrate_xyz(
                &scene,
                scene.photon.caustics_object_id,
                scene.photon.ground_id.expect("receiver"),
                (Renderer::new(c.render.clone()).unwrap()).settings(),
            )
            .unwrap();
            let atlas = map(&scene, &c.render, 1);
            let mut integral = vec![Vector3::zeros(); lighting.caustics.pixels().len()];
            for (j, &(nm, width)) in atlas.wavelength_samples().iter().enumerate() {
                for (xyz, &e) in integral.iter_mut().zip(atlas.layer_irradiance(j).unwrap()) {
                    *xyz += cie_xyz(nm).unwrap() * (e * width);
                }
            }
            for (a, b) in integral.iter().zip(lighting.caustics.pixels()) {
                assert!((a - b).norm() < 2e-12 * (1.0 + b.norm()));
            }
            let area = scene.photon.receiver.area() / integral.len() as f64;
            let received = lighting
                .combined()
                .unwrap()
                .pixels()
                .iter()
                .map(|p| p.y * area)
                .sum::<f64>();
            let samples = c.render.wavelength_samples().unwrap();
            let k = if scene.scene.light.as_disk().is_some() {
                d65_normalization_continuous()
            } else {
                d65_normalization(&samples).unwrap()
            };
            let emitted = scene.scene.light.total_flux()
                * samples
                    .iter()
                    .map(|&(nm, w)| k * d65_spd(nm).unwrap() * cie_xyz(nm).unwrap().y * w)
                    .sum::<f64>();
            assert!(received > 0.0 && received <= emitted * (1.0 + 1e-12));
            assert!(lighting.caustics.pixels().iter().any(|p| p.y > 0.0));
            for threads in [1, 2, 4] {
                c.render.thread_count = Some(threads);
                let actual = spectral_caustics::integrator::photon::integrate_xyz(
                    &scene,
                    scene.photon.caustics_object_id,
                    scene.photon.ground_id.expect("receiver"),
                    (Renderer::new(c.render.clone()).unwrap()).settings(),
                )
                .unwrap();
                same(
                    lighting.direct.as_ref().unwrap(),
                    actual.direct.as_ref().unwrap(),
                );
                same(&lighting.caustics, &actual.caustics);
                let actual = map(&scene, &c.render, threads);
                for j in 0..atlas.wavelength_samples().len() {
                    assert_eq!(
                        atlas
                            .layer_irradiance(j)
                            .unwrap()
                            .iter()
                            .map(|v| v.to_bits())
                            .collect::<Vec<_>>(),
                        actual
                            .layer_irradiance(j)
                            .unwrap()
                            .iter()
                            .map(|v| v.to_bits())
                            .collect::<Vec<_>>()
                    );
                }
            }
            c.render.max_spectral_blocks = 3;
            let regrouped = map(&scene, &c.render, 4);
            for j in 0..atlas.wavelength_samples().len() {
                for (a, b) in atlas
                    .layer_irradiance(j)
                    .unwrap()
                    .iter()
                    .zip(regrouped.layer_irradiance(j).unwrap())
                {
                    assert!((a - b).abs() <= 1e-14 * a.abs().max(b.abs()).max(1e-20));
                }
            }
        }
    }
}

#[test]
fn camera_components_packet_and_rng_are_deterministic_with_forward_photons() {
    for source in [
        include_str!("../configs/vertical_prism_spectral_caustics.json"),
        include_str!("../configs/vertical_prism_area_light.json"),
    ] {
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            let mut c = config(source);
            c.render.mode = mode;
            c.render.samples_per_pixel_axis = 1;
            for (depth, mis) in [
                (0, EnvironmentMisHeuristic::Balance),
                (4, EnvironmentMisHeuristic::Power),
            ] {
                c.scene.max_diffuse_bounces = depth;
                c.scene.environment_mis_heuristic = mis;
                let scene = c.build().unwrap();
                for packet in [1, 4] {
                    c.render.spectral_packet_size = packet;
                    c.render.thread_count = Some(1);
                    let renderer = Renderer::new(c.render.clone()).unwrap();
                    let reference = renderer.render(&scene).unwrap();
                    let light = spectral_caustics::integrator::photon::integrate_xyz(
                        &scene,
                        scene.photon.caustics_object_id,
                        scene.photon.ground_id.expect("receiver"),
                        (renderer).settings(),
                    )
                    .unwrap();
                    let rgb = spectral_caustics::integrator::camera::integrate_rgb(
                        &scene,
                        &spectral_caustics::config::render::RenderSetting {
                            width: (renderer).settings().width,
                            height: (renderer).settings().height,
                            camera_spp: 8,
                            seed: c.render.seed,
                            ..Default::default()
                        },
                        Some(&light),
                    )
                    .unwrap();
                    for images in [&reference, &rgb] {
                        for ((d, a), combined) in images
                            .direct
                            .pixels()
                            .iter()
                            .zip(images.caustics.pixels())
                            .zip(images.combined.pixels())
                        {
                            // RGB 相机在线性 RGB 中相加再转 XYZ，允许转换的舍入差。
                            assert!((*d + *a - *combined).norm() < 2e-12 * (1.0 + combined.norm()));
                        }
                    }
                    for threads in [2, 4] {
                        c.render.thread_count = Some(threads);
                        let renderer = Renderer::new(c.render.clone()).unwrap();
                        let actual = renderer.render(&scene).unwrap();
                        same(&reference.direct, &actual.direct);
                        same(&reference.caustics, &actual.caustics);
                        same(&reference.combined, &actual.combined);
                        let lighting = spectral_caustics::integrator::photon::integrate_xyz(
                            &scene,
                            scene.photon.caustics_object_id,
                            scene.photon.ground_id.expect("receiver"),
                            (renderer).settings(),
                        )
                        .unwrap();
                        let actual = spectral_caustics::integrator::camera::integrate_rgb(
                            &scene,
                            &spectral_caustics::config::render::RenderSetting {
                                width: (renderer).settings().width,
                                height: (renderer).settings().height,
                                camera_spp: 8,
                                seed: c.render.seed,
                                ..Default::default()
                            },
                            Some(&lighting),
                        )
                        .unwrap();
                        same(&rgb.direct, &actual.direct);
                        same(&rgb.caustics, &actual.caustics);
                        same(&rgb.combined, &actual.combined);
                    }
                }
            }
        }
    }
}

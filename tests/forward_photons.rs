use nalgebra::Vector3;
use spectral_caustics::caustics::SpectralCausticsMap;
use spectral_caustics::config::Config;
use spectral_caustics::config::RenderJob;
use spectral_caustics::config::render::RenderSetting;
use spectral_caustics::config::scene::EnvironmentMisHeuristic;
use spectral_caustics::film::Framebuffer;
use spectral_caustics::optics::dispersion::IorMode;
use spectral_caustics::renderer::Renderer;
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

use nalgebra::Vector3;
use spectral_caustics::{
    config::{
        Config,
        rendersetting::{PhotonTraceMode, RenderSetting},
        sceneconfig::EnvironmentMisHeuristic,
    },
    optics::{
        emission::d65_normalization_continuous,
        iormode::IorMode,
        spectrum::{cie_xyz, d65_normalization, d65_spd},
    },
    render::{
        Render, SpectralCausticsMap, caustics::render_wavelength_caustics, framebuffer::Framebuffer,
    },
    scene::Scene,
};

fn config(source: &str) -> Config {
    let mut c = Config::from_json(source).unwrap();
    c.render.width = 16;
    c.render.height = 12;
    c.render.samples_per_pixel_axis = 2;
    c.render.camera_spp = 8;
    c.render.wavelength_step_nm = 100.0;
    c.render.photon_trace_mode = PhotonTraceMode::Forward;
    c.render.photon_max_events = 16;
    c.render.thread_count = Some(1);
    if let Some(e) = &mut c.scene.environment {
        e.map_path = None;
    }
    c
}

fn same(a: &Framebuffer, b: &Framebuffer) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    for (a, b) in a.pixels.iter().zip(&b.pixels) {
        assert_eq!(a.map(f64::to_bits), b.map(f64::to_bits));
    }
}

fn map(scene: &Scene, settings: &RenderSetting, threads: usize) -> SpectralCausticsMap {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| {
            render_wavelength_caustics(scene, settings)
                .unwrap()
                .unwrap()
        })
}

#[test]
fn legacy_json_defaults_roundtrip_and_forward_validation_are_explicit() {
    let original =
        Config::from_json(include_str!("../configs/vertical_prism_area_light.json")).unwrap();
    assert_eq!(
        original.render.photon_trace_mode,
        PhotonTraceMode::TwoTransmission
    );
    assert_eq!(original.render.photon_max_events, 32);
    let mut value = serde_json::to_value(&original.render).unwrap();
    value.as_object_mut().unwrap().remove("photon_trace_mode");
    value.as_object_mut().unwrap().remove("photon_max_events");
    assert_eq!(
        serde_json::from_value::<RenderSetting>(value.clone()).unwrap(),
        original.render
    );
    value["photon_trace_mode"] = serde_json::json!("forward");
    value["photon_max_events"] = serde_json::json!(16);
    let settings: RenderSetting = serde_json::from_value(value.clone()).unwrap();
    settings.validate().unwrap();
    assert_eq!(
        serde_json::from_str::<RenderSetting>(&serde_json::to_string(&settings).unwrap()).unwrap(),
        settings
    );
    value["photon_trace_mode"] = serde_json::json!("split");
    assert!(serde_json::from_value::<RenderSetting>(value).is_err());
    for limit in [0, 1025, usize::MAX] {
        assert!(
            RenderSetting {
                photon_max_events: limit,
                ..settings.clone()
            }
            .validate()
            .is_err()
        );
    }
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
            let scene = Scene::from_config(&c.scene).unwrap();
            let lighting = Render::new(c.render.clone())
                .unwrap()
                .render_lighting(&scene)
                .unwrap();
            let atlas = map(&scene, &c.render, 1);
            let mut integral = vec![Vector3::zeros(); lighting.caustics.pixels.len()];
            for (j, &(nm, width)) in atlas.wavelength_samples().iter().enumerate() {
                for (xyz, &e) in integral.iter_mut().zip(atlas.layer_irradiance(j).unwrap()) {
                    *xyz += cie_xyz(nm).unwrap() * (e * width);
                }
            }
            for (a, b) in integral.iter().zip(&lighting.caustics.pixels) {
                assert!((a - b).norm() < 2e-12 * (1.0 + b.norm()));
            }
            let area = scene.receiver.area() / integral.len() as f64;
            let received = lighting
                .combined()
                .unwrap()
                .pixels
                .iter()
                .map(|p| p.y * area)
                .sum::<f64>();
            let samples = c.render.wavelength_samples().unwrap();
            let k = if scene.light.as_disk().is_some() {
                d65_normalization_continuous()
            } else {
                d65_normalization(&samples).unwrap()
            };
            let emitted = scene.light.total_flux()
                * samples
                    .iter()
                    .map(|&(nm, w)| k * d65_spd(nm).unwrap() * cie_xyz(nm).unwrap().y * w)
                    .sum::<f64>();
            assert!(received > 0.0 && received <= emitted * (1.0 + 1e-12));
            assert!(lighting.caustics.pixels.iter().any(|p| p.y > 0.0));
            for threads in [1, 2, 4] {
                c.render.thread_count = Some(threads);
                let actual = Render::new(c.render.clone())
                    .unwrap()
                    .render_lighting(&scene)
                    .unwrap();
                same(&lighting.direct, &actual.direct);
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
            c.render.photon_trace_mode = PhotonTraceMode::TwoTransmission;
            let legacy = Render::new(c.render.clone())
                .unwrap()
                .render_lighting(&scene)
                .unwrap();
            same(&lighting.direct, &legacy.direct); // 发射样本与 Direct 不受事件采样影响。
            assert_ne!(lighting.caustics.pixels, legacy.caustics.pixels);
            // Eλ 不依赖采样块切分；同时检查事件 stream 绑定全局样本编号。
            c.render.photon_trace_mode = PhotonTraceMode::Forward;
            c.render.max_spectral_blocks = 3;
            let regrouped = map(&scene, &c.render, 4);
            for j in 0..atlas.wavelength_samples().len() {
                assert_eq!(atlas.layer_irradiance(j), regrouped.layer_irradiance(j));
            }
        }
    }
}

#[test]
fn camera_direct_components_packet_and_rng_remain_compatible_with_both_photon_modes() {
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
                let scene = Scene::from_config(&c.scene).unwrap();
                for packet in [1, 4] {
                    c.render.spectral_packet_size = packet;
                    c.render.photon_trace_mode = PhotonTraceMode::Forward;
                    c.render.thread_count = Some(1);
                    let renderer = Render::new(c.render.clone()).unwrap();
                    let reference = renderer.render_spectral_camera_components(&scene).unwrap();
                    let light = renderer.render_lighting(&scene).unwrap();
                    let rgb = renderer
                        .render_camera_glass_components(&scene, &light, 8, c.render.seed)
                        .unwrap();
                    for images in [&reference, &rgb] {
                        for ((d, a), combined) in images
                            .direct
                            .pixels
                            .iter()
                            .zip(&images.caustics.pixels)
                            .zip(&images.combined.pixels)
                        {
                            // RGB 相机在线性 RGB 中相加再转 XYZ，允许转换的舍入差。
                            assert!((*d + *a - *combined).norm() < 2e-12 * (1.0 + combined.norm()));
                        }
                    }
                    for threads in [2, 4] {
                        c.render.thread_count = Some(threads);
                        let renderer = Render::new(c.render.clone()).unwrap();
                        let actual = renderer.render_spectral_camera_components(&scene).unwrap();
                        same(&reference.direct, &actual.direct);
                        same(&reference.caustics, &actual.caustics);
                        same(&reference.combined, &actual.combined);
                        let lighting = renderer.render_lighting(&scene).unwrap();
                        let actual = renderer
                            .render_camera_glass_components(&scene, &lighting, 8, c.render.seed)
                            .unwrap();
                        same(&rgb.direct, &actual.direct);
                        same(&rgb.caustics, &actual.caustics);
                        same(&rgb.combined, &actual.combined);
                    }
                    c.render.photon_trace_mode = PhotonTraceMode::TwoTransmission;
                    let renderer = Render::new(c.render.clone()).unwrap();
                    let legacy = renderer.render_spectral_camera_components(&scene).unwrap();
                    same(&reference.direct, &legacy.direct);
                    let light = renderer.render_lighting(&scene).unwrap();
                    let legacy = renderer
                        .render_camera_glass_components(&scene, &light, 8, c.render.seed)
                        .unwrap();
                    same(&rgb.direct, &legacy.direct);
                }
            }
        }
    }
}

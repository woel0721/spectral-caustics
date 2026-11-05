use spectral_caustics::config::Config;

#[cfg(not(feature = "diagnostics"))]
#[test]
fn ordinary_build_rejects_diagnostic_configuration() {
    let mut json = serde_json::to_value(Config::default()).unwrap();
    json["render"]["diagnostics"] = serde_json::json!({"path":true});
    assert!(Config::from_json(&json.to_string()).is_err());
}

#[cfg(feature = "diagnostics")]
#[test]
fn spectral_observer_counts_every_lane_without_changing_transport_or_thread_results() {
    use spectral_caustics::{config::render::CameraMode, renderer::Renderer};
    let mut config = Config::default();
    config.render.width = 12;
    config.render.height = 8;
    config.render.camera_spp = 8;
    config.render.samples_per_pixel_axis = 1;
    config.render.camera_mode = CameraMode::Spectral;
    for packet in [1, 4] {
        config.render.spectral_packet_size = packet;
        config.render.diagnostics.path = false;
        let job = config.build().unwrap();
        let ordinary = Renderer::new(config.render.clone())
            .unwrap()
            .render(&job)
            .unwrap();
        assert!(ordinary.path_debug.is_none());
        config.render.diagnostics.path = true;
        let mut reference = None;
        for threads in [1, 2, 4] {
            config.render.thread_count = Some(threads);
            let job = config.build().unwrap();
            let images = Renderer::new(config.render.clone())
                .unwrap()
                .render(&job)
                .unwrap();
            for (a, b) in [
                (&ordinary.direct, &images.direct),
                (&ordinary.caustics, &images.caustics),
                (&ordinary.combined, &images.combined),
            ] {
                assert_eq!(a.pixels(), b.pixels());
            }
            let stats = images.path_debug.unwrap();
            assert!(stats.pixels.iter().all(|p| p.samples == 8));
            assert!(
                stats
                    .pixels
                    .iter()
                    .all(|p| p.terminations.values().sum::<u64>() == p.target_samples)
            );
            if let Some(previous) = &reference {
                assert_eq!(&stats, previous);
            }
            reference = Some(stats);
        }
    }
}

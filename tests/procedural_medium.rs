use nalgebra::Vector3;
use spectral_caustics::{
    config::{Config, render::CameraMode},
    geometry::ray::Ray,
    medium::MediumConfig,
    renderer::Renderer,
};

#[test]
fn configured_fog_has_local_support_and_an_independent_shape_seed() {
    let mut config = Config::load("configs/local_procedural_fog.json").unwrap();
    config.validate().unwrap();
    assert_eq!(
        Config::from_json(&config.to_json().unwrap()).unwrap(),
        config
    );

    let job = config.build().unwrap();
    let fog = job.scene.medium.unwrap();
    let core = Vector3::new(0.0, 0.0, 0.8);
    let core_density = fog.density_at(core);
    assert!(core_density > 0.0);
    assert!(fog.density_at(Vector3::new(0.79, 0.0, 0.8)) < 0.05 * core_density);
    assert_eq!(fog.density_at(Vector3::new(0.79, 0.49, 1.39)), 0.0);
    let through = Ray::new(Vector3::new(0.0, -2.0, 0.8), Vector3::y()).unwrap();
    let outside = Ray::new(Vector3::new(0.81, -2.0, 0.8), Vector3::y()).unwrap();
    assert!((0.0..1.0).contains(&fog.transmittance_ray(&through, 4.0)));
    assert_eq!(fog.transmittance_ray(&outside, 4.0), 1.0);

    config.render.seed += 1;
    let repeat = config.build().unwrap().scene.medium.unwrap();
    assert_eq!(repeat.density_at(core).to_bits(), core_density.to_bits());
    let Some(MediumConfig::Procedural(procedural)) = &mut config.scene.medium else {
        panic!("expected procedural medium");
    };
    procedural.procedural.seed += 1;
    let different = config.build().unwrap().scene.medium.unwrap();
    assert_ne!(different.density_at(core), core_density);
}

#[test]
fn procedural_medium_rejects_conflicting_or_invalid_grid_inputs() {
    let config = Config::load("configs/local_procedural_fog.json").unwrap();
    let mut json = serde_json::to_value(&config).unwrap();
    json["scene"]["medium"]["densities"] = serde_json::json!([1.0]);
    assert!(Config::from_json(&json.to_string()).is_err());

    for resolution in [[0, 1, 1], [usize::MAX, 2, 1], [usize::MAX, 1, 1]] {
        let mut invalid = config.clone();
        let Some(MediumConfig::Procedural(procedural)) = &mut invalid.scene.medium else {
            panic!("expected procedural medium");
        };
        procedural.resolution = resolution;
        assert!(invalid.build().is_err());
    }
}

#[test]
fn disk_lights_local_fog_and_parallel_renders_agree() {
    let mut config = Config::load("configs/local_procedural_fog.json").unwrap();
    config.render.width = 24;
    config.render.height = 16;
    config.render.camera_spp = 32;
    config.scene.light_samples = 1;
    let render = |c: &Config| {
        Renderer::new(c.render.clone())
            .unwrap()
            .render(&c.build().unwrap())
            .unwrap()
    };

    for (mode, packet) in [
        (CameraMode::Rgb, 1),
        (CameraMode::Spectral, 1),
        (CameraMode::Spectral, 4),
    ] {
        config.render.camera_mode = mode;
        config.render.spectral_packet_size = packet;
        config.render.thread_count = Some(1);
        let reference = render(&config);
        for threads in [2, 4] {
            config.render.thread_count = Some(threads);
            let parallel = render(&config);
            for (actual, expected) in [
                (&parallel.direct, &reference.direct),
                (&parallel.caustics, &reference.caustics),
                (&parallel.combined, &reference.combined),
            ] {
                for (a, e) in actual.pixels().iter().zip(expected.pixels()) {
                    assert_eq!(a.map(f64::to_bits), e.map(f64::to_bits));
                }
            }
        }

        let mut control = config.clone();
        control.scene.max_volume_scatters = 0;
        let extinction = render(&control);
        control.scene.medium = None;
        let vacuum = render(&control);
        assert!(
            extinction
                .combined
                .pixels()
                .iter()
                .zip(vacuum.combined.pixels())
                .any(|(e, v)| e.y < v.y - 1e-6)
        );
        assert!(
            reference
                .combined
                .pixels()
                .iter()
                .zip(extinction.combined.pixels())
                .any(|(scattering, e)| scattering.y > e.y + 1e-6)
        );

        let mut dark = config.clone();
        let spectral_caustics::config::light::LightConfig::DiskArea(light) = &mut dark.scene.light
        else {
            panic!("expected disk area light");
        };
        light.total_flux = 0.0;
        assert!(
            render(&dark)
                .combined
                .pixels()
                .iter()
                .all(|v| *v == Vector3::zeros())
        );
    }
}

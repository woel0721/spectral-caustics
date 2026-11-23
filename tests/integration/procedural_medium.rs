use crate::support::assert_camera_images_bits_eq;
use nalgebra::Vector3;
use serde_json::json;
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
    assert!((0.0..1.0).contains(&fog.transmittance_ray(&through, 4.0, 550.0)));
    assert_eq!(fog.transmittance_ray(&outside, 4.0, 550.0), 1.0);

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
    let mut base = Config::load("configs/local_procedural_fog.json").unwrap();
    base.render.width = 12;
    base.render.height = 8;
    base.render.camera_spp = 16;
    base.scene.light_samples = 1;
    let Some(MediumConfig::Procedural(base_procedural)) = &mut base.scene.medium else {
        panic!("expected a procedural grid");
    };
    base_procedural.resolution = [16, 12, 12];
    let mut spectral_json = serde_json::to_value(&base).unwrap();
    spectral_json["render"]["camera_mode"] = json!("spectral");
    spectral_json["scene"]["medium"]["sigma_a_base"] = json!({"samples": [[380, 0.1], [780, 0.3]]});
    spectral_json["scene"]["medium"]["sigma_s_base"] =
        json!({"samples": [[380, 2.5], [550, 1.2], [780, 0.0]]});
    let spectral = Config::from_json(&spectral_json.to_string()).unwrap();
    assert_eq!(
        Config::from_json(&spectral.to_json().unwrap()).unwrap(),
        spectral
    );

    let Some(MediumConfig::Procedural(c)) = &spectral.scene.medium else {
        panic!("expected a procedural grid");
    };
    let mut base_built = base.build().unwrap();
    let mut spectral_built = spectral.build().unwrap();
    let fog = spectral_built.scene.medium.as_ref().unwrap();
    let dy = (c.max_m[1] - c.min_m[1]) / c.resolution[1] as f64;
    let column_density = (0..c.resolution[1])
        .map(|i| fog.density_at(Vector3::new(0.0, c.min_m[1] + (i as f64 + 0.5) * dy, 0.81)) * dy)
        .sum::<f64>();
    assert!(column_density > 0.0);
    let through = Ray::new(Vector3::new(0.0, -2.0, 0.81), Vector3::y()).unwrap();
    for (nm, sigma_t) in [(380.0, 2.6), (550.0, 1.385), (780.0, 0.3)] {
        let actual = fog.transmittance_ray(&through, 4.0, nm);
        assert!((actual - (-sigma_t * column_density).exp()).abs() < 1e-13);
    }

    let mut base_dark = base.clone();
    let spectral_caustics::config::light::LightConfig::DiskArea(light) = &mut base_dark.scene.light
    else {
        panic!("expected disk area light");
    };
    light.total_flux = 0.0;
    let base_dark_built = base_dark.build().unwrap();
    let mut spectral_dark = spectral.clone();
    let spectral_caustics::config::light::LightConfig::DiskArea(light) =
        &mut spectral_dark.scene.light
    else {
        panic!("expected disk area light");
    };
    light.total_flux = 0.0;
    let spectral_dark_built = spectral_dark.build().unwrap();

    for (use_spectral, mode, packet) in [
        (false, CameraMode::Rgb, 1),
        (false, CameraMode::Spectral, 1),
        (false, CameraMode::Spectral, 4),
        (true, CameraMode::Spectral, 1),
        (true, CameraMode::Spectral, 4),
    ] {
        let mut render_setting = if use_spectral {
            spectral.render.clone()
        } else {
            base.render.clone()
        };
        let built = if use_spectral {
            &mut spectral_built
        } else {
            &mut base_built
        };
        let dark_built = if use_spectral {
            &spectral_dark_built
        } else {
            &base_dark_built
        };
        render_setting.camera_mode = mode;
        render_setting.spectral_packet_size = packet;
        render_setting.thread_count = Some(1);
        let reference = Renderer::new(render_setting.clone())
            .unwrap()
            .render(built)
            .unwrap();
        for (d, (c, combined)) in reference.direct.pixels().iter().zip(
            reference
                .caustics
                .pixels()
                .iter()
                .zip(reference.combined.pixels()),
        ) {
            assert_eq!((d + c).map(f64::to_bits), combined.map(f64::to_bits));
        }
        for threads in [2, 4] {
            render_setting.thread_count = Some(threads);
            let parallel = Renderer::new(render_setting.clone())
                .unwrap()
                .render(built)
                .unwrap();
            assert_camera_images_bits_eq(&parallel, &reference);
        }

        let original_scatters = built.camera_options.max_volume_scatters;
        built.camera_options.max_volume_scatters = 0;
        let extinction = Renderer::new(render_setting.clone())
            .unwrap()
            .render(built)
            .unwrap();
        let medium = built.scene.medium.take();
        let vacuum = Renderer::new(render_setting.clone())
            .unwrap()
            .render(built)
            .unwrap();
        built.scene.medium = medium;
        built.camera_options.max_volume_scatters = original_scatters;
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

        assert!(
            Renderer::new(render_setting)
                .unwrap()
                .render(dark_built)
                .unwrap()
                .combined
                .pixels()
                .iter()
                .all(|v| *v == Vector3::zeros())
        );
    }
}

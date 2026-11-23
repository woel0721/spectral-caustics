use crate::support::{assert_camera_images_bits_eq, assert_framebuffer_bits_eq};
use nalgebra::Vector3;
use serde_json::json;
use spectral_caustics::config::Config;
use spectral_caustics::config::render::CameraMode;
use spectral_caustics::config::scene::EnvironmentMisHeuristic;
use spectral_caustics::config::scene::ObjectConfig;
use spectral_caustics::geometry::ray::Ray;
use spectral_caustics::optics::dispersion::Cauchy;
use spectral_caustics::optics::dispersion::IorMode;
use spectral_caustics::optics::interface::fresnel_dielectric;
use spectral_caustics::optics::interface::refract;
use spectral_caustics::renderer::Renderer;

fn config(caustics: bool) -> Config {
    if caustics {
        let mut c = Config::from_json(include_str!(
            "../../configs/vertical_prism_spectral_caustics.json"
        ))
        .unwrap();
        c.render.camera_mode = CameraMode::Rgb;
        c.render.camera_spp = 64;
        c.render.spectral_packet_size = 1;
        c
    } else {
        Config::from_json(include_str!("../../configs/vertical_prism_spectral.json")).unwrap()
    }
}

#[test]
fn vertical_prism_configs_bind_z_geometry_and_validate_explicit_parameters() {
    for caustics in [false, true] {
        let c = config(caustics);
        let scene = c.build().unwrap();
        assert_eq!(scene.photon.caustics_object_id, caustics.then_some(0));
        assert!(scene.ground_checkerboard.is_none());
        assert!(matches!(
            c.scene.objects[0],
            ObjectConfig::VerticalPrism { .. }
        ));
        assert!(matches!(c.scene.objects[1], ObjectConfig::Ground { .. }));
        let json = c.to_json().unwrap();
        assert_eq!(Config::from_json(&json).unwrap(), c);
        let hit = scene
            .scene
            .intersect(&Ray::new(Vector3::new(0.0, 0.0, 3.0), -Vector3::z()).unwrap())
            .unwrap();
        assert_eq!(hit.object_id, 0);
        assert!((hit.coords.z - 1.85).abs() < 1e-12);
        assert_eq!(hit.normal, Vector3::z());
    }
    let base = serde_json::to_value(config(true)).unwrap();
    for (field, value) in [
        ("z_max_m", json!(0.05)),
        ("z_min_m", json!(2.0)),
        ("cross_section_m", json!([[0, 0], [1, 1], [2, 2]])),
        ("bevel_size_m", json!(0.1)),
        (
            "material",
            json!({"type":"lambertian", "albedo":[0.18,0.18,0.18]}),
        ),
    ] {
        let mut invalid = base.clone();
        invalid["scene"]["objects"][0][field] = value;
        assert!(
            Config::from_json(&invalid.to_string())
                .and_then(|config| {
                    config.validate()?;
                    Ok(config)
                })
                .is_err(),
            "{field}"
        );
    }
    let mut value = base;
    value["scene"]["objects"][0]
        .as_object_mut()
        .unwrap()
        .remove("caustics");
    value["scene"]["objects"][0]
        .as_object_mut()
        .unwrap()
        .remove("material");
    let defaulted = Config::from_json(&value.to_string()).unwrap();
    assert_eq!(defaulted.build().unwrap().photon.caustics_object_id, None);
    let mut duplicate = config(true);
    duplicate
        .scene
        .objects
        .push(duplicate.scene.objects[0].clone());
    assert!(duplicate.validate().unwrap_err().contains("一个焦散物体"));
}

#[test]
fn striped_through_prism_spectral_render_repeats_for_threads_and_mis() {
    let mut c = config(false);
    c.render.width = 12;
    c.render.height = 12;
    c.render.camera_spp = 8;
    let mut balance_modes = Vec::new();
    for (heuristic, mode) in [
        (EnvironmentMisHeuristic::Balance, IorMode::Constant),
        (EnvironmentMisHeuristic::Balance, IorMode::Dispersion),
        (EnvironmentMisHeuristic::Power, IorMode::Dispersion),
    ] {
        c.scene.environment_mis_heuristic = heuristic;
        c.render.mode = mode;
        let scene = c.build().unwrap();
        let mut reference = None;
        for threads in [1, 2, 4] {
            c.render.thread_count = Some(threads);
            let image = Renderer::new(c.render.clone())
                .unwrap()
                .render(&scene)
                .map(|images| images.combined)
                .unwrap();
            assert!(
                image
                    .pixels()
                    .iter()
                    .all(|p| p.iter().all(|v| v.is_finite()))
            );
            assert!(image.pixels().iter().any(|p| p.y > 0.1));
            if let Some(previous) = &reference {
                assert_framebuffer_bits_eq(&image, previous);
            }
            reference = Some(image);
        }
        if heuristic == EnvironmentMisHeuristic::Balance {
            balance_modes.push(reference.unwrap());
        }
    }
    assert!(
        balance_modes[0]
            .pixels()
            .iter()
            .zip(balance_modes[1].pixels())
            .any(|(a, b)| (a - b).norm() > 1e-6)
    );
}

#[test]
fn vertical_prism_ground_caustics_are_additive_colored_and_thread_deterministic() {
    let mut c = config(true);
    c.render.width = 16;
    c.render.height = 16;
    c.render.camera_spp = 4;
    c.render.samples_per_pixel_axis = 1;
    c.render.wavelength_step_nm = 20.0;
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        let scene = c.build().unwrap();
        let mut reference = None;
        for threads in [1, 2, 4] {
            c.render.thread_count = Some(threads);
            let (lighting, images) = crate::support::run_in_threads(threads, || {
                let lighting = spectral_caustics::integrator::photon::integrate_xyz(
                    &scene,
                    scene.photon.caustics_object_id,
                    scene.photon.ground_id.expect("receiver"),
                    &c.render,
                )
                .unwrap();
                let images = spectral_caustics::integrator::camera::integrate_rgb(
                    &scene,
                    &c.render,
                    Some(&lighting),
                )
                .unwrap();
                (lighting, images)
            });
            let pixel_area =
                scene.photon.receiver.area() / (c.render.width * c.render.height) as f64;
            let flux: f64 = lighting
                .caustics
                .pixels()
                .iter()
                .map(|p| p.y * pixel_area)
                .sum();
            assert!(flux > 0.0 && flux < c.scene.light.as_parallel().unwrap().total_flux);
            let white = lighting.sun_irradiance_xyz / lighting.sun_irradiance_xyz.y;
            let chroma = lighting
                .caustics
                .pixels()
                .iter()
                .filter(|p| p.y > 1e-6)
                .map(|p| (p / p.y - white).norm())
                .fold(0.0_f64, f64::max);
            if mode == IorMode::Constant {
                assert!(chroma < 1e-12);
            } else {
                assert!(chroma > 0.1);
            }
            assert!(images.caustics.pixels().iter().any(|p| p.y > 0.0));
            for ((d, k), sum) in images
                .direct
                .pixels()
                .iter()
                .zip(images.caustics.pixels())
                .zip(images.combined.pixels())
            {
                assert!((d + k - sum).norm() < 1e-12 * (1.0 + sum.norm()));
            }
            if let Some((light, previous)) = &reference {
                let light: &spectral_caustics::caustics::SpectralLighting = light;
                let previous: &spectral_caustics::integrator::camera::CameraLightingImages =
                    previous;
                assert_framebuffer_bits_eq(&lighting.caustics, &light.caustics);
                assert_camera_images_bits_eq(&images, previous);
            }
            reference = Some((lighting, images));
        }
    }
}

#[test]
fn vertical_sides_refract_the_same_ray_to_separate_spectral_ground_positions() {
    let c = config(true);
    let scene = c.build().unwrap();
    let light = &c.scene.light;
    let ray = Ray::new(
        Vector3::new(
            (light.as_parallel().unwrap().aperture_min_m[0]
                + light.as_parallel().unwrap().aperture_max_m[0])
                * 0.5,
            (light.as_parallel().unwrap().aperture_min_m[1]
                + light.as_parallel().unwrap().aperture_max_m[1])
                * 0.5,
            light.as_parallel().unwrap().height_m,
        ),
        light.as_parallel().unwrap().direction.into(),
    )
    .unwrap();
    let entry = scene.scene.intersect(&ray).unwrap();
    assert_eq!(entry.normal, -Vector3::y());
    let model = Cauchy {
        a: 1.4907,
        b_um2: 0.0084,
    };
    let mut landings = Vec::new();
    for nm in [450.0, 550.0, 650.0] {
        let n = model.ior(nm).unwrap();
        let inside = Ray::new(
            entry.coords,
            refract(ray.direction, entry.normal, 1.0, n).unwrap(),
        )
        .unwrap();
        let exit = scene.scene.intersect(&inside).unwrap();
        assert_eq!(exit.object_id, 0);
        assert!((exit.normal - Vector3::new(-3.0_f64.sqrt() / 2.0, 0.5, 0.0)).norm() < 1e-12);
        let outside = Ray::new(
            exit.coords,
            refract(inside.direction, -exit.normal, n, 1.0).unwrap(),
        )
        .unwrap();
        let ground = scene.scene.intersect(&outside).unwrap();
        assert_eq!(Some(ground.object_id), scene.photon.ground_id);
        let landing = spectral_caustics::integrator::photon::trace_photon(
            &scene,
            ray,
            Some(nm),
            IorMode::Dispersion,
            32,
            &mut crate::support::TransmissionRng,
        )
        .unwrap()
        .landing()
        .unwrap();
        let f1 = fresnel_dielectric(-ray.direction.dot(&entry.normal), 1.0, n).unwrap();
        let f2 = fresnel_dielectric(inside.direction.dot(&exit.normal), n, 1.0).unwrap();
        assert!((landing.position - ground.coords).norm() < 1e-12);
        assert!(landing.weight == 1.0 && (0.0..=1.0).contains(&f1) && (0.0..=1.0).contains(&f2));
        assert!((0.0..=1.0).contains(&landing.weight));
        landings.push(landing.position);
        assert_eq!(
            spectral_caustics::integrator::photon::trace_photon(
                &scene,
                ray,
                Some(nm),
                IorMode::Constant,
                32,
                &mut crate::support::TransmissionRng
            )
            .unwrap()
            .landing(),
            spectral_caustics::integrator::photon::trace_photon(
                &scene,
                ray,
                Some(550.0),
                IorMode::Constant,
                32,
                &mut crate::support::TransmissionRng
            )
            .unwrap()
            .landing()
        );
    }
    assert!(landings[0].x > landings[1].x && landings[1].x > landings[2].x);
    assert!((landings[0] - landings[2]).norm() > 0.1);
}

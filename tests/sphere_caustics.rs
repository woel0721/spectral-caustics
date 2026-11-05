use spectral_caustics::geometry::ray::Ray;
use spectral_caustics::optics::interface::fresnel_dielectric;
use spectral_caustics::optics::interface::refract;
mod support;
use nalgebra::Vector3;
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use serde_json::json;
use spectral_caustics::config::Config;
use spectral_caustics::config::material::MaterialConfig;
use spectral_caustics::config::scene::EnvironmentMisHeuristic;
use spectral_caustics::config::scene::ObjectConfig;
use spectral_caustics::film::Framebuffer;
use spectral_caustics::material::Material;
use spectral_caustics::material::dielectric::Dielectric;
use spectral_caustics::optics::dispersion::Cauchy;
use spectral_caustics::optics::dispersion::IorMode;
use spectral_caustics::renderer::Renderer;
use spectral_caustics::spectrum::d65_normalization;
use spectral_caustics::spectrum::d65_spd;
use spectral_caustics::spectrum::wavelength_to_xyz;

fn config() -> Config {
    let mut config = Config::from_json(include_str!("../configs/sphere.json")).unwrap();
    config.render.width = 32;
    config.render.height = 24;
    config.render.samples_per_pixel_axis = 2;
    config.render.camera_spp = 4;
    config.render.thread_count = Some(1);
    config.scene.environment.as_mut().unwrap().map_path = None;
    config.scene.environment_samples = 3;
    config
}

fn set_glass(config: &mut Config, ior: f64, dispersion: Option<Cauchy>) {
    let ObjectConfig::Sphere { material, .. } = &mut config.scene.objects[0] else {
        panic!()
    };
    *material = MaterialConfig::Dielectric { ior, dispersion };
}

fn identical(a: &Framebuffer, b: &Framebuffer) {
    assert_eq!((a.width(), a.height()), (b.width(), b.height()));
    for (a, b) in a.pixels().iter().zip(b.pixels()) {
        for (a, b) in a.iter().zip(b.iter()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }
}

#[test]
fn material_owns_constant_and_cauchy_indices_without_changing_rgb_sampling() {
    let model = Cauchy {
        a: 1.48,
        b_um2: 0.008,
    };
    let constant = Dielectric::new(1.33).unwrap();
    let dispersive = Dielectric::with_dispersion(1.33, Some(model)).unwrap();
    for nm in [400.0, 550.0, 700.0] {
        assert_eq!(
            constant.photon_ior(Some(nm), IorMode::Dispersion),
            Some(1.33)
        );
        assert_eq!(
            dispersive.photon_ior(Some(nm), IorMode::Constant),
            Some(1.33)
        );
        assert_eq!(
            dispersive.photon_ior(Some(nm), IorMode::Dispersion),
            Some(model.a + model.b_um2 / (nm / 1000.0).powi(2))
        );
    }
    assert!(dispersive.photon_ior(None, IorMode::Dispersion).is_none());
    for nm in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e-200] {
        assert!(
            dispersive
                .photon_ior(Some(nm), IorMode::Dispersion)
                .is_none()
        );
    }
    let mut a = ChaCha12Rng::seed_from_u64(42);
    let mut b = a.clone();
    for wo in [Vector3::new(0.4, 0.0, 1.0), Vector3::new(0.2, 0.0, -1.0)] {
        for _ in 0..128 {
            assert_eq!(
                constant.sample(wo, Vector3::z(), &mut a),
                dispersive.sample(wo, Vector3::z(), &mut b)
            );
        }
    }
}

#[cfg(feature = "diagnostics")]
#[test]
fn selection_follows_object_flags_and_rejects_ambiguous_or_non_dielectric_targets() {
    let mut c = config();
    #[cfg(feature = "diagnostics")]
    {
        c.render.diagnostics.target = Some(1);
    }
    for reverse in [false, true] {
        if reverse {
            c.scene.objects.reverse();
        }
        let scene = c.build().unwrap();
        assert_eq!(scene.photon.caustics_object_id, Some(usize::from(reverse)));
        assert_eq!(scene.photon.ground_id, Some(usize::from(!reverse)));
        assert_eq!(scene.diagnostics.target, Some(1));
    }
    let c = config();
    let mut value = serde_json::to_value(&c).unwrap();
    value["scene"]["objects"][0]["material"] = json!({"type":"mirror","reflectance":[1.0,1.0,1.0]});
    assert!(
        Config::from_json(&value.to_string())
            .and_then(|config| {
                config.validate()?;
                Ok(config)
            })
            .unwrap_err()
            .to_string()
            .contains("Dielectric")
    );
    let mut ambiguous = c.clone();
    ambiguous.scene.objects.push(c.scene.objects[0].clone());
    assert!(ambiguous.validate().unwrap_err().contains("一个焦散物体"));
    let mut no_target = c.clone();
    if let ObjectConfig::Sphere { caustics, .. } = &mut no_target.scene.objects[0] {
        *caustics = false;
    }
    let scene = no_target.build().unwrap();
    assert_eq!(scene.photon.caustics_object_id, None);
    assert!(
        spectral_caustics::integrator::photon::integrate_xyz(
            &scene,
            scene.photon.caustics_object_id,
            scene.photon.ground_id.expect("receiver"),
            (Renderer::new(no_target.render).unwrap()).settings()
        )
        .unwrap()
        .caustics
        .pixels()
        .iter()
        .all(|p| p.norm() == 0.0)
    );
    let prism = Config::default();
    let scene = prism.build().unwrap();
    assert_eq!(
        scene.photon.caustics_object_id,
        scene.photon.caustics_object_id
    );
}

#[test]
fn invalid_dispersion_parameters_and_unknown_fields_fail_configuration_validation() {
    let c = config();
    for model in [
        json!({"a":0.0,"b_um2":0.0042}),
        json!({"a":1.5,"b_um2":-0.01}),
        json!({"a":1.5,"b_um2":1e308}),
        json!({"a":1.5,"b_um2":0.0042,"b":1.0}),
    ] {
        let mut value = serde_json::to_value(&c).unwrap();
        value["scene"]["objects"][0]["material"]["dispersion"] = model;
        assert!(
            Config::from_json(&value.to_string())
                .and_then(|config| {
                    config.validate()?;
                    Ok(config)
                })
                .is_err()
        );
    }
    for model in [
        Cauchy {
            a: f64::NAN,
            b_um2: 0.0,
        },
        Cauchy {
            a: 1.5,
            b_um2: f64::INFINITY,
        },
    ] {
        assert!(Dielectric::with_dispersion(1.52, Some(model)).is_err());
    }
    assert_eq!(Config::from_json(&c.to_json().unwrap()).unwrap(), c);
}

#[test]
fn spectral_sphere_matches_independent_constant_ior_renders_integrated_in_xyz() {
    let mut c = config();
    c.render.width = 12;
    c.render.height = 10;
    c.render.wavelength_step_nm = 40.0;
    let scene = c.build().unwrap();
    let actual = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (Renderer::new(c.render.clone()).unwrap()).settings(),
    )
    .unwrap();
    let mut expected = vec![Vector3::zeros(); c.render.width * c.render.height];
    let white_y = actual.sun_irradiance_xyz.y / scene.scene.light.sunlight().unwrap().1;
    let samples = c.render.wavelength_samples().unwrap();
    let k = d65_normalization(&samples).unwrap();
    for (nm, delta_nm) in samples {
        set_glass(&mut c, Cauchy::default().ior(nm).unwrap(), None);
        let mono = spectral_caustics::integrator::photon::integrate_xyz(
            &c.build().unwrap(),
            c.build().unwrap().photon.caustics_object_id,
            c.build().unwrap().photon.ground_id.expect("receiver"),
            (Renderer::new(c.render.clone()).unwrap()).settings(),
        )
        .unwrap();
        let xyz = wavelength_to_xyz(nm, d65_spd(nm).unwrap(), delta_nm, k).unwrap();
        for (expected, mono) in expected.iter_mut().zip(mono.caustics.pixels()) {
            *expected += xyz * (mono.y / white_y);
        }
        identical(
            actual.direct.as_ref().unwrap(),
            mono.direct.as_ref().unwrap(),
        );
    }
    assert!(actual.caustics.pixels().iter().any(|p| p.y > 0.0));
    for (actual, expected) in actual.caustics.pixels().iter().zip(expected) {
        assert!((actual - expected).norm() < 1e-11 * (1.0 + expected.norm()));
    }
}

#[test]
fn photon_flux_is_normalized_once_across_spatial_and_wavelength_sample_counts() {
    let mut c = config();
    c.render.width = 20;
    c.render.height = 20;
    c.scene.light.as_parallel_mut().unwrap().direction = [0.0, 0.0, -1.0];
    c.scene.light.as_parallel_mut().unwrap().aperture_min_m = [-1.0; 2];
    c.scene.light.as_parallel_mut().unwrap().aperture_max_m = [1.0; 2];
    c.scene.receiver.min_m = [-4.0; 2];
    c.scene.receiver.max_m = [4.0; 2];
    set_glass(&mut c, 1.0, None);
    for samples in [1, 2, 4] {
        for step in [10.0, 40.0] {
            c.render.samples_per_pixel_axis = samples;
            c.render.wavelength_step_nm = step;
            let scene = c.build().unwrap();
            let lighting = spectral_caustics::integrator::photon::integrate_xyz(
                &scene,
                scene.photon.caustics_object_id,
                scene.photon.ground_id.expect("receiver"),
                (Renderer::new(c.render.clone()).unwrap()).settings(),
            )
            .unwrap();
            let pixel_area =
                scene.photon.receiver.area() / (c.render.width * c.render.height) as f64;
            let flux: f64 = lighting
                .direct
                .as_ref()
                .unwrap()
                .pixels()
                .iter()
                .chain(lighting.caustics.pixels())
                .map(|p| p.y * pixel_area)
                .sum();
            assert!((flux - c.scene.light.as_parallel().unwrap().total_flux).abs() < 1e-11);
            assert!(lighting.caustics.pixels().iter().any(|p| p.y > 0.0));
        }
    }
    set_glass(&mut c, 1.52, Some(Cauchy::default()));
    let scene = c.build().unwrap();
    let lighting = spectral_caustics::integrator::photon::integrate_xyz(
        &scene,
        scene.photon.caustics_object_id,
        scene.photon.ground_id.expect("receiver"),
        (Renderer::new(c.render).unwrap()).settings(),
    )
    .unwrap();
    let flux: f64 = lighting
        .combined()
        .unwrap()
        .pixels()
        .iter()
        .map(|p| p.y * scene.photon.receiver.area() / 400.0)
        .sum();
    assert!(flux > 0.0 && flux < c.scene.light.as_parallel().unwrap().total_flux);
}

#[test]
fn sphere_components_are_additive_colored_and_repeat_across_threads_mis_and_depths() {
    let mut c = config();
    c.render.width = 24;
    c.render.height = 18;
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        for heuristic in [
            EnvironmentMisHeuristic::Balance,
            EnvironmentMisHeuristic::Power,
        ] {
            c.scene.environment_mis_heuristic = heuristic;
            for depth in [0, 4] {
                c.scene.max_diffuse_bounces = depth;
                let scene = c.build().unwrap();
                let mut reference = None;
                for threads in [1, 2, 4, 1] {
                    c.render.thread_count = Some(threads);
                    let renderer = Renderer::new(c.render.clone()).unwrap();
                    let lighting = spectral_caustics::integrator::photon::integrate_xyz(
                        &scene,
                        scene.photon.caustics_object_id,
                        scene.photon.ground_id.expect("receiver"),
                        (renderer).settings(),
                    )
                    .unwrap();
                    let images = spectral_caustics::integrator::camera::integrate_rgb(
                        &scene,
                        &spectral_caustics::config::render::RenderSetting {
                            width: (renderer).settings().width,
                            height: (renderer).settings().height,
                            camera_spp: 4,
                            seed: 42,
                            ..Default::default()
                        },
                        Some(&lighting),
                    )
                    .unwrap();
                    assert!(images.caustics.pixels().iter().any(|p| p.y > 0.0));
                    for ((direct, caustics), combined) in images
                        .direct
                        .pixels()
                        .iter()
                        .zip(images.caustics.pixels())
                        .zip(images.combined.pixels())
                    {
                        assert!(
                            (direct + caustics - combined).norm() < 1e-12 * (1.0 + combined.norm())
                        );
                    }
                    if let Some((light, previous)) = &reference {
                        let light: &spectral_caustics::caustics::SpectralLighting = light;
                        let previous: &spectral_caustics::integrator::camera::CameraLightingImages =
                            previous;
                        identical(
                            lighting.direct.as_ref().unwrap(),
                            light.direct.as_ref().unwrap(),
                        );
                        identical(&lighting.caustics, &light.caustics);
                        identical(&images.direct, &previous.direct);
                        identical(&images.caustics, &previous.caustics);
                        identical(&images.combined, &previous.combined);
                    }
                    reference = Some((lighting, images));
                }
            }
        }
    }
}

#[test]
fn changing_light_dispersion_preserves_camera_direct_and_rgb_paths() {
    let mut c = config();
    let mut reference = None;
    let mut caustics = Vec::new();
    for dispersion in [
        None,
        Some(Cauchy::default()),
        Some(Cauchy {
            a: 1.48,
            b_um2: 0.008,
        }),
    ] {
        set_glass(&mut c, 1.52, dispersion);
        let scene = c.build().unwrap();
        let renderer = Renderer::new(c.render.clone()).unwrap();
        let lighting = spectral_caustics::integrator::photon::integrate_xyz(
            &scene,
            scene.photon.caustics_object_id,
            scene.photon.ground_id.expect("receiver"),
            (renderer).settings(),
        )
        .unwrap();
        let white = lighting.sun_irradiance_xyz / lighting.sun_irradiance_xyz.y;
        let chroma_difference = lighting
            .caustics
            .pixels()
            .iter()
            .filter(|p| p.y > 1e-5)
            .map(|p| (p / p.y - white).norm())
            .fold(0.0_f64, f64::max);
        if dispersion.is_none() {
            assert!(chroma_difference < 1e-12);
        } else {
            assert!(chroma_difference > 0.01);
        }
        let images = spectral_caustics::integrator::camera::integrate_rgb(
            &scene,
            &spectral_caustics::config::render::RenderSetting {
                width: (renderer).settings().width,
                height: (renderer).settings().height,
                camera_spp: 4,
                seed: 42,
                ..Default::default()
            },
            Some(&lighting),
        )
        .unwrap();
        let rgb = spectral_caustics::integrator::camera::integrate_rgb(
            &scene,
            &spectral_caustics::config::render::RenderSetting {
                width: 16,
                height: 12,
                camera_spp: 4,
                seed: 42,
                ..Default::default()
            },
            None,
        )
        .map(|images| images.combined)
        .unwrap();
        if let Some((direct, previous_rgb)) = &reference {
            identical(&images.direct, direct);
            identical(&rgb, previous_rgb);
        }
        reference = Some((images.direct, rgb));
        caustics.push(images.caustics);
    }
    assert!(
        caustics[0]
            .pixels()
            .iter()
            .zip(caustics[1].pixels())
            .any(|(a, b)| (a - b).norm() > 1e-6)
    );
    assert!(
        caustics[1]
            .pixels()
            .iter()
            .zip(caustics[2].pixels())
            .any(|(a, b)| (a - b).norm() > 1e-6)
    );
}

#[test]
fn sphere_entry_exit_normals_snell_and_flux_match_the_analytic_ball_lens() {
    let mut c = config();
    if let ObjectConfig::Sphere { center_m, .. } = &mut c.scene.objects[0] {
        center_m[2] = 1.3;
    }
    let scene = c.build().unwrap();
    for x in [0.0, 0.4, 0.65] {
        let ray = Ray::new(Vector3::new(x, 0.0, 3.0), -Vector3::z()).unwrap();
        let entry = scene.scene.intersect(&ray).unwrap();
        assert_eq!(Some(entry.object_id), scene.photon.caustics_object_id);
        assert!(entry.normal.dot(&ray.direction) < 0.0);
        assert!((entry.normal.norm() - 1.0).abs() < 1e-12);
        assert!((entry.coords.z - (1.3 + (0.8_f64.powi(2) - x * x).sqrt())).abs() < 1e-12);
        for nm in [400.0, 700.0] {
            let n = Cauchy::default().ior(nm).unwrap();
            let inside = Ray::new(
                entry.coords,
                refract(ray.direction, entry.normal, 1.0, n).unwrap(),
            )
            .unwrap();
            let exit = scene.scene.intersect(&inside).unwrap();
            assert_eq!(exit.object_id, entry.object_id);
            assert_eq!(exit.material_id, entry.material_id);
            assert!(exit.normal.dot(&inside.direction) > 0.0);
            assert!((exit.normal.norm() - 1.0).abs() < 1e-12);
            let outside = Ray::new(
                exit.coords,
                refract(inside.direction, -exit.normal, n, 1.0).unwrap(),
            )
            .unwrap();
            let i = (x / 0.8).asin();
            let r = (i.sin() / n).asin();
            let deviation = 2.0 * (i - r);
            assert!(
                (outside.direction - Vector3::new(-deviation.sin(), 0.0, -deviation.cos())).norm()
                    < 5e-5
            );
            let ground = scene.scene.intersect(&outside).unwrap();
            assert_eq!(Some(ground.object_id), scene.photon.ground_id);
            let landing = spectral_caustics::integrator::photon::trace_photon(
                &scene,
                ray,
                Some(nm),
                IorMode::Dispersion,
                32,
                &mut support::TransmissionRng,
            )
            .unwrap()
            .landing()
            .unwrap();
            assert!((landing.position - ground.coords).norm() < 1e-12);
            let f1 = fresnel_dielectric(-ray.direction.dot(&entry.normal), 1.0, n).unwrap();
            let f2 = fresnel_dielectric(inside.direction.dot(&exit.normal), n, 1.0).unwrap();
            assert!(
                landing.weight == 1.0 && (0.0..=1.0).contains(&f1) && (0.0..=1.0).contains(&f2)
            );
            assert!(landing.weight > 0.0 && landing.weight <= 1.0);
            if x == 0.0 {
                let f = ((n - 1.0) / (n + 1.0)).powi(2);
                assert!((f1 - f).abs() < 1e-14 && (f2 - f).abs() < 1e-14);
            }
        }
        let blue = spectral_caustics::integrator::photon::trace_photon(
            &scene,
            ray,
            Some(400.0),
            IorMode::Dispersion,
            32,
            &mut support::TransmissionRng,
        )
        .unwrap()
        .landing()
        .unwrap();
        let red = spectral_caustics::integrator::photon::trace_photon(
            &scene,
            ray,
            Some(700.0),
            IorMode::Dispersion,
            32,
            &mut support::TransmissionRng,
        )
        .unwrap()
        .landing()
        .unwrap();
        if x > 0.0 {
            assert!(blue.position.x < red.position.x - 1e-4);
        }
        assert_eq!(
            spectral_caustics::integrator::photon::trace_photon(
                &scene,
                ray,
                Some(400.0),
                IorMode::Constant,
                32,
                &mut support::TransmissionRng
            )
            .unwrap()
            .landing(),
            spectral_caustics::integrator::photon::trace_photon(
                &scene,
                ray,
                Some(700.0),
                IorMode::Constant,
                32,
                &mut support::TransmissionRng
            )
            .unwrap()
            .landing()
        );
    }
}

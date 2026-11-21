use nalgebra::{Vector2, Vector3};
use spectral_caustics::caustics::SpectralCausticsMap;
use spectral_caustics::config::Config;
use spectral_caustics::config::RenderJob;
use spectral_caustics::config::render::{CameraMode, RenderSetting};
use spectral_caustics::config::scene::{EnvironmentMisHeuristic, ObjectConfig};
use spectral_caustics::film::Framebuffer;
use spectral_caustics::geometry::{ground::Ground, ray::Ray, vertical_prism::VerticalPrism};
use spectral_caustics::integrator::photon::{integrate_xyz, trace_photon};
use spectral_caustics::material::{dielectric::Dielectric, lambertian::Lambertian};
use spectral_caustics::medium::{
    GridMediumConfig, HgAnisotropy, HomogeneousMedium, HomogeneousMediumConfig, Medium,
    MediumCoefficient, MediumConfig,
};
use spectral_caustics::optics::dispersion::{Cauchy, IorMode};
use spectral_caustics::renderer::Renderer;
use spectral_caustics::scene::MediumInterface;
use spectral_caustics::spectrum::cie_xyz;
use spectral_caustics::spectrum::d65_normalization;
use spectral_caustics::spectrum::d65_spd;
use spectral_caustics::spectrum::emission::d65_normalization_continuous;
mod support;

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

fn assert_linear_sum(direct: &Framebuffer, caustics: &Framebuffer, combined: &Framebuffer) {
    for ((direct, caustics), combined) in direct
        .pixels()
        .iter()
        .zip(caustics.pixels())
        .zip(combined.pixels())
    {
        assert_eq!(
            (direct + caustics).map(f64::to_bits),
            combined.map(f64::to_bits)
        );
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
fn json_vacuum_interface_matches_manual_binding_in_camera_and_photon_outputs() {
    let c = config(include_str!("../configs/sphere_medium_interface.json"));
    let bound = c.build().unwrap();
    let mut unbound_config = c.clone();
    let ObjectConfig::Sphere {
        medium_interface, ..
    } = &mut unbound_config.scene.objects[0]
    else {
        panic!("expected a glass sphere");
    };
    *medium_interface = None;
    let unbound = unbound_config.build().unwrap();
    let mut manual = unbound_config.build().unwrap();
    let target = manual.photon.caustics_object_id.unwrap();
    manual
        .scene
        .bind_medium_interface(target, MediumInterface { interior: None })
        .unwrap();
    let ground = manual.photon.ground_id.unwrap();

    let xyz = |job| integrate_xyz(job, Some(target), ground, &c.render).unwrap();
    let json_xyz = xyz(&bound);
    let rust_xyz = xyz(&manual);
    same(&json_xyz.caustics, &rust_xyz.caustics);
    same(
        json_xyz.direct.as_ref().unwrap(),
        rust_xyz.direct.as_ref().unwrap(),
    );

    // Removing extinction inside the glass increases the deposited caustic flux.
    // Geometrically direct photons never enter the sphere.
    let unbound_xyz = xyz(&unbound);
    same(
        json_xyz.direct.as_ref().unwrap(),
        unbound_xyz.direct.as_ref().unwrap(),
    );
    let luminance = |image: &Framebuffer| image.pixels().iter().map(|p| p.y).sum::<f64>();
    assert!(luminance(&json_xyz.caustics) > luminance(&unbound_xyz.caustics));

    let json_atlas = map(&bound, &c.render, 1);
    let rust_atlas = map(&manual, &c.render, 4);
    assert_eq!(
        json_atlas.wavelength_samples(),
        rust_atlas.wavelength_samples()
    );
    for layer in 0..json_atlas.wavelength_samples().len() {
        let a = json_atlas.layer_irradiance(layer).unwrap();
        let b = rust_atlas.layer_irradiance(layer).unwrap();
        assert!(a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()));
    }

    for (camera_mode, packet) in [
        (CameraMode::Rgb, 1),
        (CameraMode::Spectral, 1),
        (CameraMode::Spectral, 4),
    ] {
        let settings = RenderSetting {
            camera_mode,
            spectral_packet_size: packet,
            ..c.render.clone()
        };
        let renderer = Renderer::new(settings.clone()).unwrap();
        let json_image = renderer.render(&bound).unwrap();
        let rust_image = Renderer::new(RenderSetting {
            thread_count: Some(4),
            ..settings
        })
        .unwrap()
        .render(&manual)
        .unwrap();
        same(&json_image.direct, &rust_image.direct);
        same(&json_image.caustics, &rust_image.caustics);
        same(&json_image.combined, &rust_image.combined);
        assert_ne!(
            json_image.direct.pixels(),
            renderer.render(&unbound).unwrap().direct.pixels()
        );
    }
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
        sigma_a_base: 0.2.into(),
        sigma_s_base: 0.3.into(),
        g: 0.7.into(),
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
        let mut spectral_hg = grid.clone();
        spectral_hg.g = HgAnisotropy::Curve {
            samples: vec![[380.0, -0.6], [780.0, 0.7]],
        };
        c.scene.medium = Some(MediumConfig::Grid(spectral_hg));
        let hg = c.build().unwrap();
        let hg_xyz = xyz(&hg);
        same(&fog_xyz.caustics, &hg_xyz.caustics);
        same(
            fog_xyz.direct.as_ref().unwrap(),
            hg_xyz.direct.as_ref().unwrap(),
        );
        let hg_atlas = map(&hg, &c.render, 2);
        for layer in 0..hg_atlas.wavelength_samples().len() {
            let fog = fog_atlas.layer_irradiance(layer).unwrap();
            let hg = hg_atlas.layer_irradiance(layer).unwrap();
            assert!(fog.iter().zip(hg).all(|(a, b)| a.to_bits() == b.to_bits()));
        }
        let land = |job| {
            let ray = Ray::new(Vector3::z() * 3.0, -Vector3::z()).unwrap();
            trace_photon(
                job,
                ray,
                None,
                IorMode::Constant,
                4,
                &mut support::TransmissionRng,
            )
            .unwrap()
            .landing()
            .unwrap()
        };
        assert_eq!(land(&fog), land(&hg));

        let mut spectral_grid = grid.clone();
        spectral_grid.sigma_a_base = MediumCoefficient::Curve {
            samples: vec![[380.0, 0.9], [780.0, 0.1]],
        };
        c.scene.medium = Some(MediumConfig::Grid(spectral_grid));
        let filtered = map(&c.build().unwrap(), &c.render, 2);
        for (i, &(nm, _)) in filtered.wavelength_samples().iter().enumerate() {
            let sigma_t = 1.2 - 0.8 * ((nm - 380.0) / 400.0);
            let tr = (-sigma_t * 0.4 * 1.0_f64.hypot(0.45)).exp();
            for (&clear, &filtered) in clear_atlas
                .layer_irradiance(i)
                .unwrap()
                .iter()
                .zip(filtered.layer_irradiance(i).unwrap())
            {
                assert!((clear * tr - filtered).abs() <= 2e-12 * clear);
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
    let absorption = |sigma_a: f64| -> Box<dyn Medium> {
        Box::new(
            HomogeneousMedium::try_from(&HomogeneousMediumConfig {
                min_m: [-10.0; 3],
                max_m: [10.0; 3],
                sigma_a: sigma_a.into(),
                sigma_s: 0.0.into(),
                g: 0.0.into(),
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
fn constant_ior_spectral_absorption_preserves_landings_and_filters_xyz_and_atlas() {
    let mut job = RenderJob::new();
    let light = job.scene.light.as_parallel_mut().unwrap();
    light.direction = -Vector3::z();
    light.height_m = 3.0;
    light.aperture_min_m = Vector2::repeat(-2.0);
    light.aperture_max_m = Vector2::repeat(2.0);
    let plate = VerticalPrism::new(
        [
            Vector2::new(-1.0, -1.0),
            Vector2::new(1.0, -1.0),
            Vector2::new(0.0, 1.0),
        ],
        1.0,
        2.0,
    )
    .unwrap();
    let ground = job.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    let target = job
        .scene
        .add(Box::new(plate), Box::new(Dielectric::new(1.5).unwrap()));
    job.photon.ground_id = Some(ground);
    job.select_caustics_object(target).unwrap();
    let settings = RenderSetting {
        width: 12,
        height: 8,
        samples_per_pixel_axis: 2,
        mode: IorMode::Constant,
        wavelength_step_nm: 5.0,
        photon_max_events: 2,
        ..Default::default()
    };
    let xyz = |job: &RenderJob, target, threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| integrate_xyz(job, target, ground, &settings).unwrap())
    };
    let clear_atlas = map(&job, &settings, 1);
    let clear_xyz = xyz(&job, Some(target), 1);
    let clear_direct_only = xyz(&job, None, 1);
    let absorption = |blue, red| -> Box<dyn Medium> {
        Box::new(
            HomogeneousMedium::try_from(&HomogeneousMediumConfig {
                min_m: [-4.0; 3],
                max_m: [4.0; 3],
                sigma_a: MediumCoefficient::Curve {
                    samples: vec![[380.0, blue], [780.0, red]],
                },
                sigma_s: 0.0.into(),
                g: 0.0.into(),
            })
            .unwrap(),
        )
    };
    job.scene.medium = Some(absorption(0.3, 0.05));
    job.scene
        .bind_medium_interface(
            target,
            MediumInterface {
                interior: Some(absorption(1.2, 0.1)),
            },
        )
        .unwrap();
    let sigma_exterior = |nm: f64| 0.3 - 0.25 * ((nm - 380.0) / 400.0);
    let sigma_interior = |nm: f64| 1.2 - 1.1 * ((nm - 380.0) / 400.0);
    let direct_tr = |nm| (-3.0 * sigma_exterior(nm)).exp();
    let caustic_tr = |nm| (-2.0 * sigma_exterior(nm) - sigma_interior(nm)).exp();
    for nm in [450.0, 550.0, 650.0] {
        for (height, exterior, interior) in [(3.0, 2.0, 1.0), (1.5, 1.0, 0.5)] {
            let ray = Ray::new(Vector3::z() * height, -Vector3::z()).unwrap();
            let landing = trace_photon(
                &job,
                ray,
                Some(nm),
                IorMode::Constant,
                2,
                &mut support::TransmissionRng,
            )
            .unwrap()
            .landing()
            .unwrap();
            assert_eq!(landing.position, Vector3::zeros());
            let expected = (-exterior * sigma_exterior(nm) - interior * sigma_interior(nm)).exp();
            assert!((landing.weight - expected).abs() < 1e-14);
        }
    }
    let ray = Ray::new(Vector3::z() * 3.0, -Vector3::z()).unwrap();
    for wavelength in [None, Some(f64::NAN), Some(800.0)] {
        assert!(
            trace_photon(
                &job,
                ray,
                wavelength,
                IorMode::Constant,
                2,
                &mut support::TransmissionRng,
            )
            .is_err()
        );
    }

    let filtered_xyz = xyz(&job, Some(target), 1);
    for threads in [2, 4] {
        let parallel = xyz(&job, Some(target), threads);
        same(&filtered_xyz.caustics, &parallel.caustics);
        same(
            filtered_xyz.direct.as_ref().unwrap(),
            parallel.direct.as_ref().unwrap(),
        );
    }
    let direct_only = xyz(&job, None, 4);
    same(
        filtered_xyz.direct.as_ref().unwrap(),
        direct_only.direct.as_ref().unwrap(),
    );
    assert!(
        direct_only
            .caustics
            .pixels()
            .iter()
            .all(|v| *v == Vector3::zeros())
    );
    assert_eq!(
        clear_xyz.sun_irradiance_xyz,
        filtered_xyz.sun_irradiance_xyz
    );
    // Integrate the analytic filtered spectrum on an independent 1 nm grid.
    let reference_xyz = |tr: &dyn Fn(f64) -> f64| {
        let mut xyz = Vector3::zeros();
        let mut y = 0.0;
        for nm in 380..=780 {
            let width = if nm == 380 || nm == 780 { 0.5 } else { 1.0 };
            let nm = f64::from(nm);
            let source = cie_xyz(nm).unwrap() * (d65_spd(nm).unwrap() * width);
            xyz += source * tr(nm);
            y += source.y;
        }
        xyz / y
    };
    for (clear, filtered, expected) in [
        (
            &clear_xyz.caustics,
            &filtered_xyz.caustics,
            reference_xyz(&caustic_tr),
        ),
        (
            clear_direct_only.direct.as_ref().unwrap(),
            direct_only.direct.as_ref().unwrap(),
            reference_xyz(&direct_tr),
        ),
    ] {
        assert!(clear.pixels().iter().any(|p| p.y > 0.0));
        for (a, b) in clear.pixels().iter().zip(filtered.pixels()) {
            assert!((a.y * expected - b).norm() <= 8e-4 * a.y);
        }
    }
    let filtered_atlas = map(&job, &settings, 1);
    let absorption_with_volume_budget = map(
        &job,
        &RenderSetting {
            photon_max_volume_scatters: 2,
            ..settings.clone()
        },
        1,
    );
    let mut reintegrated = vec![Vector3::zeros(); filtered_xyz.caustics.pixels().len()];
    for (i, &(nm, width)) in filtered_atlas.wavelength_samples().iter().enumerate() {
        let cie = cie_xyz(nm).unwrap() * width;
        let clear = clear_atlas.layer_irradiance(i).unwrap();
        let filtered = filtered_atlas.layer_irradiance(i).unwrap();
        let with_budget = absorption_with_volume_budget.layer_irradiance(i).unwrap();
        for (((&clear, &filtered), &with_budget), xyz) in clear
            .iter()
            .zip(filtered)
            .zip(with_budget)
            .zip(&mut reintegrated)
        {
            assert!((clear * caustic_tr(nm) - filtered).abs() <= 2e-12 * clear);
            assert!((with_budget - filtered).abs() <= 2e-12 * filtered);
            *xyz += cie * filtered;
        }
    }
    for (xyz, expected) in reintegrated.iter().zip(filtered_xyz.caustics.pixels()) {
        assert!((xyz - expected).norm() <= 2e-12 * expected.norm());
    }
    for threads in [2, 4] {
        let parallel = map(&job, &settings, threads);
        for i in 0..filtered_atlas.wavelength_samples().len() {
            assert!(
                filtered_atlas
                    .layer_irradiance(i)
                    .unwrap()
                    .iter()
                    .zip(parallel.layer_irradiance(i).unwrap())
                    .all(|(a, b)| a.to_bits() == b.to_bits())
            );
        }
    }
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
fn photon_volume_budget_defaults_to_zero_and_roundtrips_in_json() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../configs/sphere_medium_interface.json")).unwrap();
    let parse = |value: &serde_json::Value| Config::from_json(&value.to_string()).unwrap();
    let default = parse(&value);
    assert_eq!(default.render.photon_max_volume_scatters, 0);
    value["render"]["photon_max_volume_scatters"] = serde_json::json!(0);
    assert_eq!(parse(&value).render, default.render);
    for budget in [1, 4, 1024] {
        value["render"]["photon_max_volume_scatters"] = serde_json::json!(budget);
        let enabled = parse(&value);
        enabled.validate().unwrap();
        assert_eq!(enabled.render.photon_max_volume_scatters, budget);
        assert_eq!(
            parse(&serde_json::to_value(&enabled).unwrap()).render,
            enabled.render
        );
    }
    value["render"]["photon_max_volume_scatters"] = serde_json::json!(1025);
    assert!(
        parse(&value)
            .validate()
            .unwrap_err()
            .contains("photon_max_volume_scatters")
    );
    value["render"]["photon_max_volume_scatters"] = serde_json::json!(-1);
    assert!(Config::from_json(&value.to_string()).is_err());
}

#[test]
fn photon_volume_landing_maps_reintegrate_and_repeat_across_threads() {
    let mut c = config(include_str!("../configs/sphere_medium_interface.json"));
    c.render.width = 24;
    c.render.height = 18;
    c.render.samples_per_pixel_axis = 3;
    let min_m = [-3.0, -3.0, 0.0];
    let max_m = [3.0, 3.0, 0.49];
    let homogeneous = |g| {
        MediumConfig::Homogeneous(HomogeneousMediumConfig {
            min_m,
            max_m,
            sigma_a: 0.2.into(),
            sigma_s: 2.0.into(),
            g,
        })
    };
    let g_curve = HgAnisotropy::Curve {
        samples: vec![[380.0, -0.2], [780.0, 0.85]],
    };
    let grid = MediumConfig::Grid(GridMediumConfig {
        min_m,
        max_m,
        resolution: [1, 1, 2],
        densities: vec![0.6, 1.4],
        sigma_a_base: 0.2.into(),
        sigma_s_base: MediumCoefficient::Curve {
            samples: vec![[380.0, 1.0], [780.0, 3.0]],
        },
        g: 0.45.into(),
    });
    for (medium, mode, phase_only) in [
        (homogeneous(0.45.into()), IorMode::Constant, false),
        (homogeneous(g_curve), IorMode::Constant, true),
        (grid.clone(), IorMode::Constant, false),
        (grid, IorMode::Dispersion, false),
    ] {
        c.scene.medium = Some(medium);
        c.render.mode = mode;
        c.render.camera_mode = CameraMode::Spectral;
        c.render.photon_max_volume_scatters = 2;
        let job = c.build().unwrap();
        let target = job.photon.caustics_object_id;
        let ground = job.photon.ground_id.unwrap();
        let xyz = |settings, threads| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| integrate_xyz(&job, target, ground, settings).unwrap())
        };
        let enabled = &c.render;
        let disabled = RenderSetting {
            photon_max_volume_scatters: 0,
            ..enabled.clone()
        };
        let baseline = xyz(&disabled, 1);
        let serial = xyz(enabled, 1);
        same(
            baseline.direct.as_ref().unwrap(),
            serial.direct.as_ref().unwrap(),
        );
        // A pixel outside the ballistic footprint can only receive photons
        // whose travel direction changed in the fog below the sphere.
        assert!(
            baseline
                .caustics
                .pixels()
                .iter()
                .zip(serial.caustics.pixels())
                .any(|(old, new)| old.y == 0.0 && new.y > 0.0)
        );
        let atlas = map(&job, enabled, 1);
        let mut integral = vec![Vector3::zeros(); serial.caustics.pixels().len()];
        for (i, &(nm, width)) in atlas.wavelength_samples().iter().enumerate() {
            for (xyz, &e) in integral.iter_mut().zip(atlas.layer_irradiance(i).unwrap()) {
                *xyz += cie_xyz(nm).unwrap() * (e * width);
            }
        }
        for (integrated, xyz) in integral.iter().zip(serial.caustics.pixels()) {
            assert!((integrated - xyz).norm() <= 2e-12 * xyz.norm());
        }
        if phase_only {
            let phase_changes_footprint = |map: &SpectralCausticsMap| {
                let last = map.wavelength_samples().len() - 1;
                let (blue, red) = (
                    map.layer_irradiance(0).unwrap(),
                    map.layer_irradiance(last).unwrap(),
                );
                blue.iter().zip(red).any(|(&a, &b)| {
                    let a = a / d65_spd(380.0).unwrap();
                    let b = b / d65_spd(780.0).unwrap();
                    (a - b).abs() > 1e-12 * a.max(b)
                })
            };
            assert!(!phase_changes_footprint(&map(&job, &disabled, 1)));
            assert!(phase_changes_footprint(&atlas));
            let render = |settings: &RenderSetting, threads| {
                Renderer::new(RenderSetting {
                    thread_count: Some(threads),
                    spectral_packet_size: 4,
                    ..settings.clone()
                })
                .unwrap()
                .render(&job)
                .unwrap()
            };
            let enabled_image = render(enabled, 1);
            let disabled_image = render(&disabled, 1);
            same(&enabled_image.direct, &disabled_image.direct);
            assert_ne!(
                enabled_image.caustics.pixels(),
                disabled_image.caustics.pixels()
            );
            assert_linear_sum(
                &disabled_image.direct,
                &disabled_image.caustics,
                &disabled_image.combined,
            );
            let mut previous_caustics = disabled_image.caustics;
            for photon_max_volume_scatters in [1, 4] {
                let image = render(
                    &RenderSetting {
                        photon_max_volume_scatters,
                        ..enabled.clone()
                    },
                    1,
                );
                same(&image.direct, &disabled_image.direct);
                assert_linear_sum(&image.direct, &image.caustics, &image.combined);
                assert_ne!(previous_caustics.pixels(), image.caustics.pixels());
                previous_caustics = image.caustics;
            }
            for threads in [2, 4] {
                let parallel = render(enabled, threads);
                same(&enabled_image.direct, &parallel.direct);
                same(&enabled_image.caustics, &parallel.caustics);
                same(&enabled_image.combined, &parallel.combined);
            }
        }
        for threads in [2, 4] {
            let parallel = xyz(enabled, threads);
            same(&serial.caustics, &parallel.caustics);
            same(
                serial.direct.as_ref().unwrap(),
                parallel.direct.as_ref().unwrap(),
            );
            let parallel = map(&job, enabled, threads);
            for i in 0..atlas.wavelength_samples().len() {
                let a = atlas.layer_irradiance(i).unwrap();
                let b = parallel.layer_irradiance(i).unwrap();
                assert!(a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits()));
            }
        }
    }
}

#[test]
fn photon_volume_caustics_stay_zero_when_all_emission_misses_the_glass() {
    let mut c = config(include_str!("../configs/sphere_medium_interface.json"));
    c.render.mode = IorMode::Constant;
    c.render.photon_max_volume_scatters = 4;
    let light = c.scene.light.as_parallel_mut().unwrap();
    // The glass ends at x=0.8. All photons start farther to the right,
    // traveling towards +x through scattering medium to the receiver.
    light.aperture_min_m = [0.95, -0.8];
    light.aperture_max_m = [1.15, 0.8];
    let job = c.build().unwrap();
    let target = job.photon.caustics_object_id.unwrap();
    let ground = job.photon.ground_id.unwrap();
    let lighting = integrate_xyz(&job, Some(target), ground, &c.render).unwrap();
    assert!(lighting.direct.unwrap().pixels().iter().any(|p| p.y > 0.0));
    assert!(
        lighting
            .caustics
            .pixels()
            .iter()
            .all(|p| *p == Vector3::zeros())
    );
    let atlas = map(&job, &c.render, 1);
    for i in 0..atlas.wavelength_samples().len() {
        assert!(atlas.layer_irradiance(i).unwrap().iter().all(|&v| v == 0.0));
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

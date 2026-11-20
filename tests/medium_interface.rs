use nalgebra::{Vector2, Vector3};
use rand::{RngCore, SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use spectral_caustics::{
    camera::Camera,
    config::{
        Config, RenderJob,
        camera::CameraConfig,
        render::{CameraMode, RenderSetting},
    },
    geometry::{
        ground::Ground, prism::Prism, ray::Ray, sphere::Sphere, vertical_prism::VerticalPrism,
    },
    integrator::camera::{
        path::{MAX_DELTA_EVENTS, trace_camera_path},
        transport::{CameraTransport, Caustics},
        types::{CameraPathRngs, CameraPathState},
    },
    integrator::photon::{PhotonMedium, PhotonTermination, trace_photon},
    light::{DiskAreaLight, Light},
    material::{
        Material, ScatterType, dielectric::Dielectric, lambertian::Lambertian, mirror::Mirror,
    },
    medium::{HomogeneousMedium, HomogeneousMediumConfig, Medium, MediumCoefficient},
    optics::{
        dispersion::IorMode,
        interface::{DielectricEvent, sample_dielectric},
    },
    renderer::Renderer,
    scene::{
        MediumInterface,
        MediumRegion::{Exterior, Interior},
    },
};

fn interface_example_json() -> Value {
    serde_json::from_str(include_str!("../configs/sphere_medium_interface.json")).unwrap()
}

#[test]
fn json_medium_interface_roundtrip_preserves_binding_and_interior_extinction() {
    let homogeneous = json!({
        "min_m": [-1.0, -1.0, 0.0], "max_m": [1.0, 1.0, 3.0],
        "sigma_a": 0.5, "sigma_s": 0.0, "g": 0.0
    });
    let grid = json!({
        "min_m": [-1.0, -1.0, 0.0], "max_m": [1.0, 1.0, 3.0],
        "resolution": [1, 1, 1], "densities": [2.0],
        "sigma_a_base": 0.75, "sigma_s_base": 0.0, "g": 0.0
    });
    let vacuum = json!({"interior": null});
    for (input, serialized, region, sigma_t) in [
        (None, None, Exterior, 0.32_f64),
        (Some(Value::Null), None, Exterior, 0.32),
        (Some(vacuum.clone()), Some(vacuum.clone()), Interior(0), 0.0),
        (Some(json!({})), Some(vacuum), Interior(0), 0.0),
        (
            Some(json!({"interior": homogeneous})),
            Some(json!({"interior": homogeneous})),
            Interior(0),
            0.5,
        ),
        (
            Some(json!({"interior": grid})),
            Some(json!({"interior": grid})),
            Interior(0),
            1.5,
        ),
    ] {
        let mut json = interface_example_json();
        let object = json["scene"]["objects"][0].as_object_mut().unwrap();
        object.insert("caustics".into(), Value::Bool(false));
        match input {
            Some(input) => object.insert("medium_interface".into(), input),
            None => object.remove("medium_interface"),
        };
        let config = Config::from_json(&json.to_string()).unwrap();
        let text = config.to_json().unwrap();
        let saved: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            saved["scene"]["objects"][0].get("medium_interface"),
            serialized.as_ref()
        );
        let reloaded = Config::from_json(&text).unwrap();
        assert_eq!(config, reloaded);
        let job = reloaded.build().unwrap();
        assert_eq!(job.photon.caustics_object_id, None);
        let ray = Ray::new(Vector3::new(0.0, 0.0, 1.3), Vector3::x()).unwrap();
        let actual_region = job.scene.initial_medium_region(ray.origin, None).unwrap();
        assert_eq!(actual_region, region);
        let tr = job
            .scene
            .active_medium(actual_region)
            .unwrap()
            .map_or(1.0, |medium| medium.transmittance_ray(&ray, 0.25, 550.0));
        assert!((tr - (-sigma_t * 0.25).exp()).abs() < 1e-14);
    }
}

#[test]
fn json_medium_interface_binds_mixed_scene_object_indices_and_prism_shapes() {
    let ordinary = serde_json::to_value(Config::default()).unwrap();
    let vertical = json!({
        "type": "vertical_prism",
        "cross_section_m": [[-1.0, -1.0], [1.0, -1.0], [0.0, 1.0]],
        "z_min_m": 0.5, "z_max_m": 1.5
    });
    for (mut prism, interior_point) in [
        (
            ordinary["scene"]["objects"][0].clone(),
            Vector3::new(0.0, 0.0, 0.3),
        ),
        (vertical, Vector3::new(0.0, 0.0, 1.0)),
    ] {
        let mut scene = interface_example_json();
        prism["medium_interface"] = json!({"interior": null});
        let objects = scene["scene"]["objects"].as_array_mut().unwrap();
        objects[0]
            .as_object_mut()
            .unwrap()
            .remove("medium_interface");
        objects[0]["caustics"] = json!(false);
        objects.push(prism);
        let config = Config::from_json(&scene.to_string()).unwrap();
        config.validate().unwrap();
        let job = config.build().unwrap();
        let region = job
            .scene
            .initial_medium_region(interior_point, None)
            .unwrap();
        assert_eq!(region, Interior(2));
        assert!(job.scene.active_medium(region).unwrap().is_none());
    }
}

#[test]
fn json_medium_interface_rejects_invalid_bindings_in_validate_and_build() {
    let mut non_glass = interface_example_json();
    non_glass["scene"]["objects"][0]["material"] =
        json!({"type": "lambertian", "albedo": [0.5, 0.5, 0.5]});
    let mut duplicate = interface_example_json();
    let mut second = duplicate["scene"]["objects"][0].clone();
    second["caustics"] = json!(false);
    duplicate["scene"]["objects"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let mut bad_grid = interface_example_json();
    bad_grid["scene"]["objects"][0]["medium_interface"]["interior"] = json!({
        "min_m": [-1.0, -1.0, 0.0], "max_m": [1.0, 1.0, 3.0],
        "resolution": [1, 1, 2], "densities": [2.0],
        "sigma_a_base": 0.75, "sigma_s_base": 0.0, "g": 0.0
    });
    for (cfg, field) in [
        (non_glass, "scene.objects[0].medium_interface"),
        (duplicate, "scene.objects[2].medium_interface"),
        (bad_grid, "scene.objects[0].medium_interface.interior"),
    ] {
        let cfg = Config::from_json(&cfg.to_string()).unwrap();
        let validate = cfg.validate().unwrap_err();
        let build = cfg.build().err().expect("invalid scene built").to_string();
        assert!(validate.contains(field), "{validate}");
        assert!(build.contains(field), "{build}");
    }

    let mut unknown = interface_example_json();
    unknown["scene"]["objects"][0]["medium_interface"]["exterior"] = Value::Null;
    assert!(Config::from_json(&unknown.to_string()).is_err());
    let mut open = interface_example_json();
    open["scene"]["objects"][1]["medium_interface"] = json!({"interior": null});
    assert!(Config::from_json(&open.to_string()).is_err());
    let mut beveled: Value =
        serde_json::from_str(include_str!("../configs/beveled_prism.json")).unwrap();
    beveled["scene"]["objects"][0]["medium_interface"] = json!({"interior": null});
    assert!(Config::from_json(&beveled.to_string()).is_err());
}

fn glass() -> Box<Dielectric> {
    Box::new(Dielectric::new(1.5).unwrap())
}
fn sphere() -> Box<Sphere> {
    Box::new(Sphere::new(Vector3::zeros(), 1.0).unwrap())
}
fn slab(z_max: f64) -> Box<VerticalPrism> {
    // A finite glass plate. The normal-incidence path crosses the two parallel caps.
    Box::new(
        VerticalPrism::new(
            [
                Vector2::new(-10.0, -10.0),
                Vector2::new(10.0, -10.0),
                Vector2::new(0.0, 10.0),
            ],
            1.0,
            z_max,
        )
        .unwrap(),
    )
}
fn absorption(sigma: f64) -> Box<dyn Medium> {
    Box::new(
        HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: [-10.0; 3],
            max_m: [10.0; 3],
            sigma_a: sigma.into(),
            sigma_s: 0.0.into(),
            g: 0.0,
        })
        .unwrap(),
    )
}

fn trace<T: CameraTransport>(job: &RenderJob, state: &mut CameraPathState<T>, sun: f64) {
    trace_camera_path(
        job,
        state,
        &mut CameraPathRngs {
            camera: &mut StdRng::seed_from_u64(1),
            environment: &mut StdRng::seed_from_u64(2),
            diffuse: &mut StdRng::seed_from_u64(3),
            medium: &mut StdRng::seed_from_u64(4),
        },
        Vector3::z(),
        sun,
        Caustics::None,
        IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();
}

#[test]
fn camera_sphere_launches_inside_and_outside_and_attenuates_each_segment() {
    let mut job = RenderJob::new();
    job.scene.environment = None;
    job.scene.background_color = Vector3::repeat(1.0);
    let id = job.scene.add(sphere(), glass());
    job.scene.medium = Some(absorption(0.2));
    job.scene
        .bind_medium_interface(
            id,
            MediumInterface {
                interior: Some(absorption(0.7)),
            },
        )
        .unwrap();

    // Normal incidence. Each segment starts at the real intersection.
    for (z, departure, events, interior_distance, exterior_before, eta_weight) in [
        (0.0, None, 1, 1.0, 0.0, 1.5_f64.powi(2)),
        (-1.0, Some(Interior(id)), 1, 2.0, 0.0, 1.5_f64.powi(2)),
        (-3.0, None, 2, 2.0, 2.0, 1.0),
    ] {
        let ray = Ray::new(Vector3::z() * z, Vector3::z()).unwrap();
        let exterior_distance = exterior_before + 9.0;
        let expected = eta_weight * (-0.7_f64 * interior_distance - 0.2 * exterior_distance).exp();

        let mut rgb = CameraPathState::new(ray, 0);
        rgb.medium_region = departure;
        trace(&job, &mut rgb, 0.0);
        assert_eq!(rgb.delta_events(), events);
        assert_eq!(rgb.medium_region, Some(Exterior));
        assert!((rgb.radiance.direct - Vector3::repeat(expected)).norm() < 1e-14);

        let mut spectral = CameraPathState::spectral(ray, 550.0, 0).unwrap();
        spectral.medium_region = departure;
        trace(&job, &mut spectral, 0.0);
        assert_eq!(spectral.delta_events(), events);
        assert_eq!(spectral.medium_region, Some(Exterior));
        assert!((spectral.throughput - expected).abs() < 1e-14);
    }
}

#[test]
fn camera_sphere_uses_the_path_wavelength_in_the_bound_interior() {
    let mut job = RenderJob::new();
    job.scene.environment = None;
    job.scene.background_color = Vector3::repeat(1.0);
    let id = job.scene.add(sphere(), glass());
    let interior = HomogeneousMedium::try_from(&HomogeneousMediumConfig {
        min_m: [-2.0; 3],
        max_m: [2.0; 3],
        sigma_a: MediumCoefficient::Curve {
            samples: vec![[380.0, 0.2], [780.0, 1.0]],
        },
        sigma_s: 0.0.into(),
        g: 0.0,
    })
    .unwrap();
    job.scene
        .bind_medium_interface(
            id,
            MediumInterface {
                interior: Some(Box::new(interior)),
            },
        )
        .unwrap();

    let ray = Ray::new(-3.0 * Vector3::z(), Vector3::z()).unwrap();
    for (nm, sigma_a) in [(480.0, 0.4_f64), (680.0, 0.8)] {
        let mut state = CameraPathState::spectral(ray, nm, 0).unwrap();
        trace(&job, &mut state, 0.0);
        assert_eq!(state.delta_events(), 2);
        assert_eq!(state.medium_region, Some(Exterior));
        assert!((state.throughput - (-2.0 * sigma_a).exp()).abs() < 1e-14);
    }
}

#[test]
fn camera_plate_keeps_the_incident_medium_across_reflection_and_tir() {
    let mut job = RenderJob::new();
    let id = job.scene.add(slab(2.0), glass());
    job.scene.medium = Some(absorption(0.2));
    job.scene
        .bind_medium_interface(
            id,
            MediumInterface {
                interior: Some(absorption(0.7)),
            },
        )
        .unwrap();

    // Starting inside, cos(theta) = 0.6 is below the critical cosine.
    // Allow one TIR, then measure attenuation to the opposite cap.
    let ray = Ray::new(Vector3::z() * 1.5, Vector3::new(0.8, 0.0, 0.6)).unwrap();
    let mut state = CameraPathState::new(ray, 0);
    state.path_depth = MAX_DELTA_EVENTS - 1;
    trace(&job, &mut state, 0.0);
    assert_eq!(state.delta_events(), MAX_DELTA_EVENTS);
    assert_eq!(state.last_scatter, Some(ScatterType::SpecularReflection));
    assert_eq!(state.medium_region, Some(Interior(id)));
    assert!(state.ray.direction.z < 0.0);
    let expected = (-0.7_f64 * 1.5 / 0.6).exp();
    assert!((state.throughput - Vector3::repeat(expected)).norm() < 1e-14);

    // A grazing front-face hit reflects with seed 1. The reflected ray
    // traverses the exterior box on its way to y = 10.
    let ray = Ray::new(
        Vector3::new(0.0, -5.0, 2.004),
        Vector3::new(0.0, 1.0, -0.001),
    )
    .unwrap();
    let mut state = CameraPathState::new(ray, 0);
    trace(&job, &mut state, 0.0);
    assert_eq!(state.delta_events(), 1);
    assert_eq!(state.last_scatter, Some(ScatterType::SpecularReflection));
    assert_eq!(state.medium_region, Some(Exterior));
    let expected = (-0.2_f64 * 15.0 / ray.direction.y).exp();
    assert!((state.throughput - Vector3::repeat(expected)).norm() < 1e-14);
}

#[test]
fn camera_thin_plate_finds_both_faces_and_attenuates_only_inside() {
    let mut job = RenderJob::new();
    job.scene.environment = None;
    job.scene.background_color = Vector3::repeat(1.0);
    let thickness = 5e-6;
    let id = job.scene.add(slab(1.0 + thickness), glass());
    job.scene
        .bind_medium_interface(
            id,
            MediumInterface {
                interior: Some(absorption(0.7)),
            },
        )
        .unwrap();

    let ray = Ray::new(Vector3::zeros(), Vector3::z()).unwrap();
    let mut state = CameraPathState::new(ray, 0);
    trace(&job, &mut state, 0.0);
    assert_eq!(state.delta_events(), 2);
    assert_eq!(state.medium_region, Some(Exterior));
    // Both interfaces transmit with seed 1, so their radiance eta factors cancel.
    let expected = (-0.7_f64 * thickness).exp();
    assert!((state.radiance.direct - Vector3::repeat(expected)).norm() < 1e-14);
}

#[test]
fn photon_tir_inside_five_micron_plate_attenuates_both_segments_without_rng() {
    let thickness = 5e-6;
    let mut job = RenderJob::new();
    let target = job.scene.add(slab(1.0 + thickness), glass());
    job.photon.ground_id = Some(job.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    ));
    job.select_caustics_object(target).unwrap();
    job.scene.medium = Some(absorption(0.2));
    job.scene
        .bind_medium_interface(
            target,
            MediumInterface {
                interior: Some(absorption(140_000.0)),
            },
        )
        .unwrap();

    let ray = Ray::new(
        Vector3::z() * (1.0 + thickness / 2.0),
        Vector3::new(0.8, 0.0, 0.6),
    )
    .unwrap();
    let mut rng = StdRng::seed_from_u64(42);
    let mut untouched = rng.clone();
    let result = trace_photon(&job, ray, None, IorMode::Constant, 1, &mut rng).unwrap();
    assert_eq!(result.termination, PhotonTermination::EventLimit);
    assert_eq!(result.state.event_count, 1);
    assert_eq!(result.state.medium, PhotonMedium::Glass);
    assert_eq!(result.state.medium_region, Interior(target));
    assert!(result.state.ray.direction.z < 0.0);
    assert_eq!(rng.next_u64(), untouched.next_u64());

    // The first segment crosses half the thickness and the reflected segment
    // crosses the whole thickness, each at cos(theta) = 0.6.
    let expected = (-140_000.0_f64 * (1.5 * thickness) / 0.6).exp();
    assert!((result.state.flux_weight - expected).abs() < 2e-10);
}

#[test]
fn camera_shadows_see_the_glass_boundary_five_microns_from_the_surface() {
    let mut job = RenderJob::new();
    job.scene.environment = None;
    let id = job.scene.add(sphere(), glass());
    job.scene
        .bind_medium_interface(id, MediumInterface { interior: None })
        .unwrap();
    job.scene.add(
        Box::new(Ground::new(1.0 - 5e-6).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.5)).unwrap()),
    );
    let ray = Ray::new(Vector3::z() * (1.0 - 2.5e-6), -Vector3::z()).unwrap();
    let mut sun = CameraPathState::new(ray, 0);
    trace(&job, &mut sun, 1.0);
    assert_eq!(sun.medium_region, Some(Interior(id)));
    assert_eq!(sun.radiance.direct, Vector3::zeros());

    job.scene.light =
        Light::DiskArea(DiskAreaLight::new(1.5 * Vector3::z(), -Vector3::z(), 1e-4, 1.0).unwrap());
    let mut disk = CameraPathState::new(ray, 0);
    trace(&job, &mut disk, 0.0);
    assert_eq!(disk.radiance.direct, Vector3::zeros());
}

#[test]
fn camera_disk_shadow_segment_inside_sphere_uses_the_interior_medium() {
    let radiance = |interior| {
        let mut job = RenderJob::new();
        job.scene.environment = None;
        job.scene.medium = Some(absorption(4.0));
        job.scene.light = Light::DiskArea(
            DiskAreaLight::new(0.5 * Vector3::z(), -Vector3::z(), 1e-4, 1.0).unwrap(),
        );
        let id = job.scene.add(sphere(), glass());
        job.scene.add(
            Box::new(Ground::new(-0.5).unwrap()),
            Box::new(Lambertian::new(Vector3::repeat(0.5)).unwrap()),
        );
        job.scene
            .bind_medium_interface(id, MediumInterface { interior })
            .unwrap();
        let ray = Ray::new(Vector3::zeros(), -Vector3::z()).unwrap();
        let mut state = CameraPathState::new(ray, 0);
        trace(&job, &mut state, 0.0);
        state.radiance.direct.x
    };
    let vacuum = radiance(None);
    let absorbing = radiance(Some(absorption(0.7)));
    assert!(vacuum > 0.0);
    // Camera segment is 0.5 m. The near-point emitter is 1 m from the floor
    // to within O(disk_radius²).
    let expected = (-0.7_f64 * 1.5).exp();
    assert!((absorbing / vacuum - expected).abs() < 1e-8);
}

#[test]
fn bound_sphere_renders_deterministically_in_rgb_hero_and_packet() {
    let mut job = RenderJob::new();
    job.camera = Camera::from_config(&CameraConfig {
        position: [0.0; 3],
        target: [0.0, 0.0, 1.0],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 35.0,
    })
    .unwrap();
    job.scene.environment = None;
    job.scene.background_color = Vector3::repeat(1.0);
    let id = job.scene.add(sphere(), glass());
    job.scene
        .bind_medium_interface(
            id,
            MediumInterface {
                interior: Some(absorption(0.7)),
            },
        )
        .unwrap();
    for (mode, packet) in [
        (CameraMode::Rgb, 1),
        (CameraMode::Spectral, 1),
        (CameraMode::Spectral, 4),
    ] {
        let mut settings = RenderSetting {
            width: 8,
            height: 8,
            camera_spp: 16,
            seed: 42,
            camera_mode: mode,
            spectral_packet_size: packet,
            ..Default::default()
        };
        let mut reference = None;
        for threads in [1, 2, 4] {
            settings.thread_count = Some(threads);
            let image = Renderer::new(settings.clone())
                .unwrap()
                .render(&job)
                .unwrap();
            for (direct, caustics) in image.direct.pixels().iter().zip(image.caustics.pixels()) {
                assert!(direct.y.is_finite() && direct.y > 0.0);
                assert_eq!(*caustics, Vector3::zeros());
            }
            assert_eq!(image.direct.pixels(), image.combined.pixels());
            if let Some(pixels) = &reference {
                assert_eq!(image.combined.pixels(), pixels);
            } else {
                reference = Some(image.combined.pixels().to_vec());
            }
        }
    }
}

#[test]
fn sphere_classifies_launch_before_any_intersection_and_resolves_vacuum() {
    let mut scene = RenderJob::new().scene;
    let other = scene.add(Box::new(Ground::new(0.25).unwrap()), glass());
    let id = scene.add(sphere(), glass());
    scene.medium = Some(absorption(0.5));
    let ray = Ray::new(Vector3::zeros(), Vector3::z()).unwrap();
    assert_eq!(scene.initial_medium_region(ray.origin, None), Ok(Exterior));
    scene
        .bind_medium_interface(id, MediumInterface { interior: None })
        .unwrap();
    assert_eq!(scene.intersect(&ray).unwrap().object_id, other);
    assert_eq!(
        scene.initial_medium_region(ray.origin, None),
        Ok(Interior(id))
    );
    assert!(scene.active_medium(Interior(id)).unwrap().is_none());
    assert_eq!(
        scene
            .active_medium(Exterior)
            .unwrap()
            .unwrap()
            .transmittance_ray(&ray, 1.0, 550.0),
        (-0.5_f64).exp()
    );
    assert_eq!(
        scene.initial_medium_region(Vector3::x() * 2.0, None),
        Ok(Exterior)
    );
    assert!(
        scene
            .initial_medium_region(ray.origin, Some(Exterior))
            .is_err()
    );
    assert!(scene.initial_medium_region(Vector3::x(), None).is_err());
    for side in [Exterior, Interior(id)] {
        assert_eq!(
            scene.initial_medium_region(Vector3::x(), Some(side)),
            Ok(side)
        );
    }
    assert!(
        scene
            .initial_medium_region(Vector3::x(), Some(Interior(other)))
            .is_err()
    );
    assert!(
        scene
            .initial_medium_region(Vector3::repeat(f64::NAN), None)
            .is_err()
    );
    assert!(scene.active_medium(Interior(other)).is_err());
    let hit = scene.intersect(&ray).unwrap();
    assert_eq!(
        scene.region_after_event(
            Interior(id),
            &hit,
            ray.direction,
            ScatterType::DiffuseReflection
        ),
        Ok(Interior(id))
    );
}

#[test]
fn warped_prism_classifies_launch_against_the_hit_triangle() {
    let prism = Prism::from_vertices([
        Vector3::new(-0.5, -1.0, 1.0),
        Vector3::new(0.5, -1.0, 1.0),
        Vector3::new(0.0, -1.0, 1.8),
        Vector3::new(-0.5, 1.0, 1.0),
        Vector3::new(0.5, 1.0, 1.0 + 1e-9),
        Vector3::new(0.0, 1.0, 1.8),
    ])
    .unwrap();
    let mut scene = RenderJob::new().scene;
    let id = scene.add(Box::new(prism), glass());
    scene
        .bind_medium_interface(id, MediumInterface { interior: None })
        .unwrap();

    // Probe each bottom triangle and their shared diagonal.
    for (x, y, z) in [
        (0.25, -0.5, 1.00000000025),
        (-0.25, 0.5, 1.00000000025),
        (0.0, 0.0, 1.0000000005),
    ] {
        let ray = Ray::new(Vector3::new(x, y, 0.0), Vector3::z()).unwrap();
        let surface = scene.intersect(&ray).unwrap().coords;
        assert!((surface.z - z).abs() < 1e-15);
        assert!(scene.initial_medium_region(surface, None).is_err());
        for side in [Interior(id), Exterior] {
            assert_eq!(scene.initial_medium_region(surface, Some(side)), Ok(side));
        }
        let offset = Vector3::z() * 2e-10;
        assert_eq!(
            scene.initial_medium_region(surface + offset, None),
            Ok(Interior(id))
        );
        assert_eq!(
            scene.initial_medium_region(surface - offset, None),
            Ok(Exterior)
        );
    }
}

#[test]
fn finite_plate_uses_current_medium_until_transmission() {
    let mut scene = RenderJob::new().scene;
    let id = scene.add(slab(2.0), glass());
    let ground = scene.add(Box::new(Ground::new(0.0).unwrap()), glass());
    scene.medium = Some(absorption(0.2));
    scene
        .bind_medium_interface(
            id,
            MediumInterface {
                interior: Some(absorption(0.7)),
            },
        )
        .unwrap();
    assert_eq!(
        scene.initial_medium_region(Vector3::new(15.0, 0.0, 1.5), None),
        Ok(Exterior)
    );
    assert!(
        scene
            .initial_medium_region(Vector3::new(0.0, 10.0, 1.5), None)
            .is_err()
    );
    let mut ray = Ray::new(Vector3::z() * 3.0, -Vector3::z()).unwrap();
    let mut region = scene.initial_medium_region(ray.origin, None).unwrap();
    let mut weight = 1.0;
    for (event, expected) in [
        (DielectricEvent::Transmission, Interior(id)),
        (DielectricEvent::Reflection, Interior(id)),
        (DielectricEvent::Reflection, Interior(id)),
        (DielectricEvent::Transmission, Exterior),
    ] {
        let hit = scene.intersect(&ray).unwrap();
        assert_eq!(hit.object_id, id);
        weight *= scene
            .active_medium(region)
            .unwrap()
            .unwrap()
            .transmittance_ray(&ray, hit.distance, 550.0);
        region = scene
            .region_after_event(region, &hit, ray.direction, event)
            .unwrap();
        assert_eq!(region, expected);
        let direction = if event == DielectricEvent::Reflection {
            -ray.direction
        } else {
            ray.direction
        };
        ray = Ray::new(hit.coords, direction).unwrap();
    }
    let hit = scene.intersect(&ray).unwrap();
    assert_eq!(hit.object_id, ground);
    weight *= scene
        .active_medium(region)
        .unwrap()
        .unwrap()
        .transmittance_ray(&ray, hit.distance, 550.0);
    assert!((weight - (-2.5_f64).exp()).abs() < 1e-14); // 2 m exterior + 3 m interior
}

#[test]
fn camera_and_photon_tir_keep_the_inside_region() {
    let mut scene = RenderJob::new().scene;
    let id = scene.add(slab(2.0), glass());
    scene
        .bind_medium_interface(id, MediumInterface { interior: None })
        .unwrap();
    let ray = Ray::new(Vector3::z() * 1.5, Vector3::new(0.8, 0.0, 0.6)).unwrap();
    let region = scene.initial_medium_region(ray.origin, None).unwrap();
    let hit = scene.intersect(&ray).unwrap();
    let mut rng = StdRng::seed_from_u64(42);
    let mut untouched = rng.clone();
    let camera = glass()
        .sample(-ray.direction, hit.normal, &mut rng)
        .unwrap();
    let photon = sample_dielectric(ray.direction, -hit.normal, 1.5, 1.0, &mut rng).unwrap();
    assert_eq!(camera.event, ScatterType::SpecularReflection);
    assert_eq!(photon.event, DielectricEvent::Reflection);
    for event in [camera.event, photon.event.into()] {
        assert_eq!(
            scene.region_after_event(region, &hit, ray.direction, event),
            Ok(Interior(id))
        );
        assert!(
            scene
                .region_after_event(Exterior, &hit, ray.direction, event)
                .is_err()
        );
    }
    assert_eq!(rng.next_u64(), untouched.next_u64());
    assert!(
        scene
            .region_after_event(region, &hit, Vector3::x(), photon.event)
            .is_err()
    );
    assert!(
        scene
            .region_after_event(region, &hit, Vector3::repeat(f64::NAN), photon.event)
            .is_err()
    );
    assert!(
        scene
            .region_after_event(region, &hit, ray.direction, ScatterType::DiffuseReflection)
            .is_err()
    );
    // A front-face reflection (including when another lane transmits) stays outside.
    assert_eq!(
        scene.region_after_event(Exterior, &hit, -ray.direction, DielectricEvent::Reflection),
        Ok(Exterior)
    );
}

#[test]
fn binding_rejects_invalid_open_non_glass_and_duplicate_objects() {
    let mut scene = RenderJob::new().scene;
    let open = scene.add(Box::new(Ground::new(0.0).unwrap()), glass());
    let mirror = scene.add(
        sphere(),
        Box::new(Mirror::new(Vector3::repeat(0.9)).unwrap()),
    );
    let target = scene.add(sphere(), glass());
    let another = scene.add(slab(2.0), glass());
    for invalid in [usize::MAX, open, mirror] {
        assert!(
            scene
                .bind_medium_interface(invalid, MediumInterface { interior: None })
                .is_err()
        );
    }
    scene
        .bind_medium_interface(target, MediumInterface { interior: None })
        .unwrap();
    assert!(
        scene
            .bind_medium_interface(
                another,
                MediumInterface {
                    interior: Some(absorption(1.0))
                }
            )
            .is_err()
    );
    assert!(scene.active_medium(Interior(target)).unwrap().is_none());
}

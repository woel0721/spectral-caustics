use nalgebra::Vector3;
use serde_json::json;
use spectral_caustics::{
    config::{
        Config,
        sceneconfig::{EnvironmentMisHeuristic, ObjectConfig},
    },
    geometry::ray::Ray,
    material::dielectric::fresnel_dielectric,
    optics::{dispersion::Cauchy, iormode::IorMode, refract},
    render::{Render, framebuffer::Framebuffer},
    scene::Scene,
};

fn config(caustics: bool) -> Config {
    Config::from_json(if caustics {
        include_str!("../configs/vertical_prism_caustics.json")
    } else {
        include_str!("../configs/vertical_prism_spectral.json")
    })
    .unwrap()
}

fn identical(a: &Framebuffer, b: &Framebuffer) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    for (a, b) in a.pixels.iter().zip(&b.pixels) {
        for (a, b) in a.iter().zip(b.iter()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }
}

#[test]
fn vertical_prism_configs_bind_z_geometry_and_validate_explicit_parameters() {
    for caustics in [false, true] {
        let c = config(caustics);
        let scene = Scene::from_config(&c.scene).unwrap();
        assert_eq!(scene.prism_id, Some(0));
        assert_eq!(scene.caustics_object_id, caustics.then_some(0));
        assert!(scene.ground_checkerboard.is_none());
        assert!(matches!(
            c.scene.objects[0],
            ObjectConfig::VerticalPrism { .. }
        ));
        assert!(matches!(c.scene.objects[1], ObjectConfig::Ground { .. }));
        let json = c.to_json().unwrap();
        assert_eq!(Config::from_json(&json).unwrap(), c);
        let hit = scene
            .intersect(&Ray::new(Vector3::new(0.0, 0.0, 3.0), -Vector3::z()).unwrap())
            .unwrap();
        assert_eq!(hit.object_id, Some(0));
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
        assert!(Config::from_json(&invalid.to_string()).is_err(), "{field}");
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
    assert_eq!(
        Scene::from_config(&defaulted.scene)
            .unwrap()
            .caustics_object_id,
        None
    );
    let mut duplicate = config(true);
    duplicate
        .scene
        .objects
        .push(duplicate.scene.objects[0].clone());
    assert!(duplicate.validate().unwrap_err().contains("一个焦散物体"));
}

#[test]
fn vertical_sides_refract_the_same_ray_to_separate_spectral_ground_positions() {
    let c = config(true);
    let scene = Scene::from_config(&c.scene).unwrap();
    let light = &c.scene.light;
    let ray = Ray::new(
        Vector3::new(
            (light.aperture_min_m[0] + light.aperture_max_m[0]) * 0.5,
            (light.aperture_min_m[1] + light.aperture_max_m[1]) * 0.5,
            light.height_m,
        ),
        light.direction.into(),
    )
    .unwrap();
    let entry = scene.intersect(&ray).unwrap();
    assert_eq!(entry.normal, -Vector3::y());
    let model = Cauchy {
        a: 1.4907,
        b_um2: 0.0084,
    };
    let mut landings = Vec::new();
    for nm in [450.0, 550.0, 650.0] {
        let n = model.ior(nm).unwrap();
        let inside = Ray::new(
            entry.coords - entry.normal * 1e-5,
            refract(ray.direction, entry.normal, 1.0, n).unwrap(),
        )
        .unwrap();
        let exit = scene.intersect(&inside).unwrap();
        assert_eq!(exit.object_id, Some(0));
        assert!((exit.normal - Vector3::new(-3.0_f64.sqrt() / 2.0, 0.5, 0.0)).norm() < 1e-12);
        let outside = Ray::new(
            exit.coords + exit.normal * 1e-5,
            refract(inside.direction, -exit.normal, n, 1.0).unwrap(),
        )
        .unwrap();
        let ground = scene.intersect(&outside).unwrap();
        assert_eq!(ground.object_id, scene.ground_id);
        let landing = scene
            .trace_caustics_photon(ray, Some(nm), IorMode::Dispersion)
            .unwrap();
        let f1 = fresnel_dielectric(-ray.direction.dot(&entry.normal), 1.0, n).unwrap();
        let f2 = fresnel_dielectric(inside.direction.dot(&exit.normal), n, 1.0).unwrap();
        assert!((landing.position - ground.coords).norm() < 1e-12);
        assert!((landing.weight - (1.0 - f1) * (1.0 - f2)).abs() < 1e-14);
        assert!((0.0..=1.0).contains(&landing.weight));
        landings.push(landing.position);
        assert_eq!(
            scene.trace_caustics_photon(ray, Some(nm), IorMode::Constant),
            scene.trace_caustics_photon(ray, Some(550.0), IorMode::Constant)
        );
    }
    assert!(landings[0].x > landings[1].x && landings[1].x > landings[2].x);
    assert!((landings[0] - landings[2]).norm() > 0.1);
}

#[test]
fn striped_through_prism_spectral_render_repeats_for_threads_and_mis() {
    let mut c = config(false);
    c.render.width = 24;
    c.render.height = 24;
    c.render.camera_spp = 32;
    for heuristic in [
        EnvironmentMisHeuristic::Balance,
        EnvironmentMisHeuristic::Power,
    ] {
        c.scene.environment_mis_heuristic = heuristic;
        let scene = Scene::from_config(&c.scene).unwrap();
        let mut modes = Vec::new();
        for mode in [IorMode::Constant, IorMode::Dispersion] {
            c.render.mode = mode;
            let mut reference = None;
            for threads in [1, 2, 4, 1] {
                c.render.thread_count = Some(threads);
                let image = Render::new(c.render.clone())
                    .unwrap()
                    .render_camera(&scene)
                    .unwrap();
                assert!(image.pixels.iter().all(|p| p.iter().all(|v| v.is_finite())));
                assert!(image.pixels.iter().any(|p| p.y > 0.1));
                if let Some(previous) = &reference {
                    identical(&image, previous);
                }
                reference = Some(image);
            }
            modes.push(reference.unwrap());
        }
        assert!(
            modes[0]
                .pixels
                .iter()
                .zip(&modes[1].pixels)
                .any(|(a, b)| (a - b).norm() > 1e-6)
        );
    }
}

#[test]
fn vertical_prism_ground_caustics_are_additive_colored_and_thread_deterministic() {
    let mut c = config(true);
    c.render.width = 24;
    c.render.height = 24;
    c.render.camera_spp = 4;
    c.render.samples_per_pixel_axis = 2;
    for mode in [IorMode::Constant, IorMode::Dispersion] {
        c.render.mode = mode;
        let scene = Scene::from_config(&c.scene).unwrap();
        let mut reference = None;
        for threads in [1, 2, 4, 1] {
            c.render.thread_count = Some(threads);
            let renderer = Render::new(c.render.clone()).unwrap();
            let lighting = renderer.render_lighting(&scene).unwrap();
            let pixel_area = scene.receiver.area() / (c.render.width * c.render.height) as f64;
            let flux: f64 = lighting
                .caustics
                .pixels
                .iter()
                .map(|p| p.y * pixel_area)
                .sum();
            assert!(flux > 0.0 && flux < c.scene.light.total_flux);
            let white = lighting.sun_irradiance_xyz / lighting.sun_irradiance_xyz.y;
            let chroma = lighting
                .caustics
                .pixels
                .iter()
                .filter(|p| p.y > 1e-6)
                .map(|p| (p / p.y - white).norm())
                .fold(0.0_f64, f64::max);
            if mode == IorMode::Constant {
                assert!(chroma < 1e-12);
            } else {
                assert!(chroma > 0.1);
            }
            let images = renderer
                .render_camera_glass_components(&scene, &lighting, 4, c.render.seed)
                .unwrap();
            assert!(images.caustics.pixels.iter().any(|p| p.y > 0.0));
            for ((d, k), sum) in images
                .direct
                .pixels
                .iter()
                .zip(&images.caustics.pixels)
                .zip(&images.combined.pixels)
            {
                assert!((d + k - sum).norm() < 1e-12 * (1.0 + sum.norm()));
            }
            if let Some((light, previous)) = &reference {
                let light: &spectral_caustics::render::SpectralLighting = light;
                let previous: &spectral_caustics::render::CameraLightingImages = previous;
                identical(&lighting.caustics, &light.caustics);
                identical(&images.direct, &previous.direct);
                identical(&images.caustics, &previous.caustics);
                identical(&images.combined, &previous.combined);
            }
            reference = Some((lighting, images));
        }
    }
}

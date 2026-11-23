use crate::config::RenderJob;
use crate::material::checkerboard::Checkerboard;
// Scene construction and intersection tests.
use super::*;
use crate::config::scene::CheckerboardConfig;
use crate::config::scene::SceneConfig;
use crate::geometry::hit::Hit;
use crate::geometry::ray::PARALLEL_EPSILON;
use nalgebra::Vector2;

#[test]
fn checkerboard_uses_world_xy_floor_for_both_sides_of_the_origin() {
    let checkerboard = Checkerboard::try_from(&CheckerboardConfig::default()).unwrap();
    for (x, y, expected) in [
        (0.25, 0.25, 1.0),
        (0.75, 0.25, 0.25),
        (0.75, 0.75, 1.0),
        (-0.25, 0.25, 0.25),
        (-0.25, -0.25, 1.0),
        (0.5, 0.25, 0.25),
        (-0.5, 0.25, 0.25),
        (-0.500000001, 0.25, 1.0),
    ] {
        assert_eq!(
            checkerboard.scale_at(Vector3::new(x, y, 2.0)),
            Some(expected)
        );
    }
    for position in [
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
        Vector3::repeat(f64::MAX),
        Vector3::new((1_u64 << 53) as f64, 0.0, 0.0),
    ] {
        assert!(checkerboard.scale_at(position).is_none());
    }
    let tiny = Checkerboard::try_from(&CheckerboardConfig {
        cell_size_m: f64::MIN_POSITIVE,
        ..Default::default()
    })
    .unwrap();
    assert!(tiny.scale_at(Vector3::new(f64::MAX, 0.0, 0.0)).is_none());
}

#[test]
fn checkerboard_only_modulates_the_bound_ground() {
    let mut scene = RenderJob::from_config(&SceneConfig::default()).unwrap();
    let mut hit = Hit {
        coords: Vector3::new(0.75, 0.25, 0.0),
        object_id: scene.photon.ground_id.unwrap(),
        normal: nalgebra::Vector3::z(),
        distance: 1.0,
        material_id: 0,
    };
    assert_eq!(scene.ground_reflectance_scale(&hit), Some(0.25));
    for object_id in [scene.photon.caustics_object_id.unwrap(), 999] {
        hit.object_id = object_id;
        assert_eq!(scene.ground_reflectance_scale(&hit), Some(1.0));
    }
    hit.object_id = scene.photon.ground_id.unwrap();
    scene.ground_checkerboard = None;
    assert_eq!(scene.ground_reflectance_scale(&hit), Some(1.0));
}

#[test]
fn sky_gradient_uses_world_z_hdr_and_constant_background() {
    let mut scene = RenderJob::new();
    let sky = scene.scene.environment.as_mut().unwrap();
    sky.set_gradient(Vector3::new(1.0, 2.0, 3.0), Vector3::new(3.0, 4.0, 5.0))
        .unwrap();
    let sky = scene.scene.environment.as_ref().unwrap();
    for (direction, expected) in [
        (Vector3::z(), sky.zenith_color()),
        (Vector3::x(), sky.horizon_color()),
        (-Vector3::z(), sky.horizon_color()),
        (
            Vector3::new(3.0_f64.sqrt(), 0.0, 1.0),
            Vector3::new(2.0, 3.0, 4.0),
        ),
    ] {
        assert!((scene.scene.background_radiance(direction).unwrap() - expected).norm() < 1e-12);
        assert_eq!(
            scene.scene.background_radiance(direction),
            sky.radiance(direction)
        );
        assert!(
            (scene.scene.background_radiance(direction * 3.0).unwrap() - expected).norm() < 1e-12
        );
    }
    for direction in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::MAX),
    ] {
        assert!(scene.scene.background_radiance(direction).is_none());
    }
    scene.scene.environment = None;
    assert_eq!(
        scene.scene.background_radiance(Vector3::z()),
        Some(scene.scene.background_color)
    );
    scene.scene.background_color.x = f64::NAN;
    assert!(scene.scene.background_radiance(Vector3::z()).is_none());
}

#[test]
fn sunlight_uses_projected_window_area_and_rejects_degenerate_inputs() {
    let mut light = RenderJob::new().scene.light;
    light.as_parallel_mut().unwrap().direction = -Vector3::z();
    assert_eq!(light.sunlight(), Some((Vector3::z(), 0.5)));
    light.as_parallel_mut().unwrap().direction = Vector3::new(-3.0, 0.0, -4.0);
    let (wi, strength) = light.sunlight().unwrap();
    assert!((wi - Vector3::new(0.6, 0.0, 0.8)).norm() < 1e-12);
    assert_eq!(strength, 0.625);
    light.as_parallel_mut().unwrap().direction *= 10.0;
    assert_eq!(light.sunlight(), Some((wi, strength)));
    light.as_parallel_mut().unwrap().aperture_min_m *= 2.0;
    light.as_parallel_mut().unwrap().aperture_max_m *= 2.0;
    light.as_parallel_mut().unwrap().total_flux *= 4.0;
    assert_eq!(light.sunlight(), Some((wi, strength)));
    light.as_parallel_mut().unwrap().total_flux = 0.0;
    assert_eq!(light.sunlight(), Some((wi, 0.0)));
    for flux in [-1.0, f64::NAN, f64::INFINITY] {
        light.as_parallel_mut().unwrap().total_flux = flux;
        assert!(light.sunlight().is_none());
    }
    light.as_parallel_mut().unwrap().total_flux = 8.0;
    light.as_parallel_mut().unwrap().direction = Vector3::new(1.0, 0.0, -PARALLEL_EPSILON / 2.0);
    assert!(light.sunlight().is_none());
    light.as_parallel_mut().unwrap().direction = -Vector3::z();
    light.as_parallel_mut().unwrap().aperture_max_m = light.as_parallel().unwrap().aperture_min_m;
    assert!(light.sunlight().is_none());
    light.as_parallel_mut().unwrap().aperture_max_m = Vector2::repeat(f64::INFINITY);
    assert!(light.sunlight().is_none());
}
use crate::geometry::ground::Ground;
use crate::geometry::prism::Prism;
use crate::geometry::ray::Ray;

#[test]
fn scene_returns_nearest_hit_and_object_id_independently_of_order() {
    assert!(
        RenderJob::new()
            .scene
            .intersect(
                &Ray::new(Vector3::new(0.0, 0.0, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap()
            )
            .is_none()
    );
    for ground_first in [false, true] {
        let mut scene = RenderJob::new();
        let (prism_id, ground_id) = if ground_first {
            let ground_id = scene.scene.add(
                Box::new(Ground::new(0.0).unwrap()),
                Box::new(
                    crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                        .unwrap(),
                ),
            );
            (
                scene.scene.add(
                    Box::new(Prism::new().unwrap()),
                    Box::new(crate::material::dielectric::Dielectric::new(1.52).unwrap()),
                ),
                ground_id,
            )
        } else {
            let prism_id = scene.scene.add(
                Box::new(Prism::new().unwrap()),
                Box::new(crate::material::dielectric::Dielectric::new(1.52).unwrap()),
            );
            (
                prism_id,
                scene.scene.add(
                    Box::new(Ground::new(0.0).unwrap()),
                    Box::new(
                        crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(
                            0.8,
                        ))
                        .unwrap(),
                    ),
                ),
            )
        };
        let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
        let hit = scene.scene.intersect(&ray).unwrap();
        assert_eq!(hit.object_id, prism_id);
        assert!((hit.distance - 1.6).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(0.25, 0.2, 1.4)).norm() < 1e-12);
        let ray = Ray::new(Vector3::new(1.0, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
        let hit = scene.scene.intersect(&ray).unwrap();
        assert_eq!(hit.object_id, ground_id);
        assert_eq!(hit.distance, 3.0);
    }
}

#[test]
fn empty_and_configured_scenes_preserve_distinct_defaults_and_shared_fields() {
    let empty = RenderJob::new();
    let configured = RenderJob::from_config(&SceneConfig::default()).unwrap();
    assert!(empty.scene.objects.is_empty());
    assert_eq!(
        (empty.photon.caustics_object_id, empty.photon.ground_id),
        (None, None)
    );
    assert_eq!(configured.scene.objects.len(), 2);
    assert_eq!(
        (
            configured.photon.caustics_object_id,
            configured.photon.ground_id
        ),
        (Some(0), Some(1))
    );
    assert_eq!(
        empty.scene.background_color,
        configured.scene.background_color
    );
    assert_eq!(
        empty.scene.light.as_parallel().unwrap().direction,
        configured.scene.light.as_parallel().unwrap().direction
    );
    assert_eq!(
        empty.scene.light.as_parallel().unwrap().height_m,
        configured.scene.light.as_parallel().unwrap().height_m
    );
    assert_eq!(
        empty.scene.light.as_parallel().unwrap().aperture_min_m,
        configured.scene.light.as_parallel().unwrap().aperture_min_m
    );
    assert_eq!(
        empty.scene.light.as_parallel().unwrap().aperture_max_m,
        configured.scene.light.as_parallel().unwrap().aperture_max_m
    );
    assert_eq!(
        empty.scene.light.as_parallel().unwrap().total_flux,
        configured.scene.light.as_parallel().unwrap().total_flux
    );
    assert_eq!(
        empty.photon.receiver.min_m,
        configured.photon.receiver.min_m
    );
    assert_eq!(
        empty.photon.receiver.max_m,
        configured.photon.receiver.max_m
    );
    for direction in [Vector3::x(), Vector3::y(), Vector3::z()] {
        assert_eq!(
            empty.scene.background_radiance(direction),
            configured.scene.background_radiance(direction)
        );
    }
    let position = Vector3::new(0.75, 0.25, 0.0);
    assert_eq!(
        empty.ground_checkerboard.unwrap().scale_at(position),
        configured.ground_checkerboard.unwrap().scale_at(position)
    );
    for (x, y) in [(0, 0), (1, 1), (2, 2)] {
        let a = empty.camera.ray_for_pixel(x, y, 3, 3).unwrap();
        let b = configured.camera.ray_for_pixel(x, y, 3, 3).unwrap();
        assert_eq!(a.origin, b.origin);
        assert_eq!(a.direction, b.direction);
    }
    let mut explicit =
        crate::config::Config::from_json(include_str!("../../configs/default.json")).unwrap();
    assert!(
        explicit
            .scene
            .environment
            .as_ref()
            .unwrap()
            .map_path
            .is_some()
    );
    let hdr_path = std::env::temp_dir().join(format!(
        "scene-default-test-{}-{:?}.hdr",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(
        &hdr_path,
        b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 2\n\x80\x80\x80\x81\x80\x80\x80\x81",
    )
    .unwrap();
    explicit.scene.environment.as_mut().unwrap().map_path = Some(hdr_path.clone());
    let preset = explicit.build().unwrap();
    let _ = std::fs::remove_file(&hdr_path);
    use crate::config::scene::EnvironmentMisHeuristic::Balance;
    use crate::config::scene::EnvironmentMisHeuristic::Power;
    assert_eq!(
        (
            empty.camera_options.environment_samples,
            empty.camera_options.environment_mis_heuristic,
            empty.camera_options.max_diffuse_bounces
        ),
        (1, Balance, 0)
    );
    assert_eq!(
        (
            preset.camera_options.environment_samples,
            preset.camera_options.environment_mis_heuristic,
            preset.camera_options.max_diffuse_bounces
        ),
        (8, Power, 4)
    );
    assert!(!empty.scene.environment.as_ref().unwrap().has_map());
    assert!(preset.scene.environment.as_ref().unwrap().has_map());
}

#[test]
fn scene_validation_still_precedes_environment_loading_and_geometry_construction() {
    let mut config = SceneConfig::default();
    config.environment.as_mut().unwrap().map_path =
        Some("missing-scene-builder-regression.hdr".into());
    config.max_diffuse_bounces = crate::config::scene::MAX_DIFFUSE_BOUNCES + 1;
    assert_eq!(
        RenderJob::from_config(&config).err().unwrap().to_string(),
        "scene.max_diffuse_bounces 不能超过 1024"
    );
    config.max_diffuse_bounces = 0;
    assert!(
        RenderJob::from_config(&config)
            .err()
            .unwrap()
            .to_string()
            .contains("missing-scene-builder-regression.hdr")
    );
    config.environment = None;
    config.objects.clear();
    assert_eq!(
        RenderJob::from_config(&config).err().unwrap().to_string(),
        "场景必须包含一个 ground 接收地面"
    );
}

#[test]
fn scene_has_medium_checks_exterior_and_bound_interior_media() {
    use crate::geometry::sphere::Sphere;
    use crate::material::dielectric::Dielectric;
    use crate::medium::{HomogeneousMedium, HomogeneousMediumConfig};
    let make_medium = || {
        Box::new(
            HomogeneousMedium::try_from(&HomogeneousMediumConfig {
                min_m: [-2.0; 3],
                max_m: [2.0; 3],
                sigma_a: 0.1.into(),
                sigma_s: 0.5.into(),
                g: 0.0.into(),
            })
            .unwrap(),
        ) as Box<dyn Medium>
    };
    let mut scene = RenderJob::new().scene;
    assert!(!scene.has_medium());

    scene.medium = Some(make_medium());
    assert!(scene.has_medium());

    scene.medium = None;
    let id = scene.add(
        Box::new(Sphere::new(Vector3::zeros(), 1.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    scene
        .bind_medium_interface(id, MediumInterface { interior: None })
        .unwrap();
    assert!(!scene.has_medium());

    let mut interior_only = RenderJob::new().scene;
    let id = interior_only.add(
        Box::new(Sphere::new(Vector3::zeros(), 1.0).unwrap()),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    interior_only
        .bind_medium_interface(
            id,
            MediumInterface {
                interior: Some(make_medium()),
            },
        )
        .unwrap();
    assert!(interior_only.has_medium());
}

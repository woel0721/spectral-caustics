use nalgebra::{Vector2, Vector3};
use rand::{RngCore, SeedableRng, rngs::StdRng};
use spectral_caustics::{
    config::RenderJob,
    geometry::{ground::Ground, ray::Ray, sphere::Sphere, vertical_prism::VerticalPrism},
    material::{Material, ScatterType, dielectric::Dielectric, mirror::Mirror},
    medium::{HomogeneousMedium, HomogeneousMediumConfig, Medium},
    optics::interface::{DielectricEvent, sample_dielectric},
    scene::{
        MediumInterface,
        MediumRegion::{Exterior, Interior},
    },
};

fn glass() -> Box<Dielectric> {
    Box::new(Dielectric::new(1.5).unwrap())
}
fn sphere() -> Box<Sphere> {
    Box::new(Sphere::new(Vector3::zeros(), 1.0).unwrap())
}
fn slab() -> Box<VerticalPrism> {
    // A finite glass plate. The normal-incidence path crosses the two parallel caps.
    Box::new(
        VerticalPrism::new(
            [
                Vector2::new(-10.0, -10.0),
                Vector2::new(10.0, -10.0),
                Vector2::new(0.0, 10.0),
            ],
            1.0,
            2.0,
        )
        .unwrap(),
    )
}
fn absorption(sigma: f64) -> Box<dyn Medium> {
    Box::new(
        HomogeneousMedium::try_from(&HomogeneousMediumConfig {
            min_m: [-10.0; 3],
            max_m: [10.0; 3],
            sigma_a: sigma,
            sigma_s: 0.0,
            g: 0.0,
        })
        .unwrap(),
    )
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
            .transmittance_ray(&ray, 1.0),
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
fn finite_plate_uses_current_medium_until_transmission() {
    let mut scene = RenderJob::new().scene;
    let id = scene.add(slab(), glass());
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
            .transmittance_ray(&ray, hit.distance);
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
        .transmittance_ray(&ray, hit.distance);
    assert!((weight - (-2.5_f64).exp()).abs() < 1e-14); // 2 m exterior + 3 m interior
}

#[test]
fn camera_and_photon_tir_keep_the_inside_region() {
    let mut scene = RenderJob::new().scene;
    let id = scene.add(slab(), glass());
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
    let another = scene.add(slab(), glass());
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

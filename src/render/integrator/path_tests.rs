//! Unified path state: independent event budgets and diffuse-to-glass termination.
use super::{
    diffuse_tests::{SequenceRng, close, sun, two_planes},
    path::trace_camera_path,
    types::{CameraPathRngs, CameraPathState},
};
use crate::{
    geometry::{ray::Ray, triangle::Triangle},
    material::dielectric::Dielectric,
};
use nalgebra::Vector3;

#[test]
fn glass_prefix_and_deep_diffuse_keep_independent_counts_and_roulette_order() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    scene.max_diffuse_bounces = 5;
    scene.add_with_material(
        Box::new(
            Triangle::new(
                Vector3::new(-0.05, -0.05, 0.25),
                Vector3::new(0.05, -0.05, 0.25),
                Vector3::new(0.0, 0.05, 0.25),
            )
            .unwrap(),
        ),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let (wi, light) = sun();
    for survive in [false, true] {
        let mut words = vec![0.25, 0.0, 0.75, 0.25, 0.25, 0.0];
        if survive {
            words.extend([0.0, 0.75, 0.25, 0.0, 0.25, 0.0]);
        } else {
            words.push(0.99);
        }
        let mut camera = SequenceRng::new(&[0.9]);
        let mut environment = SequenceRng::new(&[]);
        let mut diffuse = SequenceRng::new(&words);
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 5);
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut camera,
                environment: &mut environment,
                diffuse: &mut diffuse,
            },
            wi,
            light,
            None,
        )
        .unwrap();
        assert_eq!(state.glass_events, 1);
        assert_eq!(state.diffuse_bounces, if survive { 5 } else { 3 });
        assert_eq!(camera.draws, 1);
        assert_eq!(environment.draws, 0);
        assert_eq!(diffuse.draws, if survive { 12 } else { 7 });
        assert!(
            state
                .radiance
                .direct
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
        );
        close(state.radiance.caustics, Vector3::zeros());
    }
}

#[test]
fn diffuse_to_glass_stops_without_sampling_a_glass_event() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    scene.max_diffuse_bounces = 5;
    scene.add_with_material(
        Box::new(
            Triangle::new(
                Vector3::new(-0.9, -0.3, 0.05),
                Vector3::new(-0.5, -0.3, 0.05),
                Vector3::new(-0.7, 0.3, 0.05),
            )
            .unwrap(),
        ),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let mut camera = SequenceRng::new(&[]);
    let mut environment = SequenceRng::new(&[]);
    let mut diffuse = SequenceRng::new(&[0.25, 0.0, 0.5, 0.25]);
    let mut state = CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 5);
    let (wi, light) = sun();
    trace_camera_path(
        &scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut diffuse,
        },
        wi,
        light,
        None,
    )
    .unwrap();
    assert_eq!(state.diffuse_bounces, 2);
    assert_eq!(state.glass_events, 0);
    assert_eq!(camera.draws, 0);
    assert_eq!(environment.draws, 0);
    assert_eq!(diffuse.draws, 4);
    assert!(state.radiance.direct.norm() > 0.0);
}

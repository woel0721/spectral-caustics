//! F1 compares the unified production loop with the frozen E2 control flow.
use super::{
    diffuse::{russian_roulette, sample_diffuse_ray, secondary_direct_lighting},
    diffuse_tests::{SequenceRng, bits, close, sun, two_planes, uniform_hdri},
    direct::{camera_lighting_setup, surface_sun_lighting},
    environment::environment_lighting,
    ground::{
        GroundLighting, bind_ground_lighting, bind_spectral_ground_lighting, ground_irradiance,
        lambertian_radiance,
    },
    offset_ray_origin,
    path::{trace_camera_path, trace_camera_ray},
    types::{CameraPathComponents, CameraPathRngs, CameraPathState, GroundLightingComponent},
};
use crate::{
    config::{Config, rendersetting::RenderSetting, sceneconfig::EnvironmentMisHeuristic},
    geometry::{hit::Hit, ray::Ray, triangle::Triangle},
    material::{
        BsdfValue, EvaluateMode, Material, MaterialSample, dielectric::Dielectric,
        lambertian::Lambertian, unit_vector,
    },
    render::{Render, utils::xyz_to_linear_srgb},
    scene::Scene,
};
use nalgebra::Vector3;
use rand::{RngCore, SeedableRng, rngs::StdRng};
use std::error::Error;

mod legacy_e2;

#[test]
fn unified_loop_matches_frozen_e2_rgb_bits_and_three_rngs() {
    let renderer = Render::new(RenderSetting {
        width: 7,
        height: 5,
        samples_per_pixel_axis: 2,
        ..Default::default()
    })
    .unwrap();
    // The prism scene covers glass/ground bindings; perpendicular planes exercise deep paths/RR.
    for scene_kind in 0..2 {
        let mut scene = if scene_kind == 0 {
            Scene::from_config(&Config::default().scene).unwrap()
        } else {
            let mut scene = two_planes(Vector3::repeat(0.8), Vector3::new(0.4, 0.7, 0.9));
            scene.ground_id = Some(0);
            scene
        };
        let photons = renderer.render_lighting(&scene).unwrap();
        let full = renderer.render(&scene).unwrap();
        for environment in [Some(uniform_hdri()), Scene::new().environment, None] {
            scene.environment = environment;
            for depth in [0, 1, 2, 3, 5] {
                scene.max_diffuse_bounces = depth;
                for heuristic in [
                    EnvironmentMisHeuristic::Balance,
                    EnvironmentMisHeuristic::Power,
                ] {
                    scene.environment_mis_heuristic = heuristic;
                    for samples in [1, 3, 8] {
                        scene.environment_samples = samples;
                        for binding in 0..5 {
                            let ground = match binding {
                                0 => None,
                                1 => Some(
                                    bind_ground_lighting(&scene, &full, &scene.receiver).unwrap(),
                                ),
                                _ => Some(
                                    bind_spectral_ground_lighting(
                                        &scene,
                                        &photons,
                                        [
                                            GroundLightingComponent::Direct,
                                            GroundLightingComponent::Caustics,
                                            GroundLightingComponent::Combined,
                                        ][binding - 2],
                                    )
                                    .unwrap(),
                                ),
                            };
                            let (wi, light) = camera_lighting_setup(&scene, 7, 5).unwrap();
                            let mut camera = StdRng::seed_from_u64(11);
                            let mut environment = StdRng::seed_from_u64(23);
                            let mut diffuse = StdRng::seed_from_u64(37);
                            let (mut old_camera, mut old_environment, mut old_diffuse) =
                                (camera.clone(), environment.clone(), diffuse.clone());
                            for y in 0..5 {
                                for x in 0..7 {
                                    let ray = scene.camera.ray_for_pixel(x, y, 7, 5).unwrap();
                                    let expected = legacy_e2::trace_camera_path_e2(
                                        &scene,
                                        ray,
                                        &mut CameraPathRngs {
                                            camera: &mut old_camera,
                                            environment: &mut old_environment,
                                            diffuse: &mut old_diffuse,
                                        },
                                        wi,
                                        light,
                                        ground.as_ref(),
                                    )
                                    .unwrap();
                                    let actual = trace_camera_ray(
                                        &scene,
                                        ray,
                                        &mut CameraPathRngs {
                                            camera: &mut camera,
                                            environment: &mut environment,
                                            diffuse: &mut diffuse,
                                        },
                                        wi,
                                        light,
                                        ground.as_ref(),
                                    )
                                    .unwrap();
                                    assert_eq!(
                                        bits(actual.direct),
                                        bits(expected.direct),
                                        "scene={scene_kind}, depth={depth}, {heuristic:?}, N={samples}, binding={binding}, ({x},{y})"
                                    );
                                    assert_eq!(bits(actual.caustics), bits(expected.caustics));
                                }
                            }
                            for _ in 0..8 {
                                assert_eq!(camera.next_u64(), old_camera.next_u64());
                                assert_eq!(environment.next_u64(), old_environment.next_u64());
                                assert_eq!(diffuse.next_u64(), old_diffuse.next_u64());
                            }
                        }
                    }
                }
            }
        }
    }
}

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
        let expected = legacy_e2::trace_camera_path_e2(
            &scene,
            Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(),
            &mut CameraPathRngs {
                camera: &mut SequenceRng::new(&[0.9]),
                environment: &mut SequenceRng::new(&[]),
                diffuse: &mut SequenceRng::new(&words),
            },
            wi,
            light,
            None,
        )
        .unwrap();
        assert_eq!(bits(state.radiance.direct), bits(expected.direct));
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
    let expected = legacy_e2::trace_camera_path_e2(
        &scene,
        Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(),
        &mut CameraPathRngs {
            camera: &mut SequenceRng::new(&[]),
            environment: &mut SequenceRng::new(&[]),
            diffuse: &mut SequenceRng::new(&[0.25, 0.0, 0.5, 0.25]),
        },
        wi,
        light,
        None,
    )
    .unwrap();
    assert_eq!(bits(state.radiance.direct), bits(expected.direct));
}

struct InvalidSun {
    lambertian: Lambertian,
    spectral: bool,
}
impl Material for InvalidSun {
    fn is_lambertian(&self) -> bool {
        true
    }
    fn sample(
        &self,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        rng: &mut dyn RngCore,
    ) -> Option<MaterialSample> {
        self.lambertian.sample(wo, normal, rng)
    }
    fn pdf(&self, wi: Vector3<f64>, wo: Vector3<f64>, normal: Vector3<f64>) -> f64 {
        self.lambertian.pdf(wi, wo, normal)
    }
    fn evaluate(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        mode: EvaluateMode,
    ) -> BsdfValue {
        if wi == sun().0 {
            if self.spectral {
                BsdfValue::Spectral(1.0)
            } else {
                BsdfValue::Rgb(Vector3::repeat(f64::NAN))
            }
        } else {
            self.lambertian.evaluate(wi, wo, normal, mode)
        }
    }
}

#[test]
fn secondary_error_precedes_deferred_first_surface_direct_error() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    scene.max_diffuse_bounces = 2;
    // Both sun evaluations fail differently; sampling at A remains valid.
    let geometries: Vec<_> = scene
        .objects
        .drain(..)
        .map(|object| object.geometry)
        .collect();
    for (index, geometry) in geometries.into_iter().enumerate() {
        scene.add_with_material(
            geometry,
            Box::new(InvalidSun {
                lambertian: Lambertian::new(Vector3::repeat(0.8)).unwrap(),
                spectral: index == 1,
            }),
        );
    }
    let (wi, light) = sun();
    let run = |old| {
        let mut camera = SequenceRng::new(&[]);
        let mut environment = SequenceRng::new(&[]);
        let mut diffuse = SequenceRng::new(&[0.25, 0.0]);
        let mut rngs = CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut diffuse,
        };
        let ray = Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap();
        let result = if old {
            legacy_e2::trace_camera_path_e2(&scene, ray, &mut rngs, wi, light, None)
        } else {
            trace_camera_ray(&scene, ray, &mut rngs, wi, light, None)
        };
        let error = result.err().expect("both paths must fail").to_string();
        assert_eq!(camera.draws, 0);
        assert_eq!(environment.draws, 0);
        assert_eq!(diffuse.draws, 2);
        error
    };
    let error = run(false);
    assert_eq!(error, run(true));
    assert_eq!(error, "材质 RGB 求值必须返回 RGB BSDF");
}

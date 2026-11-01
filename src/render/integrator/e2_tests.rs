//! E2 fixed-depth and E1 bit/RNG compatibility checks.
use super::*;
use super::{
    diffuse::{sample_diffuse_ray, trace_diffuse_path},
    diffuse_tests::{SequenceRng, bits, close, first_hit, sun, two_planes, uniform_hdri},
    direct::{camera_lighting_setup, surface_sun_lighting},
    environment::environment_lighting,
    ground::{
        GroundLighting, bind_ground_lighting, bind_spectral_ground_lighting,
        direct_ground_irradiance, ground_irradiance,
    },
    path::{MAX_GLASS_BOUNCES, trace_camera_path},
    types::{CameraPathComponents, CameraPathRngs, CameraPathState, DiffuseRay},
};
use crate::{
    config::{
        Config,
        rendersetting::RenderSetting,
        sceneconfig::{EnvironmentMisHeuristic, MAX_DIFFUSE_BOUNCES},
    },
    geometry::{hit::Hit, ray::Ray, triangle::Triangle},
    material::{
        BsdfValue, EvaluateMode, Material, MaterialSample, dielectric::Dielectric,
        lambertian::Lambertian, unit_vector,
    },
    render::{
        Render,
        utils::{linear_srgb_to_xyz, xyz_to_linear_srgb},
    },
    scene::Scene,
};
use nalgebra::Vector3;
use rand::{RngCore, SeedableRng, rngs::StdRng};
use std::{error::Error, f64::consts::PI};
mod legacy_e1;

#[test]
fn zero_and_one_bounce_match_frozen_e1_bits_and_all_three_rngs() {
    let mut scene = Scene::from_config(&Config::default().scene).unwrap();
    scene.environment = Some(uniform_hdri());
    let renderer = Render::new(RenderSetting {
        width: 11,
        height: 9,
        samples_per_pixel_axis: 2,
        ..Default::default()
    })
    .unwrap();
    let lighting = renderer.render_lighting(&scene).unwrap();
    let full = renderer.render(&scene).unwrap();
    for depth in [0, 1] {
        scene.max_diffuse_bounces = depth;
        for heuristic in [
            EnvironmentMisHeuristic::Balance,
            EnvironmentMisHeuristic::Power,
        ] {
            scene.environment_mis_heuristic = heuristic;
            for count in [1, 3, 8] {
                scene.environment_samples = count;
                for binding in 0..5 {
                    let ground = match binding {
                        0 => None,
                        1 => Some(bind_ground_lighting(&scene, &full, &scene.receiver).unwrap()),
                        _ => Some(
                            bind_spectral_ground_lighting(
                                &scene,
                                &lighting,
                                [
                                    GroundLightingComponent::Direct,
                                    GroundLightingComponent::Caustics,
                                    GroundLightingComponent::Combined,
                                ][binding - 2],
                            )
                            .unwrap(),
                        ),
                    };
                    let mut a = StdRng::seed_from_u64(11);
                    let mut b = StdRng::seed_from_u64(23);
                    let mut c = StdRng::seed_from_u64(37);
                    let mut old_a = a.clone();
                    let mut old_b = b.clone();
                    let mut old_c = c.clone();
                    let (wi, light) = camera_lighting_setup(&scene, 11, 9).unwrap();
                    for y in 0..9 {
                        for x in 0..11 {
                            let ray = scene.camera.ray_for_pixel(x, y, 11, 9).unwrap();
                            let old = legacy_e1::trace_camera_path_e1(
                                &scene,
                                ray,
                                &mut CameraPathRngs {
                                    camera: &mut old_a,
                                    environment: &mut old_b,
                                    diffuse: &mut old_c,
                                },
                                wi,
                                light,
                                ground.as_ref(),
                            )
                            .unwrap();
                            let new = trace_camera_path(
                                &scene,
                                ray,
                                &mut CameraPathRngs {
                                    camera: &mut a,
                                    environment: &mut b,
                                    diffuse: &mut c,
                                },
                                wi,
                                light,
                                ground.as_ref(),
                            )
                            .unwrap();
                            assert_eq!(bits(new.direct), bits(old.direct));
                            assert_eq!(bits(new.caustics), bits(old.caustics));
                        }
                    }
                    for _ in 0..8 {
                        assert_eq!(a.next_u64(), old_a.next_u64());
                        assert_eq!(b.next_u64(), old_b.next_u64());
                        assert_eq!(c.next_u64(), old_c.next_u64());
                    }
                }
            }
        }
    }
}

fn chain_words(depth: usize) -> Vec<f64> {
    (0..depth)
        .flat_map(|i| {
            if i % 2 == 0 {
                [0.25, 0.0]
            } else {
                [0.75, 0.25]
            }
        })
        .collect()
}
fn trace_chain(scene: &Scene, rng: &mut dyn RngCore) -> CameraPathComponents {
    let (wi, light) = sun();
    trace_camera_path(
        scene,
        Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(),
        &mut CameraPathRngs {
            camera: &mut SequenceRng::new(&[]),
            environment: &mut SequenceRng::new(&[]),
            diffuse: rng,
        },
        wi,
        light,
        None,
    )
    .unwrap()
}
fn analytic_chain(
    a: Vector3<f64>,
    b: Vector3<f64>,
    depth: usize,
    view_factor: f64,
) -> Vector3<f64> {
    // Two perpendicular planes have F_AB=F_BA=1/2; both see the same unoccluded sun.
    let k = 2.0 / (2.0_f64.sqrt() * PI);
    (0..=depth)
        .map(|d| {
            a.map(|v| v.powi((d / 2 + 1) as i32))
                .component_mul(&b.map(|v| v.powi(d.div_ceil(2) as i32)))
                * (k * view_factor.powi(d as i32))
        })
        .sum()
}

#[test]
fn fixed_depth_reaches_2_3_5_vertices_and_rgb_products_are_correct() {
    let a = Vector3::new(0.8, 0.5, 0.2);
    let b = Vector3::new(0.4, 0.7, 0.9);
    for depth in [2, 3, 5] {
        let mut scene = two_planes(a, b);
        scene.max_diffuse_bounces = depth;
        let mut rng = SequenceRng::new(&chain_words(depth));
        let hit = first_hit(&scene);
        let material = scene.material(hit.material_id.unwrap()).unwrap();
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), depth);
        trace_diffuse_path(&scene, &hit, material, sun(), None, &mut rng, &mut state).unwrap();
        assert_eq!(state.diffuse_bounces, depth);
        assert_eq!(state.glass_events, 0);
        assert_eq!(rng.draws, 2 * depth);
        let result = trace_chain(&scene, &mut SequenceRng::new(&chain_words(depth)));
        close(result.direct, analytic_chain(a, b, depth, 1.0));
        if depth <= 3 {
            close(
                state.throughput,
                a.map(|v| v.powi(depth.div_ceil(2) as i32))
                    .component_mul(&b.map(|v| v.powi((depth / 2) as i32))),
            );
        }
    }
}

#[test]
fn two_plane_2_3_5_bounce_brightness_matches_independent_view_factor_series() {
    let a = Vector3::new(0.8, 0.7, 0.6);
    let b = Vector3::new(0.9, 0.8, 0.7);
    for depth in [2, 3, 5] {
        let mut scene = two_planes(a, b);
        scene.max_diffuse_bounces = depth;
        let mut rng = StdRng::seed_from_u64(801);
        let mut mean = Vector3::zeros();
        let mut second = Vector3::zeros();
        let n = 32768;
        for _ in 0..n {
            let value = trace_chain(&scene, &mut rng).direct;
            mean += value / n as f64;
            second += value.component_mul(&value) / n as f64;
        }
        let reference = analytic_chain(a, b, depth, 0.5);
        for c in 0..3 {
            let se = ((second[c] - mean[c] * mean[c]) / (n - 1) as f64).sqrt();
            assert!(
                (mean[c] - reference[c]).abs() < 5.0 * se + 1e-12,
                "depth={depth}, mean={mean:?}, reference={reference:?}"
            );
        }
        assert!((reference - analytic_chain(a, b, depth - 1, 0.5)).norm() > 0.0);
    }
}

#[test]
fn deep_render_repeats_across_threads_mis_and_components_and_preserves_photons() {
    let mut scene = Scene::from_config(&Config::default().scene).unwrap();
    scene.environment = Some(uniform_hdri());
    // A red wall facing the camera gives the ground a second diffuse receiver.
    let corners = [
        Vector3::new(-1.5, -3.0, 0.0),
        Vector3::new(-1.5, 3.0, 0.0),
        Vector3::new(-1.5, 3.0, 3.0),
        Vector3::new(-1.5, -3.0, 3.0),
    ];
    for [a, b, c] in [[0, 1, 2], [0, 2, 3]] {
        scene.add_with_material(
            Box::new(Triangle::new(corners[a], corners[b], corners[c]).unwrap()),
            Box::new(Lambertian::new(Vector3::new(0.8, 0.2, 0.1)).unwrap()),
        );
    }
    let renderer = Render::new(RenderSetting {
        width: 11,
        height: 9,
        samples_per_pixel_axis: 2,
        ..Default::default()
    })
    .unwrap();
    let photons = renderer.render_lighting(&scene).unwrap();
    let baseline = render_camera_glass_components(&scene, &photons, 11, 9, 16, 42).unwrap();
    for depth in [2, 3, 5] {
        scene.max_diffuse_bounces = depth;
        let light = renderer.render_lighting(&scene).unwrap();
        assert_eq!(light.direct.pixels, photons.direct.pixels);
        assert_eq!(light.caustics.pixels, photons.caustics.pixels);
        for heuristic in [
            EnvironmentMisHeuristic::Balance,
            EnvironmentMisHeuristic::Power,
        ] {
            scene.environment_mis_heuristic = heuristic;
            scene.environment_samples = 3;
            let reference =
                render_camera_glass_components(&scene, &photons, 11, 9, 16, 42).unwrap();
            assert_eq!(reference.caustics.pixels, baseline.caustics.pixels);
            for threads in [1, 2, 4] {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap();
                for _ in 0..2 {
                    let actual = pool
                        .install(|| {
                            render_camera_glass_components(&scene, &photons, 11, 9, 16, 42)
                                .map_err(|e| e.to_string())
                        })
                        .unwrap();
                    for (a, b) in [
                        (&actual.direct, &reference.direct),
                        (&actual.caustics, &reference.caustics),
                        (&actual.combined, &reference.combined),
                    ] {
                        for (x, y) in a.pixels.iter().zip(&b.pixels) {
                            assert_eq!(bits(*x), bits(*y));
                        }
                    }
                    for ((d, c), combined) in actual
                        .direct
                        .pixels
                        .iter()
                        .zip(&actual.caustics.pixels)
                        .zip(&actual.combined.pixels)
                    {
                        close(d + c, *combined);
                    }
                }
            }
            for (component, want) in [
                (GroundLightingComponent::Direct, &reference.direct),
                (GroundLightingComponent::Caustics, &reference.caustics),
                (GroundLightingComponent::Combined, &reference.combined),
            ] {
                assert_eq!(
                    render_camera_glass_with_lighting(&scene, &photons, 11, 9, 16, 42, component)
                        .unwrap()
                        .pixels,
                    want.pixels
                );
            }
        }
    }
}

#[test]
fn bounded_depth_and_checked_work_reject_unreasonable_configurations() {
    let mut config = Config::default();
    config.scene.max_diffuse_bounces = MAX_DIFFUSE_BOUNCES;
    config.validate().unwrap();
    config.render.camera_spp = usize::MAX / 4;
    config.scene.environment_samples = 1;
    assert!(config.validate().is_err());
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.8));
    scene.max_diffuse_bounces = usize::MAX;
    assert!(render_camera_glass(&scene, 1, 1, 1, 42).is_err());
    scene.max_diffuse_bounces = MAX_DIFFUSE_BOUNCES;
    assert!(render_camera_glass(&scene, 1, 1, usize::MAX / 4, 42).is_err());
}

struct BadContinuation(usize);
impl Material for BadContinuation {
    fn is_lambertian(&self) -> bool {
        true
    }
    fn evaluate(
        &self,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: EvaluateMode,
    ) -> BsdfValue {
        match self.0 {
            2 => BsdfValue::Rgb(Vector3::repeat(-1.0)),
            3 => BsdfValue::Rgb(Vector3::repeat(f64::NAN)),
            4 => BsdfValue::Spectral(1.0),
            _ => BsdfValue::Rgb(Vector3::repeat(0.8 / PI)),
        }
    }
    fn pdf(&self, _: Vector3<f64>, _: Vector3<f64>, _: Vector3<f64>) -> f64 {
        if self.0 == 0 {
            0.0
        } else if self.0 == 1 {
            f64::NAN
        } else {
            0.5 / PI
        }
    }
    fn sample(
        &self,
        _: Vector3<f64>,
        normal: Vector3<f64>,
        _: &mut dyn RngCore,
    ) -> Option<MaterialSample> {
        match self.0 {
            7 => None,
            8 => Some(MaterialSample::Continuous { wi: -normal }),
            5 => Some(MaterialSample::Continuous {
                wi: Vector3::zeros(),
            }),
            6 => Some(MaterialSample::Continuous {
                wi: Vector3::repeat(f64::NAN),
            }),
            _ => Some(MaterialSample::Continuous { wi: normal }),
        }
    }
}
#[test]
fn invalid_and_empty_continuation_samples_at_later_vertices_are_handled() {
    for mode in 0..9 {
        let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.8));
        let geometry = scene.objects.pop().unwrap().geometry;
        scene.add_with_material(geometry, Box::new(BadContinuation(mode)));
        let hit = first_hit(&scene);
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 5);
        let result = trace_diffuse_path(
            &scene,
            &hit,
            scene.material(hit.material_id.unwrap()).unwrap(),
            (sun().0, 0.0),
            None,
            &mut SequenceRng::new(&[0.25, 0.0]),
            &mut state,
        );
        if mode < 7 {
            assert!(result.is_err(), "mode {mode}");
        } else {
            result.unwrap();
            assert_eq!(state.diffuse_bounces, 1);
        }
    }
}

struct Unsupported;
impl Material for Unsupported {
    fn evaluate(
        &self,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: EvaluateMode,
    ) -> BsdfValue {
        panic!("unsupported continuation must not be evaluated")
    }
}
#[test]
fn third_hit_glass_unbound_unsupported_and_backface_stop_without_new_glass_events() {
    for kind in 0..4 {
        let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
        scene.max_diffuse_bounces = 5;
        let mut vertices = [
            Vector3::new(-0.9, -0.3, 0.05),
            Vector3::new(-0.5, -0.3, 0.05),
            Vector3::new(-0.7, 0.3, 0.05),
        ];
        if kind == 3 {
            vertices.swap(1, 2);
        }
        let geometry = Box::new(Triangle::new(vertices[0], vertices[1], vertices[2]).unwrap());
        match kind {
            0 => {
                scene.add_with_material(geometry, Box::new(Dielectric::new(1.5).unwrap()));
            }
            1 => {
                scene.add(geometry);
            }
            2 => {
                scene.add_with_material(geometry, Box::new(Unsupported));
            }
            _ => {
                scene.add_with_material(
                    geometry,
                    Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
                );
            }
        }
        let actual = trace_chain(&scene, &mut SequenceRng::new(&[0.25, 0.0, 0.5, 0.25]));
        close(
            actual.direct,
            analytic_chain(Vector3::repeat(0.8), Vector3::repeat(0.7), 1, 1.0),
        );
    }
}

struct EscapeLambertian(Lambertian);
impl Material for EscapeLambertian {
    fn is_lambertian(&self) -> bool {
        true
    }
    fn evaluate(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        mode: EvaluateMode,
    ) -> BsdfValue {
        self.0.evaluate(wi, wo, normal, mode)
    }
    fn pdf(&self, wi: Vector3<f64>, wo: Vector3<f64>, normal: Vector3<f64>) -> f64 {
        self.0.pdf(wi, wo, normal)
    }
    fn sample(
        &self,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: &mut dyn RngCore,
    ) -> Option<MaterialSample> {
        Some(MaterialSample::Continuous { wi: sun().0 })
    }
}
struct PrefixRng {
    head: std::collections::VecDeque<u64>,
    tail: StdRng,
}
impl PrefixRng {
    fn new(values: &[f64], seed: u64) -> Self {
        Self {
            head: values
                .iter()
                .map(|v| ((v * (1_u64 << 53) as f64) as u64) << 11)
                .collect(),
            tail: StdRng::seed_from_u64(seed),
        }
    }
}
impl RngCore for PrefixRng {
    fn next_u64(&mut self) -> u64 {
        self.head
            .pop_front()
            .unwrap_or_else(|| self.tail.next_u64())
    }
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes()[..chunk.len()]);
        }
    }
}
#[test]
fn deep_escape_does_not_add_hdri_again_or_consume_new_environment_samples() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    scene.environment = Some(uniform_hdri());
    scene.environment_samples = 3;
    let geometry = scene.objects.pop().unwrap().geometry;
    scene.add_with_material(
        geometry,
        Box::new(EscapeLambertian(
            Lambertian::new(Vector3::repeat(0.7)).unwrap(),
        )),
    );
    for heuristic in [
        EnvironmentMisHeuristic::Balance,
        EnvironmentMisHeuristic::Power,
    ] {
        scene.environment_mis_heuristic = heuristic;
        let hit = first_hit(&scene);
        let mut rng = PrefixRng::new(&[0.25, 0.0], 12);
        let mut replay = PrefixRng::new(&[0.25, 0.0], 12);
        let mut one = CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 1);
        let mut deep = CameraPathState::new(one.ray, 5);
        let material = scene.material(hit.material_id.unwrap()).unwrap();
        trace_diffuse_path(
            &scene,
            &hit,
            material,
            (sun().0, 0.0),
            None,
            &mut rng,
            &mut one,
        )
        .unwrap();
        trace_diffuse_path(
            &scene,
            &hit,
            material,
            (sun().0, 0.0),
            None,
            &mut replay,
            &mut deep,
        )
        .unwrap();
        assert!(one.radiance.direct.norm() > 0.0);
        assert_eq!(bits(one.radiance.direct), bits(deep.radiance.direct));
        assert_eq!(deep.diffuse_bounces, 2);
        assert_eq!(rng.next_u64(), replay.next_u64());
    }
}

#[test]
fn deep_ground_hits_never_read_caustics_or_complete_maps() {
    use super::ground::SunGroundLighting;
    use crate::render::framebuffer::Framebuffer;
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    scene.ground_id = Some(0);
    let map = Framebuffer {
        width: 1,
        height: 1,
        pixels: vec![Vector3::repeat(f64::NAN)],
    };
    let ground = GroundLighting {
        irradiance: &map,
        receiver: &scene.receiver,
        object_id: 0,
        rho: 0.8,
        sun: Some(SunGroundLighting {
            wi: sun().0,
            irradiance_xyz: linear_srgb_to_xyz(Vector3::repeat(2.0)).unwrap(),
            component: GroundLightingComponent::Combined,
        }),
    };
    let hit = first_hit(&scene);
    let material = scene.material(hit.material_id.unwrap()).unwrap();
    for bound in [
        &ground,
        &GroundLighting {
            sun: None,
            ..ground
        },
    ] {
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 3);
        trace_diffuse_path(
            &scene,
            &hit,
            material,
            sun(),
            Some(bound),
            &mut SequenceRng::new(&chain_words(3)),
            &mut state,
        )
        .unwrap();
        assert_eq!(state.diffuse_bounces, 3);
        assert!(state.radiance.direct.norm() > 0.0);
        assert_eq!(state.radiance.caustics, Vector3::zeros());
    }
}

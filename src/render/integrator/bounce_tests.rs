//! Bounded diffuse depth, Russian Roulette and current transport invariants.
use super::*;
use super::{
    diffuse::russian_roulette,
    diffuse_tests::{SequenceRng, bits, close, first_hit, sun, two_planes, uniform_hdri},
    ground::GroundLighting,
    test_support::evaluate_path,
    types::{CameraPathComponents, CameraPathRngs, CameraPathState},
};
use crate::{
    config::{
        Config,
        rendersetting::RenderSetting,
        sceneconfig::{EnvironmentMisHeuristic, MAX_DIFFUSE_BOUNCES},
    },
    geometry::{ray::Ray, triangle::Triangle},
    material::{
        BsdfValue, EvaluateMode, Material, MaterialSample,
        lambertian::Lambertian,
    },
    render::{Render, utils::linear_srgb_to_xyz},
    scene::Scene,
};
use nalgebra::Vector3;
use rand::{RngCore, SeedableRng, rngs::StdRng};
use std::f64::consts::PI;

fn add_colored_wall(scene: &mut Scene) {
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
}

fn chain_words(depth: usize) -> Vec<f64> {
    let mut words = Vec::new();
    for i in 0..depth {
        if i >= 3 {
            words.push(0.0);
        } // RR survives before issuing bounce 4,5,…
        words.extend(if i % 2 == 0 {
            [0.25, 0.0]
        } else {
            [0.75, 0.25]
        });
    }
    words
}

fn trace_chain(scene: &Scene, rng: &mut dyn RngCore) -> CameraPathComponents {
    let (wi, light) = sun();
    evaluate_path(
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
        super::test_support::trace_continuation(
            &scene,
            &hit,
            material,
            sun(),
            None,
            &mut rng,
            &mut state,
        )
        .unwrap();
        assert_eq!(state.diffuse_bounces, depth);
        assert_eq!(state.glass_events, 0);
        assert_eq!(rng.draws, 2 * depth + depth.saturating_sub(3));
        let result = trace_chain(&scene, &mut SequenceRng::new(&chain_words(depth)));
        // RR changes individual depth-5 samples; the independent series mean is checked below.
        assert!(result.direct.norm() > analytic_chain(a, b, 1, 1.0).norm());
        if depth <= 3 {
            close(result.direct, analytic_chain(a, b, depth, 1.0));
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
    add_colored_wall(&mut scene);
    let renderer = Render::new(RenderSetting {
        width: 11,
        height: 9,
        samples_per_pixel_axis: 2,
        ..Default::default()
    })
    .unwrap();
    let photons = renderer.render_lighting(&scene).unwrap();
    let baseline = render_camera_glass_components(&scene, &photons, 11, 9, 16, 42).unwrap();
    for depth in [0, 1, 2, 3, 5] {
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
        let result = super::test_support::trace_continuation(
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
fn third_hit_unbound_unsupported_and_backface_still_terminate() {
    for kind in 1..4 {
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
        super::test_support::trace_continuation(
            &scene,
            &hit,
            material,
            (sun().0, 0.0),
            None,
            &mut rng,
            &mut one,
        )
        .unwrap();
        super::test_support::trace_continuation(
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
fn deep_ground_hits_never_read_caustics() {
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
        light_wi: sun().0,
        sun_irradiance_xyz: linear_srgb_to_xyz(Vector3::repeat(2.0)).unwrap(),
    };
    let hit = first_hit(&scene);
    let material = scene.material(hit.material_id.unwrap()).unwrap();
    {
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 3);
        super::test_support::trace_continuation(
            &scene,
            &hit,
            material,
            sun(),
            Some(&ground),
            &mut SequenceRng::new(&chain_words(3)),
            &mut state,
        )
        .unwrap();
        assert_eq!(state.diffuse_bounces, 3);
        assert!(state.radiance.direct.norm() > 0.0);
        assert_eq!(state.radiance.caustics, Vector3::zeros());
    }
}

fn roulette_state(throughput: Vector3<f64>, depth: usize, max: usize) -> CameraPathState {
    let mut state = CameraPathState::new(Ray::new(Vector3::zeros(), Vector3::z()).unwrap(), max);
    state.throughput = throughput;
    state.diffuse_bounces = depth;
    state
}
#[test]
fn roulette_compensates_rgb_and_terminates_at_the_probability_boundary() {
    let t = Vector3::new(0.2, 0.4, 0.1);
    let mut alive = roulette_state(t, 3, 5);
    alive.radiance.direct = Vector3::repeat(17.0);
    let mut rng = SequenceRng::new(&[0.39]);
    assert!(russian_roulette(&mut alive, &mut rng).unwrap());
    close(alive.throughput, t / 0.4);
    assert_eq!(rng.draws, 1);
    assert_eq!(alive.radiance.direct, Vector3::repeat(17.0));
    let mut dead = roulette_state(t, 3, 5);
    assert!(!russian_roulette(&mut dead, &mut SequenceRng::new(&[0.4])).unwrap());
    close(dead.throughput, t);
    for (t, p) in [
        (Vector3::repeat(0.001), 0.05),
        (Vector3::new(2.0, 3.0, 4.0), 0.95),
    ] {
        let mut state = roulette_state(t, 3, 5);
        assert!(russian_roulette(&mut state, &mut SequenceRng::new(&[p - 0.001])).unwrap());
        close(state.throughput, t / p);
        assert!(
            !russian_roulette(
                &mut roulette_state(t, 3, 5),
                &mut SequenceRng::new(&[p + 0.001])
            )
            .unwrap()
        );
    }
}
#[test]
fn roulette_never_draws_at_shallow_or_last_vertices_and_rejects_invalid_weights() {
    let mut unused = SequenceRng::new(&[]);
    for (depth, max) in [(0, 0), (0, 5), (1, 1), (1, 5), (2, 5), (3, 3), (5, 5)] {
        let mut state = roulette_state(Vector3::repeat(0.4), depth, max);
        assert!(russian_roulette(&mut state, &mut unused).unwrap());
        assert_eq!(state.throughput, Vector3::repeat(0.4));
    }
    assert!(!russian_roulette(&mut roulette_state(Vector3::zeros(), 3, 5), &mut unused).unwrap());
    for t in [
        Vector3::repeat(-0.1),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
    ] {
        assert!(russian_roulette(&mut roulette_state(t, 3, 5), &mut unused).is_err());
    }
    assert!(
        russian_roulette(
            &mut roulette_state(Vector3::repeat(f64::MAX), 3, 5),
            &mut SequenceRng::new(&[0.0])
        )
        .is_err()
    );
    assert_eq!(unused.draws, 0);
}
#[test]
fn roulette_mean_preserves_the_input_rgb_throughput() {
    let t = Vector3::new(0.2, 0.4, 0.1);
    let mut rng = StdRng::seed_from_u64(317);
    let mut mean = Vector3::zeros();
    let count = 32768;
    for _ in 0..count {
        let mut state = roulette_state(t, 3, 5);
        if russian_roulette(&mut state, &mut rng).unwrap() {
            mean += state.throughput / count as f64;
        }
    }
    // For X=t/p with probability p, Var(X)=t^2*(1-p)/p.
    for c in 0..3 {
        let se = t[c] * (0.6 / (0.4 * count as f64)).sqrt();
        assert!((mean[c] - t[c]).abs() < 5.0 * se);
    }
}

#[test]
fn revisiting_checkerboard_ground_applies_its_scale_once_at_each_vertex() {
    use crate::{config::sceneconfig::CheckerboardConfig, scene::Checkerboard};
    let a = Vector3::new(0.8, 0.6, 0.4);
    let b = Vector3::new(0.7, 0.8, 0.9);
    let mut scene = two_planes(a, b);
    scene.ground_id = Some(0);
    scene.ground_checkerboard = Some(
        Checkerboard::try_from(&CheckerboardConfig {
            cell_size_m: 1.0,
            even_scale: 0.3,
            odd_scale: 0.3,
        })
        .unwrap(),
    );
    for depth in [2, 3] {
        scene.max_diffuse_bounces = depth;
        close(
            trace_chain(&scene, &mut SequenceRng::new(&chain_words(depth))).direct,
            analytic_chain(a * 0.3, b, depth, 1.0),
        );
    }
}

#[test]
fn terminal_vertex_rejects_invalid_direct_bsdf_without_another_bsdf_sample() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    scene.max_diffuse_bounces = 2;
    scene.add_with_material(
        Box::new(
            Triangle::new(
                Vector3::new(-0.9, -0.3, 0.05),
                Vector3::new(-0.5, -0.3, 0.05),
                Vector3::new(-0.7, 0.3, 0.05),
            )
            .unwrap(),
        ),
        Box::new(BadContinuation(2)),
    );
    let hit = first_hit(&scene);
    let mut state = CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 2);
    let mut rng = SequenceRng::new(&[0.25, 0.0, 0.5, 0.25]);
    let result = super::test_support::trace_continuation(
        &scene,
        &hit,
        scene.material(hit.material_id.unwrap()).unwrap(),
        sun(),
        None,
        &mut rng,
        &mut state,
    );
    assert!(result.unwrap_err().to_string().contains("BSDF"));
    assert_eq!(state.diffuse_bounces, 2);
    assert_eq!(rng.draws, 4);
}

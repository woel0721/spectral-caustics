//! Lambertian sampling and independent two-plane radiometric references.
use super::*;
use super::{
    diffuse::sample_scatter_ray,
    environment::environment_lighting,
    ground::{GroundLighting, bind_spectral_ground_lighting},
    test_support::evaluate_path,
    types::{CameraPathRngs, ScatterRay},
};
use crate::{
    config::sceneconfig::EnvironmentMisHeuristic,
    geometry::{
        hit::Hit,
        ray::{INTERSECTION_EPSILON, PARALLEL_EPSILON, Ray},
    },
    material::{BsdfSample, BsdfValue, EvaluateMode, Material},
    render::{
        caustics::SpectralLighting,
        framebuffer::Framebuffer,
        utils::{linear_srgb_to_xyz, xyz_to_linear_srgb},
    },
    scene::Scene,
};
use nalgebra::Vector3;
use rand::{SeedableRng, rngs::StdRng};
use std::{error::Error, f64::consts::PI};

use crate::{
    camera::Camera,
    config::{
        cameraconfig::CameraConfig,
        sceneconfig::{CheckerboardConfig, EnvironmentConfig},
    },
    geometry::{ground::Ground, object::Hittable},
    material::{ScatterType, dielectric::Dielectric, lambertian::Lambertian},
    scene::{Checkerboard, Environment},
};
use rand::RngCore;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Real infinite planes: A at z=0 (+Z), B at x=1 (-X).
struct Plane {
    point: Vector3<f64>,
    normal: Vector3<f64>,
}
impl Hittable for Plane {
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        let dot = self.normal.dot(&ray.direction);
        if dot.abs() <= PARALLEL_EPSILON {
            return None;
        }
        let distance = (self.point - ray.origin).dot(&self.normal) / dot;
        (distance.is_finite() && distance > INTERSECTION_EPSILON).then(|| Hit {
            coords: ray.at(distance),
            normal: self.normal,
            distance,
            ..Default::default()
        })
    }
}

pub(super) fn two_planes(a: Vector3<f64>, b: Vector3<f64>) -> Scene {
    let mut scene = Scene::new();
    scene.environment = None;
    scene.ground_checkerboard = None;
    scene.max_diffuse_bounces = 1;
    scene.add_with_material(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(a).unwrap()),
    );
    scene.add_with_material(
        Box::new(Plane {
            point: Vector3::x(),
            normal: -Vector3::x(),
        }),
        Box::new(Lambertian::new(b).unwrap()),
    );
    scene.camera = Camera::from_config(&CameraConfig {
        position: [0.0, 0.0, 0.5],
        target: [0.0; 3],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees: 20.0,
    })
    .unwrap();
    scene
}
pub(super) fn first_hit(scene: &Scene) -> Hit {
    scene
        .intersect(&Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap())
        .unwrap()
}
pub(super) fn sun() -> (Vector3<f64>, f64) {
    (Vector3::new(-1.0, 0.0, 1.0).normalize(), 2.0)
}
pub(super) fn close(actual: Vector3<f64>, expected: Vector3<f64>) {
    assert!(
        (actual - expected).norm() < 2e-12,
        "{actual:?} != {expected:?}"
    );
}
pub(super) fn bits(v: Vector3<f64>) -> [u64; 3] {
    v.map(f64::to_bits).into()
}

pub(super) struct SequenceRng {
    words: VecDeque<u64>,
    pub(super) draws: usize,
}
impl SequenceRng {
    pub(super) fn new(values: &[f64]) -> Self {
        Self {
            words: values
                .iter()
                .map(|v| ((v * (1_u64 << 53) as f64) as u64) << 11)
                .collect(),
            draws: 0,
        }
    }
}
impl RngCore for SequenceRng {
    fn next_u64(&mut self) -> u64 {
        self.draws += 1;
        self.words.pop_front().expect("unexpected RNG draw")
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

// Exercise a continuation through the current path state, excluding the primary vertex.
fn continuation_radiance(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let mut state = super::types::CameraPathState::new(Ray::new(hit.coords, -wo).unwrap(), 1);
    super::test_support::trace_continuation(scene, hit, material, light, ground, rng, &mut state)?;
    Ok(state.radiance.direct)
}

fn indirect(
    scene: &Scene,
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let mut state =
        super::types::CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 1);
    let hit = first_hit(scene);
    super::test_support::trace_continuation(
        scene,
        &hit,
        scene.material(hit.material_id.unwrap()).unwrap(),
        sun(),
        ground,
        rng,
        &mut state,
    )?;
    Ok(state.radiance.direct)
}

#[test]
fn cosine_sample_rgb_weight_and_offset_are_correct() {
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let mut scene = two_planes(albedo, Vector3::repeat(0.7));
    scene.ground_id = Some(0);
    scene.ground_checkerboard = Some(
        Checkerboard::try_from(&CheckerboardConfig {
            cell_size_m: 1.0,
            even_scale: 0.25,
            odd_scale: 0.5,
        })
        .unwrap(),
    );
    let hit = first_hit(&scene);
    let mat = scene.material(hit.material_id.unwrap()).unwrap();
    let ScatterRay {
        ray, throughput: t, ..
    } = sample_scatter_ray(
        &scene,
        &hit,
        mat,
        Vector3::z(),
        &mut SequenceRng::new(&[0.25, 0.0]),
    )
    .unwrap()
    .unwrap();
    close(ray.direction, Vector3::new(0.5, 0.0, 0.75_f64.sqrt()));
    assert!(
        (mat.pdf(ray.direction, Vector3::z(), hit.normal) - 0.75_f64.sqrt() / PI).abs() < 1e-14
    );
    assert_eq!(
        mat.evaluate(ray.direction, Vector3::z(), hit.normal, EvaluateMode::Rgb),
        BsdfValue::Rgb(albedo / PI)
    );
    close(t, albedo * 0.25);
    close(ray.origin, Vector3::z() * 10.0 * INTERSECTION_EPSILON);
    assert_eq!(scene.intersect(&ray).unwrap().object_id, Some(1));
}

#[test]
fn two_plane_colored_indirect_matches_analytic_half_hemisphere_integral() {
    let a = Vector3::new(0.2, 0.5, 0.8);
    let b = Vector3::new(0.9, 0.4, 0.1);
    let scene = two_planes(a, b);
    // B has constant Lo=rho_B*2/(sqrt(2)*pi), its sun ray goes upward and misses A.
    // A's cosine-weighted hemisphere sees x>0 exactly half the time: F_AB=1/2.
    let per_hit = a.component_mul(&b) * (2.0 / (2.0_f64.sqrt() * PI));
    close(
        indirect(&scene, None, &mut SequenceRng::new(&[0.25, 0.0])).unwrap(),
        per_hit,
    );
    close(
        indirect(&scene, None, &mut SequenceRng::new(&[0.25, 0.5])).unwrap(),
        Vector3::zeros(),
    );
    let mut rng = StdRng::seed_from_u64(123);
    let mut mean = Vector3::zeros();
    for _ in 0..32768 {
        mean += indirect(&scene, None, &mut rng).unwrap() / 32768.0;
    }
    let expected = per_hit * 0.5;
    assert!(
        (mean - expected).norm() / expected.norm() < 5.0 / 32768.0_f64.sqrt(),
        "mean={mean:?}, reference={expected:?}"
    );
    // The trace itself returns A's existing direct plus exactly one transported B term.
    let ray = Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap();
    let (wi, light) = sun();
    let path = evaluate_path(
        &scene,
        ray,
        &mut CameraPathRngs {
            camera: &mut SequenceRng::new(&[]),
            environment: &mut SequenceRng::new(&[]),
            diffuse: &mut SequenceRng::new(&[0.25, 0.0]),
        },
        wi,
        light,
        None,
    )
    .unwrap();
    close(path.direct, a * (light * wi.z / PI) + per_hit);
    close(path.caustics, Vector3::zeros());
}

#[test]
fn diffuse_miss_does_not_add_hdri_background() {
    let mut scene = Scene::new();
    scene.max_diffuse_bounces = 1;
    let path = std::env::temp_dir().join(format!("e1-miss-{}.hdr", std::process::id()));
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 1\n".to_vec();
    bytes.extend_from_slice(&[128, 64, 32, 132]); // [8,4,2]
    std::fs::write(&path, bytes).unwrap();
    scene.environment = Some(
        Environment::try_from(&EnvironmentConfig {
            map_path: Some(path.clone()),
            ..Default::default()
        })
        .unwrap(),
    );
    std::fs::remove_file(path).unwrap();
    scene.add_with_material(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    // Any attempt to sample HDRI after this miss would exhaust the controlled RNG.
    close(
        indirect(&scene, None, &mut SequenceRng::new(&[0.25, 0.0])).unwrap(),
        Vector3::zeros(),
    );
    let hit = first_hit(&scene);
    let mat = scene.material(hit.material_id.unwrap()).unwrap();
    let e = environment_lighting(
        &scene,
        &hit,
        mat,
        Vector3::z(),
        &mut SequenceRng::new(&[0.0, 0.25, 0.0]),
    )
    .unwrap();
    assert!(e.norm() > 0.0);
}

struct Absorber;
impl Material for Absorber {
    fn evaluate(
        &self,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: EvaluateMode,
    ) -> BsdfValue {
        BsdfValue::Rgb(Vector3::zeros())
    }
}

#[test]
fn secondary_absorbing_unbound_and_back_faces_terminate() {
    for kind in 1..4 {
        let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
        scene.objects.pop();
        let geometry = Box::new(Plane {
            point: Vector3::x(),
            normal: if kind == 3 {
                Vector3::x()
            } else {
                -Vector3::x()
            },
        });
        match kind {
            1 => {
                scene.add_with_material(geometry, Box::new(Absorber));
            }
            2 => {
                scene.add(geometry);
            }
            _ => {
                scene.add_with_material(
                    geometry,
                    Box::new(Lambertian::new(Vector3::repeat(0.7)).unwrap()),
                );
            }
        }
        close(
            indirect(&scene, None, &mut SequenceRng::new(&[0.25, 0.0])).unwrap(),
            Vector3::zeros(),
        );
    }
}

#[test]
fn secondary_sun_shadow_and_primary_back_face_are_zero() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    // Upper plane blocks B->sun, but is beyond A->B.
    scene.add(Box::new(Ground::new(3.0).unwrap()));
    close(
        indirect(&scene, None, &mut SequenceRng::new(&[0.25, 0.0])).unwrap(),
        Vector3::zeros(),
    );
    let hit = first_hit(&scene);
    let mut rng = SequenceRng::new(&[]);
    assert!(
        sample_scatter_ray(
            &scene,
            &hit,
            scene.material(hit.material_id.unwrap()).unwrap(),
            -Vector3::z(),
            &mut rng
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(rng.draws, 0);
}

struct Controlled(Option<BsdfSample>);
impl Material for Controlled {
    fn sample(&self, _: Vector3<f64>, _: Vector3<f64>, _: &mut dyn RngCore) -> Option<BsdfSample> {
        self.0
    }
    fn evaluate(
        &self,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: EvaluateMode,
    ) -> BsdfValue {
        panic!("continuation must use sample.weight without re-evaluating BSDF");
    }
}
#[test]
fn invalid_diffuse_samples_fail_explicitly_or_terminate() {
    let scene = Scene::new();
    let hit = Hit {
        normal: Vector3::z(),
        ..Default::default()
    };
    let valid = BsdfSample {
        wi: Vector3::z(),
        weight: Vector3::new(0.2, 0.5, 0.8),
        pdf: Some(1.0 / PI),
        event: ScatterType::DiffuseReflection,
    };
    let run = |sample, hit: &Hit| {
        sample_scatter_ray(
            &scene,
            hit,
            &Controlled(sample),
            Vector3::z(),
            &mut SequenceRng::new(&[]),
        )
    };
    let result = run(Some(valid), &hit).unwrap().unwrap();
    close(result.throughput, valid.weight);
    for pdf in [
        None,
        Some(0.0),
        Some(-1.0),
        Some(f64::NAN),
        Some(f64::INFINITY),
    ] {
        assert!(run(Some(BsdfSample { pdf, ..valid }), &hit).is_err());
    }
    for weight in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(
            run(
                Some(BsdfSample {
                    weight: Vector3::repeat(weight),
                    ..valid
                }),
                &hit
            )
            .is_err()
        );
    }
    for wi in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
    ] {
        assert!(run(Some(BsdfSample { wi, ..valid }), &hit).is_err());
    }
    for wi in [-Vector3::z(), Vector3::x()] {
        assert!(
            run(Some(BsdfSample { wi, ..valid }), &hit)
                .unwrap()
                .is_none()
        );
    }
    assert!(run(None, &hit).unwrap().is_none());
    assert!(
        run(
            Some(BsdfSample {
                weight: Vector3::zeros(),
                ..valid
            }),
            &hit
        )
        .unwrap()
        .is_none()
    );
    assert!(
        run(
            Some(BsdfSample {
                event: ScatterType::SpecularReflection,
                pdf: None,
                ..valid
            }),
            &hit
        )
        .is_err()
    );
    for bad_hit in [
        Hit {
            normal: Vector3::zeros(),
            ..hit
        },
        Hit {
            coords: Vector3::repeat(f64::NAN),
            ..hit
        },
    ] {
        assert!(run(Some(valid), &bad_hit).is_err());
    }
}

#[test]
fn secondary_bound_ground_uses_sun_once_checker_once_and_no_caustics() {
    // A is a downward-facing small plane above a horizontal receiver B.
    let mut scene = Scene::new();
    scene.environment = None;
    scene.ground_checkerboard = None;
    let a = scene.add_with_material(
        Box::new(Plane {
            point: Vector3::z(),
            normal: -Vector3::z(),
        }),
        Box::new(Lambertian::new(Vector3::new(0.2, 0.5, 0.8)).unwrap()),
    );
    let b = scene.add_with_material(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.6)).unwrap()),
    );
    scene.ground_id = Some(b);
    scene.ground_checkerboard = Some(
        Checkerboard::try_from(&CheckerboardConfig {
            cell_size_m: 1.0,
            even_scale: 0.3,
            odd_scale: 0.3,
        })
        .unwrap(),
    );
    // A at z=1 must not shadow B's sun: replace A's infinite geometry with a finite triangle.
    scene.objects[a].geometry = Box::new(
        crate::geometry::triangle::Triangle::new(
            Vector3::new(-0.1, -0.1, 1.0),
            Vector3::new(0.0, 0.1, 1.0),
            Vector3::new(0.1, -0.1, 1.0),
        )
        .unwrap(),
    );
    let hit = scene
        .intersect(&Ray::new(Vector3::z() * 0.5, Vector3::z()).unwrap())
        .unwrap();
    let material = scene.material(hit.material_id.unwrap()).unwrap();
    let mut map = Framebuffer::new(1, 1);
    map.pixels[0] = Vector3::repeat(1e9);
    let sun_rgb = Vector3::new(1.2, 0.8, 0.4);
    let lighting = SpectralLighting {
        direct: Framebuffer::new(1, 1),
        caustics: map,
        receiver: scene.receiver,
        light_wi: Vector3::new(-1.0, 0.0, 1.0).normalize(),
        sun_irradiance_xyz: linear_srgb_to_xyz(sun_rgb).unwrap(),
    };
    let ground = bind_spectral_ground_lighting(&scene, &lighting).unwrap();
    let result = continuation_radiance(
        &scene,
        &hit,
        material,
        -Vector3::z(),
        sun(),
        Some(&ground),
        &mut SequenceRng::new(&[0.25, 0.0]),
    )
    .unwrap();
    // Matrix RGB/XYZ roundtrip uses the same existing conversion; no approximation to its inverse.
    let b_rgb = xyz_to_linear_srgb(
        lambertian_radiance(lighting.sun_irradiance_xyz * lighting.light_wi.z, 0.6).unwrap(),
    )
    .unwrap();
    close(
        result,
        Vector3::new(0.2, 0.5, 0.8).component_mul(&b_rgb) * 0.3,
    );
    // Poisoned caustics cannot be read on a secondary hit.
    let poison = Framebuffer {
        width: 1,
        height: 1,
        pixels: vec![Vector3::repeat(f64::NAN)],
    };
    let poisoned = GroundLighting {
        irradiance: &poison,
        ..ground
    };
    close(
        continuation_radiance(
            &scene,
            &hit,
            material,
            -Vector3::z(),
            sun(),
            Some(&poisoned),
            &mut SequenceRng::new(&[0.25, 0.0]),
        )
        .unwrap(),
        result,
    );
}

struct CountingLambertian {
    mat: Lambertian,
    count: Arc<AtomicUsize>,
}
impl Material for CountingLambertian {
    fn evaluate(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        n: Vector3<f64>,
        mode: EvaluateMode,
    ) -> BsdfValue {
        self.mat.evaluate(wi, wo, n, mode)
    }
    fn pdf(&self, wi: Vector3<f64>, wo: Vector3<f64>, n: Vector3<f64>) -> f64 {
        self.mat.pdf(wi, wo, n)
    }
    fn sample(
        &self,
        wo: Vector3<f64>,
        n: Vector3<f64>,
        rng: &mut dyn RngCore,
    ) -> Option<BsdfSample> {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.mat.sample(wo, n, rng)
    }
}
#[test]
fn no_third_diffuse_sample_is_generated() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    scene.objects.pop();
    let count = Arc::new(AtomicUsize::new(0));
    scene.add_with_material(
        Box::new(Plane {
            point: Vector3::x(),
            normal: -Vector3::x(),
        }),
        Box::new(CountingLambertian {
            mat: Lambertian::new(Vector3::repeat(0.7)).unwrap(),
            count: count.clone(),
        }),
    );
    assert!(
        indirect(&scene, None, &mut SequenceRng::new(&[0.25, 0.0]))
            .unwrap()
            .norm()
            > 0.0
    );
    assert_eq!(count.load(Ordering::Relaxed), 0);
}

pub(super) fn uniform_hdri() -> Environment {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "e1-uniform-{}-{}.hdr",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n".to_vec();
    for _ in 0..8 {
        bytes.extend_from_slice(&[128, 64, 32, 132]);
    }
    std::fs::write(&path, bytes).unwrap();
    let environment = Environment::try_from(&EnvironmentConfig {
        map_path: Some(path.clone()),
        ..Default::default()
    })
    .unwrap();
    std::fs::remove_file(path).unwrap();
    environment
}

#[test]
fn secondary_hdri_balance_and_power_converge_to_independent_two_plane_reference() {
    let a = Vector3::new(0.2, 0.5, 0.8);
    let b = Vector3::new(0.9, 0.4, 0.1);
    let sky = Vector3::new(8.0, 4.0, 2.0);
    let mut scene = two_planes(a, b);
    scene.environment = Some(uniform_hdri());
    scene.environment_samples = 3; // exercise unequal strategy counts at B
    let hit = first_hit(&scene);
    // From B exactly half its cosine hemisphere (world z>0) sees the uniform sky;
    // A sees B over half its cosine hemisphere: L_ind = rho_A*rho_B*L_sky/4.
    let expected = a.component_mul(&b).component_mul(&sky) * 0.25;
    for heuristic in [
        EnvironmentMisHeuristic::Balance,
        EnvironmentMisHeuristic::Power,
    ] {
        scene.environment_mis_heuristic = heuristic;
        let mat = scene.material(hit.material_id.unwrap()).unwrap();
        let mut rng = StdRng::seed_from_u64(197);
        let mut mean = Vector3::zeros();
        let mut second = Vector3::zeros();
        let count = 32768;
        for _ in 0..count {
            let value = continuation_radiance(
                &scene,
                &hit,
                mat,
                Vector3::z(),
                (sun().0, 0.0),
                None,
                &mut rng,
            )
            .unwrap();
            mean += value / count as f64;
            second += value.component_mul(&value) / count as f64;
        }
        let variance = second - mean.component_mul(&mean);
        for c in 0..3 {
            let standard_error = (variance[c] / (count - 1) as f64).sqrt();
            assert!(
                (mean[c] - expected[c]).abs() < 5.0 * standard_error + 1e-12,
                "{heuristic:?}: mean={mean:?}, reference={expected:?}"
            );
        }
    }
}

#[test]
fn actual_glass_before_a_multiplies_both_direct_and_rgb_indirect_once() {
    let mut scene = two_planes(Vector3::new(0.2, 0.5, 0.8), Vector3::new(0.9, 0.4, 0.1));
    // A real dielectric interface exercises Fresnel reflection and transmission.
    scene.add_with_material(
        Box::new(
            crate::geometry::triangle::Triangle::new(
                Vector3::new(-0.05, -0.05, 0.25),
                Vector3::new(0.05, -0.05, 0.25),
                Vector3::new(0.0, 0.05, 0.25),
            )
            .unwrap(),
        ),
        Box::new(Dielectric::new(1.5).unwrap()),
    );
    let ray = Ray::new(Vector3::z() * 0.1, Vector3::z()).unwrap();
    let (wi, light) = sun();
    // Upward ray reflects down with weight 1; downward ray transmits with weight 4/9.
    // The small glass triangle misses both A→B and the sun shadow rays.
    let ray_down = Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap();
    for (ray, word, weight) in [(ray_down, 0.9, 4.0 / 9.0), (ray, 0.0, 1.0)] {
        let result = evaluate_path(
            &scene,
            ray,
            &mut CameraPathRngs {
                camera: &mut SequenceRng::new(&[word]),
                environment: &mut SequenceRng::new(&[]),
                diffuse: &mut SequenceRng::new(&[0.25, 0.0]),
            },
            wi,
            light,
            None,
        )
        .unwrap();
        let a = Vector3::new(0.2, 0.5, 0.8);
        let b = Vector3::new(0.9, 0.4, 0.1);
        let local = a * (light * wi.z / PI) + a.component_mul(&b) * (light * wi.z / PI);
        close(result.direct, local * weight);
    }
}

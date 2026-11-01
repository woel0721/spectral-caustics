//! E1 regression and independent two-plane radiometric references.
use super::*;
use crate::{
    camera::Camera,
    config::{
        Config,
        cameraconfig::CameraConfig,
        rendersetting::RenderSetting,
        sceneconfig::{CheckerboardConfig, EnvironmentConfig},
    },
    geometry::{ground::Ground, object::Hittable},
    material::{GlassEvent, GlassSample, dielectric::Dielectric, lambertian::Lambertian},
    render::Render,
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

mod legacy;

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

fn two_planes(a: Vector3<f64>, b: Vector3<f64>) -> Scene {
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
fn first_hit(scene: &Scene) -> Hit {
    scene
        .intersect(&Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap())
        .unwrap()
}
fn sun() -> (Vector3<f64>, f64) {
    (Vector3::new(-1.0, 0.0, 1.0).normalize(), 2.0)
}
fn close(actual: Vector3<f64>, expected: Vector3<f64>) {
    assert!(
        (actual - expected).norm() < 2e-12,
        "{actual:?} != {expected:?}"
    );
}
fn bits(v: Vector3<f64>) -> [u64; 3] {
    v.map(f64::to_bits).into()
}

struct SequenceRng {
    words: VecDeque<u64>,
    draws: usize,
}
impl SequenceRng {
    fn new(values: &[f64]) -> Self {
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

fn indirect(
    scene: &Scene,
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let hit = first_hit(scene);
    sample_one_diffuse_indirect(
        scene,
        &hit,
        scene.material(hit.material_id.unwrap()).unwrap(),
        Vector3::z(),
        sun(),
        ground,
        rng,
    )
}

#[test]
fn disabled_e1_matches_2521356_rgb_bits_and_all_rng_states() {
    // Frozen control flow, real glass + checkerboard + all ground bindings/MIS modes.
    let mut scene = Scene::from_config(&Config::default().scene).unwrap();
    let renderer = Render::new(RenderSetting {
        width: 17,
        height: 13,
        samples_per_pixel_axis: 2,
        ..Default::default()
    })
    .unwrap();
    let lighting = renderer.render_lighting(&scene).unwrap();
    let full = renderer.render(&scene).unwrap();
    for use_hdri in [false, true] {
        if use_hdri {
            scene.environment = Some(uniform_hdri());
        }
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
                    let mut camera = StdRng::seed_from_u64(17);
                    let mut environment = StdRng::seed_from_u64(29);
                    let mut old_camera = camera.clone();
                    let mut old_environment = environment.clone();
                    let mut unused = SequenceRng::new(&[]);
                    for y in 0..9 {
                        for x in 0..11 {
                            let ray = scene.camera.ray_for_pixel(x, y, 11, 9).unwrap();
                            let (wi, light) = camera_lighting_setup(&scene, 11, 9).unwrap();
                            let old = legacy::trace_camera_path_2521356(
                                &scene,
                                ray,
                                &mut old_camera,
                                wi,
                                light,
                                ground.as_ref(),
                                &mut old_environment,
                            )
                            .unwrap();
                            let new = trace_camera_path(
                                &scene,
                                ray,
                                &mut CameraPathRngs {
                                    camera: &mut camera,
                                    environment: &mut environment,
                                    diffuse: &mut unused,
                                },
                                wi,
                                light,
                                ground.as_ref(),
                            )
                            .unwrap();
                            assert_eq!(bits(old.direct), bits(new.direct));
                            assert_eq!(bits(old.caustics), bits(new.caustics));
                        }
                    }
                    for _ in 0..8 {
                        assert_eq!(camera.next_u64(), old_camera.next_u64());
                        assert_eq!(environment.next_u64(), old_environment.next_u64());
                    }
                    assert_eq!(unused.draws, 0);
                }
            }
        }
    }
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
    let DiffuseRay { ray, throughput: t } = sample_diffuse_ray(
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
    let path = trace_camera_path(
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

struct Unsupported;
impl Material for Unsupported {
    fn evaluate(
        &self,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: EvaluateMode,
    ) -> BsdfValue {
        panic!("unsupported secondary must stop")
    }
}

#[test]
fn second_glass_unsupported_unbound_and_back_faces_terminate() {
    for kind in 0..4 {
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
            0 => {
                scene.add_with_material(geometry, Box::new(Dielectric::new(1.5).unwrap()));
            }
            1 => {
                scene.add_with_material(geometry, Box::new(Unsupported));
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
        sample_diffuse_ray(
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

struct Controlled {
    sample: Option<MaterialSample>,
    pdf: f64,
    bsdf: BsdfValue,
}
impl Material for Controlled {
    fn is_lambertian(&self) -> bool {
        true
    }
    fn sample(
        &self,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: &mut dyn RngCore,
    ) -> Option<MaterialSample> {
        self.sample
    }
    fn pdf(&self, _: Vector3<f64>, _: Vector3<f64>, _: Vector3<f64>) -> f64 {
        self.pdf
    }
    fn evaluate(
        &self,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: Vector3<f64>,
        _: EvaluateMode,
    ) -> BsdfValue {
        self.bsdf
    }
}
#[test]
fn invalid_diffuse_samples_fail_explicitly_or_terminate() {
    let scene = Scene::new();
    let hit = Hit {
        normal: Vector3::z(),
        ..Default::default()
    };
    let valid = || Controlled {
        sample: Some(MaterialSample::Continuous { wi: Vector3::z() }),
        pdf: 1.0 / PI,
        bsdf: BsdfValue::Rgb(Vector3::repeat(0.8 / PI)),
    };
    let run = |m: &Controlled, h: &Hit| {
        sample_diffuse_ray(&scene, h, m, Vector3::z(), &mut SequenceRng::new(&[]))
    };
    for pdf in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MIN_POSITIVE * 0.01] {
        let mut mat = valid();
        mat.pdf = pdf;
        if pdf > 0.0 {
            mat.bsdf = BsdfValue::Rgb(Vector3::repeat(f64::MAX));
        }
        assert!(run(&mat, &hit).is_err());
    }
    for bsdf in [
        BsdfValue::Rgb(Vector3::repeat(-1.0)),
        BsdfValue::Rgb(Vector3::repeat(f64::NAN)),
        BsdfValue::Rgb(Vector3::repeat(f64::INFINITY)),
        BsdfValue::Spectral(1.0),
    ] {
        let mut mat = valid();
        mat.bsdf = bsdf;
        assert!(run(&mat, &hit).is_err());
    }
    for wi in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
    ] {
        let mut mat = valid();
        mat.sample = Some(MaterialSample::Continuous { wi });
        assert!(run(&mat, &hit).is_err());
    }
    for wi in [-Vector3::z(), Vector3::x()] {
        let mut mat = valid();
        mat.sample = Some(MaterialSample::Continuous { wi });
        assert!(run(&mat, &hit).unwrap().is_none());
    }
    let mut mat = valid();
    mat.sample = None;
    assert!(run(&mat, &hit).unwrap().is_none());
    mat.sample = Some(MaterialSample::Glass(GlassSample {
        wi: Vector3::z(),
        event: GlassEvent::Reflection,
        probability: 1.0,
        weight: 1.0,
    }));
    assert!(run(&mat, &hit).is_err());
    mat = valid();
    mat.bsdf = BsdfValue::Rgb(Vector3::zeros());
    assert!(run(&mat, &hit).unwrap().is_none());
    assert!(
        run(
            &valid(),
            &Hit {
                normal: Vector3::zeros(),
                ..hit
            }
        )
        .is_err()
    );
    assert!(
        run(
            &valid(),
            &Hit {
                coords: Vector3::repeat(f64::NAN),
                ..hit
            }
        )
        .is_err()
    );
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
    let ground =
        bind_spectral_ground_lighting(&scene, &lighting, GroundLightingComponent::Combined)
            .unwrap();
    let result = sample_one_diffuse_indirect(
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
        sample_one_diffuse_indirect(
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
    // Legacy complete map also cannot serve as an indirect emitter; use analytical RGB sun.
    let legacy = GroundLighting {
        sun: None,
        ..poisoned
    };
    let old = sample_one_diffuse_indirect(
        &scene,
        &hit,
        material,
        -Vector3::z(),
        sun(),
        Some(&legacy),
        &mut SequenceRng::new(&[0.25, 0.0]),
    )
    .unwrap();
    close(
        old,
        Vector3::new(0.2, 0.5, 0.8) * (0.6 * sun().1 * sun().0.z / PI * 0.3),
    );
}

struct CountingLambertian {
    mat: Lambertian,
    count: Arc<AtomicUsize>,
}
impl Material for CountingLambertian {
    fn is_lambertian(&self) -> bool {
        true
    }
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
    ) -> Option<MaterialSample> {
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

fn fingerprint(f: &Framebuffer) -> u64 {
    f.pixels
        .iter()
        .flat_map(|v| v.iter())
        .flat_map(|x| x.to_bits().to_le_bytes())
        .fold(0xcbf29ce484222325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
        })
}
#[test]
fn photon_maps_match_2521356_frozen_fingerprints_with_e1_on_or_off() {
    let mut scene = Scene::from_config(&Config::default().scene).unwrap();
    let renderer = Render::new(RenderSetting {
        width: 17,
        height: 13,
        samples_per_pixel_axis: 2,
        thread_count: Some(1),
        ..Default::default()
    })
    .unwrap();
    for bounces in [0, 1] {
        scene.max_diffuse_bounces = bounces;
        let l = renderer.render_lighting(&scene).unwrap();
        assert_eq!(fingerprint(&l.direct), 0xdba9b0f3c03cb9f5);
        assert_eq!(fingerprint(&l.caustics), 0x6150cb9f280efe33);
    }
}

fn uniform_hdri() -> Environment {
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
            let value = sample_one_diffuse_indirect(
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
fn e1_replays_in_1_2_4_threads_preserves_caustics_and_single_component_views() {
    let mut scene = Scene::from_config(&Config::default().scene).unwrap();
    scene.environment = Some(uniform_hdri());
    // A second diffuse object allows genuine indirect transport in this glass scene.
    scene.add_with_material(
        Box::new(Plane {
            point: Vector3::x() * -1.5,
            normal: Vector3::x(),
        }),
        Box::new(Lambertian::new(Vector3::new(0.8, 0.2, 0.1)).unwrap()),
    );
    let renderer = Render::new(RenderSetting {
        width: 11,
        height: 9,
        samples_per_pixel_axis: 2,
        ..Default::default()
    })
    .unwrap();
    let lighting = renderer.render_lighting(&scene).unwrap();
    for heuristic in [
        EnvironmentMisHeuristic::Balance,
        EnvironmentMisHeuristic::Power,
    ] {
        scene.environment_mis_heuristic = heuristic;
        for samples in [1, 3] {
            scene.environment_samples = samples;
            scene.max_diffuse_bounces = 0;
            let old = render_camera_glass_components(&scene, &lighting, 11, 9, 16, 42).unwrap();
            scene.max_diffuse_bounces = 1;
            let reference =
                render_camera_glass_components(&scene, &lighting, 11, 9, 16, 42).unwrap();
            assert_eq!(reference.caustics.pixels, old.caustics.pixels);
            assert_ne!(reference.direct.pixels, old.direct.pixels);
            for threads in [1, 2, 4] {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap();
                for _ in 0..2 {
                    let actual = pool
                        .install(|| {
                            render_camera_glass_components(&scene, &lighting, 11, 9, 16, 42)
                                .map_err(|e| e.to_string())
                        })
                        .unwrap();
                    for (got, want) in [
                        (&actual.direct, &reference.direct),
                        (&actual.caustics, &reference.caustics),
                        (&actual.combined, &reference.combined),
                    ] {
                        for (g, w) in got.pixels.iter().zip(&want.pixels) {
                            assert_eq!(bits(*g), bits(*w));
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
                let single =
                    render_camera_glass_with_lighting(&scene, &lighting, 11, 9, 16, 42, component)
                        .unwrap();
                assert_eq!(single.pixels, want.pixels);
            }
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
        let result = trace_camera_path(
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

use crate::integrator::camera::path::sample_scatter_ray;
// Lambertian sampling and independent two-plane radiometric references.
use super::super::ground::GroundLighting;
use super::super::ground::bind_spectral_ground_lighting;
use super::super::lighting::environment::environment_lighting;
use super::super::types::CameraPathRngs;
use super::super::types::ScatterRay;
use super::support::{
    Plane, SequenceRng, close, evaluate_path, first_hit, sun, two_planes, uniform_hdri,
};
use crate::caustics::SpectralLighting;
use crate::color::linear_srgb_to_xyz;
use crate::color::xyz_to_linear_srgb;
use crate::config::RenderJob;
use crate::config::scene::EnvironmentMisHeuristic;
use crate::film::Framebuffer;
use crate::geometry::hit::Hit;
use crate::geometry::ray::INTERSECTION_EPSILON;
use crate::geometry::ray::Ray;
use crate::material::BsdfSample;
use crate::material::Material;
use nalgebra::Vector3;
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::error::Error;
use std::f64::consts::PI;

use crate::config::scene::CheckerboardConfig;
use crate::config::scene::EnvironmentConfig;
use crate::environment::Environment;
use crate::geometry::ground::Ground;
use crate::material::ScatterType;
use crate::material::checkerboard::Checkerboard;
use crate::material::dielectric::Dielectric;
use crate::material::lambertian::Lambertian;
use rand::RngCore;

// Exercise a continuation through the current path state, excluding the primary vertex.
fn continuation_radiance(
    scene: &RenderJob,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn RngCore,
) -> Result<Vector3<f64>, Box<dyn Error + Send + Sync>> {
    let mut state =
        super::super::types::CameraPathState::new(Ray::new(hit.coords, -wo).unwrap(), 1);
    super::support::trace_continuation(scene, hit, material, light, ground, rng, &mut state)?;
    Ok(state.radiance.direct)
}

fn indirect(
    scene: &RenderJob,
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn RngCore,
) -> Result<Vector3<f64>, Box<dyn Error + Send + Sync>> {
    let mut state = super::super::types::CameraPathState::new(
        Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(),
        1,
    );
    let hit = first_hit(scene);
    super::support::trace_continuation(
        scene,
        &hit,
        scene.scene.material(hit.material_id),
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
    scene.photon.ground_id = Some(0);
    scene.ground_checkerboard = Some(
        Checkerboard::try_from(&CheckerboardConfig {
            cell_size_m: 1.0,
            even_scale: 0.25,
            odd_scale: 0.5,
        })
        .unwrap(),
    );
    let hit = first_hit(&scene);
    let mat = scene.scene.material(hit.material_id);
    let ScatterRay {
        ray, throughput: t, ..
    } = sample_scatter_ray(
        &scene,
        &hit,
        mat,
        Vector3::z(),
        crate::integrator::camera::transport::Rgb::default(),
        crate::optics::dispersion::IorMode::Constant,
        &mut SequenceRng::new(&[0.25, 0.0]),
    )
    .unwrap()
    .unwrap();
    close(ray.direction, Vector3::new(0.5, 0.0, 0.75_f64.sqrt()));
    assert!(
        (mat.pdf(ray.direction, Vector3::z(), hit.normal) - 0.75_f64.sqrt() / PI).abs() < 1e-14
    );
    assert_eq!(
        mat.evaluate_rgb(ray.direction, Vector3::z(), hit.normal),
        (albedo / PI)
    );
    close(t, albedo * 0.25);
    close(ray.origin, Vector3::z() * 10.0 * INTERSECTION_EPSILON);
    assert_eq!(scene.scene.intersect(&ray).unwrap().object_id, 1);
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
    let samples = 8192;
    let mut mean = Vector3::zeros();
    for _ in 0..samples {
        mean += indirect(&scene, None, &mut rng).unwrap() / samples as f64;
    }
    let expected = per_hit * 0.5;
    assert!(
        (mean - expected).norm() / expected.norm() < 5.0 / (samples as f64).sqrt(),
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
            medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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
    let mut scene = RenderJob::new();
    scene.camera_options.max_diffuse_bounces = 1;
    let hdr = super::support::TemporaryHdr::from_rgbe("e1-miss", 1, 1, &[[128, 64, 32, 132]]); // [8,4,2]
    scene.scene.environment = Some(
        Environment::try_from(&EnvironmentConfig {
            map_path: Some(hdr.0.clone()),
            ..Default::default()
        })
        .unwrap(),
    );
    drop(hdr);
    scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.8)).unwrap()),
    );
    // Any attempt to sample HDRI after this miss would exhaust the controlled RNG.
    close(
        indirect(&scene, None, &mut SequenceRng::new(&[0.25, 0.0])).unwrap(),
        Vector3::zeros(),
    );
    let hit = first_hit(&scene);
    let mat = scene.scene.material(hit.material_id);
    let e = environment_lighting(
        &scene,
        &hit,
        mat,
        Vector3::z(),
        &mut SequenceRng::new(&[0.0, 0.25, 0.0]),
        crate::integrator::camera::transport::Rgb::default(),
        crate::optics::dispersion::IorMode::Constant,
        scene.scene.medium.as_deref(),
    )
    .unwrap();
    assert!(e.norm() > 0.0);
}

#[test]
fn secondary_sun_shadow_and_primary_back_face_are_zero() {
    let mut scene = two_planes(Vector3::repeat(0.8), Vector3::repeat(0.7));
    // Upper plane blocks B->sun, but is beyond A->B.
    scene.scene.add(
        Box::new(Ground::new(3.0).unwrap()),
        Box::new(
            crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8)).unwrap(),
        ),
    );
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
            scene.scene.material(hit.material_id),
            -Vector3::z(),
            crate::integrator::camera::transport::Rgb::default(),
            crate::optics::dispersion::IorMode::Constant,
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
    fn evaluate_rgb(&self, _: Vector3<f64>, _: Vector3<f64>, _: Vector3<f64>) -> Vector3<f64> {
        panic!("continuation must use sample.weight without re-evaluating BSDF");
    }
}
#[test]
fn invalid_diffuse_samples_fail_explicitly_or_terminate() {
    let scene = RenderJob::new();
    let hit = Hit {
        normal: Vector3::z(),
        coords: nalgebra::Vector3::zeros(),
        distance: 1.0,
        object_id: 0,
        material_id: 0,
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
            crate::integrator::camera::transport::Rgb::default(),
            crate::optics::dispersion::IorMode::Constant,
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
    let mut scene = RenderJob::new();
    scene.scene.environment = None;
    scene.ground_checkerboard = None;
    let a = scene.scene.add(
        Box::new(Plane {
            point: Vector3::z(),
            normal: -Vector3::z(),
        }),
        Box::new(Lambertian::new(Vector3::new(0.2, 0.5, 0.8)).unwrap()),
    );
    let b = scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(Vector3::repeat(0.6)).unwrap()),
    );
    scene.photon.ground_id = Some(b);
    scene.ground_checkerboard = Some(
        Checkerboard::try_from(&CheckerboardConfig {
            cell_size_m: 1.0,
            even_scale: 0.3,
            odd_scale: 0.3,
        })
        .unwrap(),
    );
    // A at z=1 must not shadow B's sun: replace A's infinite geometry with a finite triangle.
    scene.scene.objects[a].geometry = Box::new(
        crate::geometry::triangle::Triangle::new(
            Vector3::new(-0.1, -0.1, 1.0),
            Vector3::new(0.0, 0.1, 1.0),
            Vector3::new(0.1, -0.1, 1.0),
        )
        .unwrap(),
    );
    let hit = scene
        .scene
        .intersect(&Ray::new(Vector3::z() * 0.5, Vector3::z()).unwrap())
        .unwrap();
    let material = scene.scene.material(hit.material_id);
    let mut map = Framebuffer::new(1, 1).unwrap();
    map.pixels[0] = Vector3::repeat(1e9);
    let sun_rgb = Vector3::new(1.2, 0.8, 0.4);
    let lighting = SpectralLighting {
        direct: Some(Framebuffer::new(1, 1).unwrap()),
        caustics: map,
        receiver: scene.photon.receiver,
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
    let b_rgb =
        xyz_to_linear_srgb(lighting.sun_irradiance_xyz * (lighting.light_wi.z * 0.6 / PI)).unwrap();
    close(
        result,
        Vector3::new(0.2, 0.5, 0.8).component_mul(&b_rgb) * 0.3,
    );
    // Removing the bright map leaves secondary lighting identical.
    let dark_lighting = SpectralLighting {
        caustics: Framebuffer::new(1, 1).unwrap(),
        ..lighting
    };
    let dark_ground = bind_spectral_ground_lighting(&scene, &dark_lighting).unwrap();
    close(
        continuation_radiance(
            &scene,
            &hit,
            material,
            -Vector3::z(),
            sun(),
            Some(&dark_ground),
            &mut SequenceRng::new(&[0.25, 0.0]),
        )
        .unwrap(),
        result,
    );
}

#[test]
fn secondary_hdri_balance_and_power_converge_to_independent_two_plane_reference() {
    let a = Vector3::new(0.2, 0.5, 0.8);
    let b = Vector3::new(0.9, 0.4, 0.1);
    let sky = Vector3::new(8.0, 4.0, 2.0);
    let mut scene = two_planes(a, b);
    scene.scene.environment = Some(uniform_hdri());
    scene.camera_options.environment_samples = 3; // exercise unequal strategy counts at B
    let hit = first_hit(&scene);
    // From B exactly half its cosine hemisphere (world z>0) sees the uniform sky;
    // A sees B over half its cosine hemisphere: L_ind = rho_A*rho_B*L_sky/4.
    let expected = a.component_mul(&b).component_mul(&sky) * 0.25;
    for heuristic in [
        EnvironmentMisHeuristic::Balance,
        EnvironmentMisHeuristic::Power,
    ] {
        scene.camera_options.environment_mis_heuristic = heuristic;
        let mat = scene.scene.material(hit.material_id);
        let mut rng = StdRng::seed_from_u64(197);
        let mut mean = Vector3::zeros();
        let mut second = Vector3::zeros();
        let count = 8192;
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
    scene.scene.add(
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
                medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
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

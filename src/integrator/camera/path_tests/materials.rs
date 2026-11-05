use super::*;
use crate::config::RenderJob;
use crate::geometry::ground::Ground;
use crate::integrator::camera::{
    path::trace_camera_path,
    transport::Caustics,
    types::{CameraPathRngs, CameraPathState},
};
use crate::material::BsdfSample;
use crate::material::Material;
use crate::material::ScatterType;
use crate::material::dielectric::Dielectric;
use crate::material::ggx_metal::GgxMetal;
use crate::material::lambertian::Lambertian;
use crate::material::mirror::Mirror;
use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;

fn metal() -> GgxMetal {
    GgxMetal::new(
        Vector3::new(0.2, 0.8, 1.5),
        Vector3::new(3.0, 2.0, 1.0),
        0.5,
    )
    .unwrap()
}
fn constant_sky(scene: &mut RenderJob, color: Vector3<f64>) {
    let sky = scene.scene.environment.as_mut().unwrap();
    sky.set_gradient(color, color).unwrap();
}
#[test]
fn glossy_escape_is_already_estimated_by_nee_and_is_not_added_again() {
    let sky = Vector3::new(1.0, 2.0, 3.0);
    let mut scene = RenderJob::new();
    constant_sky(&mut scene, sky);
    scene
        .scene
        .add(Box::new(Ground::new(0.0).unwrap()), Box::new(metal()));
    let mut reference = None;
    for depth in [0, 1, 4] {
        scene.camera_options.max_diffuse_bounces = depth;
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), depth);
        let (mut camera, mut environment, mut continuation) = (
            SequenceRng::new(&[]),
            SequenceRng::new(&[0.0, 0.0]),
            SequenceRng::new(&[0.0, 0.0]),
        );
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut camera,
                environment: &mut environment,
                diffuse: &mut continuation,
            },
            sun().0,
            0.0,
            (None).map_or(Caustics::None, Caustics::Rgb),
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        if let Some(expected) = reference {
            close(state.radiance.direct, expected);
        } else {
            reference = Some(state.radiance.direct);
        }
        assert!(state.radiance.direct.norm() > 0.0);
        assert_eq!(continuation.draws, if depth == 0 { 0 } else { 2 });
        if depth > 0 {
            assert_eq!(state.last_scatter, Some(ScatterType::GlossyReflection));
        }
    }
}
#[test]
fn actual_ggx_mirror_dielectric_chain_shades_a_lambertian_terminal_vertex() {
    let mut scene = RenderJob::new();
    scene.scene.environment = None;
    let m = metal();
    let wo = Vector3::z();
    let f = m
        .sample(wo, wo, &mut SequenceRng::new(&[0.0, 0.0]))
        .unwrap()
        .weight;
    let r = Vector3::new(0.9, 0.6, 0.3);
    let albedo = Vector3::new(0.2, 0.5, 0.8);
    let (light_wi, light) = sun();
    let first_bsdf = m.evaluate_rgb(light_wi, wo, wo);
    let first_direct = first_bsdf * (light * light_wi.z);
    scene
        .scene
        .add(Box::new(Ground::new(0.0).unwrap()), Box::new(m));
    let center = Vector3::z();
    let v = (Vector3::x() + Vector3::z()).normalize() * 0.2;
    scene.scene.add(
        Box::new(
            Triangle::new(
                center - Vector3::y() * 0.2 - v,
                center + Vector3::y() * 0.2 - v,
                center + v,
            )
            .unwrap(),
        ),
        Box::new(Mirror::new(r).unwrap()),
    );
    for (x, outward) in [(0.3, false), (0.4, true)] {
        scene.scene.add(
            Box::new(glass_patch(x, 1.0, outward)),
            Box::new(Dielectric::new(1.5).unwrap()),
        );
    }
    scene.scene.add(
        Box::new(glass_patch(1.0, 1.0, false)),
        Box::new(Lambertian::new(albedo).unwrap()),
    );
    let mut state = CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 1);
    let (mut camera, mut environment, mut continuation) = (
        SequenceRng::new(&[]),
        SequenceRng::new(&[]),
        SequenceRng::new(&[0.0, 0.0, 0.99, 0.99]),
    );
    trace_camera_path(
        &scene,
        &mut state,
        &mut CameraPathRngs {
            camera: &mut camera,
            environment: &mut environment,
            diffuse: &mut continuation,
        },
        light_wi,
        light,
        (None).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
    .unwrap();
    assert_eq!(
        (
            state.path_depth,
            state.non_delta_bounces,
            state.delta_events()
        ),
        (4, 1, 3)
    );
    close(state.throughput, f.component_mul(&r)); // two eta² factors cancel.
    let terminal = albedo * (light * (-light_wi.x) / std::f64::consts::PI);
    close(
        state.radiance.direct,
        first_direct + f.component_mul(&r).component_mul(&terminal),
    );
    close(state.radiance.caustics, Vector3::zeros());
    assert_eq!(
        (camera.draws, environment.draws, continuation.draws),
        (0, 0, 4)
    );
}
#[test]
fn repeated_ggx_bounces_use_rr_once_and_compensate_rgb() {
    let mut scene = RenderJob::new();
    scene.scene.environment = None;
    scene.camera_options.max_diffuse_bounces = 4;
    let m = metal();
    let f = m
        .sample(
            Vector3::z(),
            Vector3::z(),
            &mut SequenceRng::new(&[0.0, 0.0]),
        )
        .unwrap()
        .weight;
    scene
        .scene
        .add(Box::new(Ground::new(0.0).unwrap()), Box::new(m.clone()));
    scene.scene.add(
        Box::new(
            Triangle::new(
                Vector3::new(-10.0, -10.0, 1.0),
                Vector3::new(0.0, 10.0, 1.0),
                Vector3::new(10.0, -10.0, 1.0),
            )
            .unwrap(),
        ),
        Box::new(m),
    );
    for survive in [false, true] {
        let mut words = vec![0.0; 6];
        words.push(if survive { 0.0 } else { 0.999 });
        if survive {
            words.extend([0.0, 0.0]);
        }
        let (mut camera, mut environment, mut continuation) = (
            SequenceRng::new(&[]),
            SequenceRng::new(&[]),
            SequenceRng::new(&words),
        );
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 4);
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut camera,
                environment: &mut environment,
                diffuse: &mut continuation,
            },
            sun().0,
            0.0,
            (None).map_or(Caustics::None, Caustics::Rgb),
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        let p = f.map(|v| v.powi(3)).max().clamp(0.05, 0.95);
        close(
            state.throughput,
            if survive {
                f.map(|v| v.powi(4)) / p
            } else {
                f.map(|v| v.powi(3))
            },
        );
        assert_eq!(state.path_depth, if survive { 4 } else { 3 });
        assert_eq!(state.last_scatter, Some(ScatterType::GlossyReflection));
        assert_eq!(continuation.draws, if survive { 9 } else { 7 });
    }
}

/// Test-only 50/50 continuous + delta material: full selection probability in pdf,
/// both branch weights already compensate the branch probability.
struct Mixed {
    diffuse: Lambertian,
    mirror: Mirror,
}
impl Mixed {
    fn new() -> Self {
        Self {
            diffuse: Lambertian::new(Vector3::new(0.2, 0.5, 0.8)).unwrap(),
            mirror: Mirror::new(Vector3::new(0.9, 0.6, 0.3)).unwrap(),
        }
    }
}
impl Material for Mixed {
    fn evaluate_rgb(&self, wi: Vector3<f64>, wo: Vector3<f64>, n: Vector3<f64>) -> Vector3<f64> {
        self.diffuse.evaluate_rgb(wi, wo, n) * 0.5
    }
    fn pdf(&self, wi: Vector3<f64>, wo: Vector3<f64>, n: Vector3<f64>) -> f64 {
        0.5 * self.diffuse.pdf(wi, wo, n)
    }
    fn has_delta(&self) -> bool {
        true
    }
    fn sample(
        &self,
        wo: Vector3<f64>,
        n: Vector3<f64>,
        rng: &mut dyn rand::RngCore,
    ) -> Option<BsdfSample> {
        if rng.random::<f64>() < 0.5 {
            self.mirror.sample(wo, n, rng)
        } else {
            let mut sample = self.diffuse.sample(wo, n, rng)?;
            sample.pdf = sample.pdf.map(|p| p * 0.5);
            Some(sample)
        }
    }
}
#[test]
fn mixed_delta_nee_quota_is_null_and_continuation_adds_delta_escape_once() {
    let sky = Vector3::new(1.0, 2.0, 3.0);
    let mut scene = RenderJob::new();
    constant_sky(&mut scene, sky);
    scene
        .scene
        .add(Box::new(Ground::new(0.0).unwrap()), Box::new(Mixed::new()));
    for choose_delta in [true, false] {
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 0);
        let words = if choose_delta {
            vec![0.0]
        } else {
            vec![0.9, 0.25, 0.25]
        };
        let (mut camera, mut environment, mut continuation) = (
            SequenceRng::new(&[]),
            SequenceRng::new(&[0.0]),
            SequenceRng::new(&words),
        );
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut camera,
                environment: &mut environment,
                diffuse: &mut continuation,
            },
            sun().0,
            0.0,
            (None).map_or(Caustics::None, Caustics::Rgb),
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        assert_eq!(environment.draws, 1); // no replacement for the delta NEE sample.
        assert_eq!(continuation.draws, words.len()); // no replacement for exhausted continuous budget.
        close(
            state.radiance.direct,
            if choose_delta {
                Vector3::new(0.9, 0.6, 0.3).component_mul(&sky)
            } else {
                Vector3::zeros()
            },
        );
    }
}
#[test]
fn mixed_event_mean_matches_half_diffuse_plus_half_mirror_without_double_environment() {
    let sky = Vector3::new(1.0, 2.0, 3.0);
    let mut scene = RenderJob::new();
    scene.camera_options.max_diffuse_bounces = 1;
    constant_sky(&mut scene, sky);
    scene
        .scene
        .add(Box::new(Ground::new(0.0).unwrap()), Box::new(Mixed::new()));
    let (mut camera, mut environment, mut continuation) = (
        StdRng::seed_from_u64(1),
        StdRng::seed_from_u64(2),
        StdRng::seed_from_u64(3),
    );
    let mut sum = Vector3::zeros();
    for _ in 0..16384 {
        let mut state =
            CameraPathState::new(Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap(), 1);
        trace_camera_path(
            &scene,
            &mut state,
            &mut CameraPathRngs {
                camera: &mut camera,
                environment: &mut environment,
                diffuse: &mut continuation,
            },
            sun().0,
            0.0,
            (None).map_or(Caustics::None, Caustics::Rgb),
            crate::optics::dispersion::IorMode::Constant,
            #[cfg(feature = "diagnostics")]
            None,
        )
        .unwrap();
        sum += state.radiance.direct;
    }
    let expected =
        (Vector3::new(0.2, 0.5, 0.8) + Vector3::new(0.9, 0.6, 0.3)).component_mul(&sky) * 0.5;
    assert!((sum / 16384.0 - expected).norm() < 0.03);
}

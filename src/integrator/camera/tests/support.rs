//! 共享场景、确定性随机数及当前路径实现的调用辅助函数。

use super::super::ground::GroundLighting;
use super::super::types::{CameraPathComponents, CameraPathRngs, CameraPathState};
use crate::camera::Camera;
use crate::config::RenderJob;
use crate::config::camera::CameraConfig;
use crate::config::scene::EnvironmentConfig;
use crate::environment::Environment;
use crate::geometry::ground::Ground;
use crate::geometry::hit::{GeometryHit, Hit};
use crate::geometry::object::Hittable;
use crate::geometry::ray::{INTERSECTION_EPSILON, PARALLEL_EPSILON, Ray};
use crate::integrator::camera::transport::Caustics;
use crate::material::Material;
use crate::material::lambertian::Lambertian;
use image::Rgb;
use image::codecs::hdr::HdrEncoder;
use nalgebra::Vector3;
use rand::RngCore;
use std::collections::VecDeque;
use std::error::Error;
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Half-plane blocker covering `y > 0` on a horizontal `Ground` plane.
pub(super) struct HalfPlane(pub(super) Ground);
impl Hittable for HalfPlane {
    fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
        let hit = self.0.intersect(ray)?;
        (hit.coords.y > 0.0).then_some(hit)
    }
}

/// Purely absorbing material (zero BRDF, no scatter samples).
pub(super) struct Absorber;
impl Material for Absorber {
    fn evaluate_rgb(&self, _: Vector3<f64>, _: Vector3<f64>, _: Vector3<f64>) -> Vector3<f64> {
        Vector3::zeros()
    }
}

/// RAII temporary `.hdr` file deleted automatically on drop (including panics).
pub(super) struct TemporaryHdr(pub(super) PathBuf);

impl TemporaryHdr {
    fn unique_path(prefix: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{stamp}-{}.hdr",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    pub(super) fn new(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> Self {
        let path = Self::unique_path("environment-test");
        let pixels: Vec<_> = pixels.into_iter().map(Rgb).collect();
        HdrEncoder::new(File::create_new(&path).unwrap())
            .encode(&pixels, width, height)
            .unwrap();
        Self(path)
    }

    pub(super) fn from_rgbe(prefix: &str, width: usize, height: usize, pixels: &[[u8; 4]]) -> Self {
        let path = Self::unique_path(prefix);
        let mut bytes =
            format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes();
        for pixel in pixels {
            bytes.extend_from_slice(pixel);
        }
        fs::write(&path, bytes).unwrap();
        Self(path)
    }

    pub(super) fn uniform(rgb: [f32; 3]) -> Self {
        Self::new(4, 2, vec![rgb; 8])
    }

    pub(super) fn high_contrast() -> Self {
        let mut pixels = vec![[0.125; 3]; 64 * 32];
        // 上半球约 0.22% 球面积的 2x2 高亮块；其余区域保持暗灰。
        for row in 7..9 {
            for col in 31..33 {
                pixels[row * 64 + col] = [1024.0; 3];
            }
        }
        Self::new(64, 32, pixels)
    }

    pub(super) fn scene(&self, rotation_degrees: f64) -> RenderJob {
        let mut scene = RenderJob::new();
        scene.scene.environment = Some(
            Environment::try_from(&EnvironmentConfig {
                map_path: Some(self.0.clone()),
                rotation_degrees,
                ..Default::default()
            })
            .unwrap(),
        );
        scene
    }
}

impl Drop for TemporaryHdr {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Real infinite planes: A at z=0 (+Z), B at x=1 (-X).
pub(super) struct Plane {
    pub(super) point: Vector3<f64>,
    pub(super) normal: Vector3<f64>,
}
impl Hittable for Plane {
    fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
        let dot = self.normal.dot(&ray.direction);
        if dot.abs() <= PARALLEL_EPSILON {
            return None;
        }
        let distance = (self.point - ray.origin).dot(&self.normal) / dot;
        (distance.is_finite() && distance > INTERSECTION_EPSILON).then(|| GeometryHit {
            coords: ray.at(distance),
            normal: self.normal,
            distance,
        })
    }
}

pub(crate) fn two_planes(a: Vector3<f64>, b: Vector3<f64>) -> RenderJob {
    let mut scene = RenderJob::new();
    scene.scene.environment = None;
    scene.ground_checkerboard = None;
    scene.camera_options.max_diffuse_bounces = 1;
    scene.scene.add(
        Box::new(Ground::new(0.0).unwrap()),
        Box::new(Lambertian::new(a).unwrap()),
    );
    scene.scene.add(
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
pub(crate) fn first_hit(scene: &RenderJob) -> Hit {
    scene
        .scene
        .intersect(&Ray::new(Vector3::z() * 0.5, -Vector3::z()).unwrap())
        .unwrap()
}
pub(crate) fn sun() -> (Vector3<f64>, f64) {
    (Vector3::new(-1.0, 0.0, 1.0).normalize(), 2.0)
}
pub(crate) fn close(actual: Vector3<f64>, expected: Vector3<f64>) {
    assert!(
        (actual - expected).norm() < 2e-12,
        "{actual:?} != {expected:?}"
    );
}
pub(crate) fn bits(v: Vector3<f64>) -> [u64; 3] {
    v.map(f64::to_bits).into()
}

pub(crate) struct SequenceRng {
    words: VecDeque<u64>,
    pub(crate) draws: usize,
}
impl SequenceRng {
    pub(crate) fn new(values: &[f64]) -> Self {
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

pub(crate) fn uniform_hdri() -> Environment {
    let hdr = TemporaryHdr::from_rgbe("e1-uniform", 4, 2, &[[128, 64, 32, 132]; 8]);
    Environment::try_from(&EnvironmentConfig {
        map_path: Some(hdr.0.clone()),
        ..Default::default()
    })
    .unwrap()
}

pub(super) fn evaluate_path(
    scene: &RenderJob,
    ray: Ray,
    rngs: &mut CameraPathRngs<'_>,
    light_wi: Vector3<f64>,
    light: f64,
    ground: Option<&GroundLighting<'_>>,
) -> Result<CameraPathComponents, Box<dyn Error + Send + Sync>> {
    let mut state = CameraPathState::new(ray, scene.camera_options.max_diffuse_bounces);
    super::super::path::trace_camera_path(
        scene,
        &mut state,
        rngs,
        light_wi,
        light,
        (ground).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )?;
    Ok(state.radiance)
}

pub(super) fn trace_continuation(
    scene: &RenderJob,
    hit: &Hit,
    material: &dyn Material,
    light: (Vector3<f64>, f64),
    ground: Option<&GroundLighting<'_>>,
    rng: &mut dyn rand::RngCore,
    state: &mut CameraPathState,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if state.max_non_delta_bounces == 0 {
        return Ok(());
    }
    let Some(first) = super::super::path::sample_scatter_ray(
        scene,
        hit,
        material,
        -state.ray.direction,
        crate::integrator::camera::transport::Rgb::default(),
        crate::optics::dispersion::IorMode::Constant,
        rng,
    )?
    else {
        return Ok(());
    };
    if !state.apply_scatter(first.event, first.throughput)? {
        return Ok(());
    }
    state.ray = first.ray;
    let mut unused_camera = SequenceRng::new(&[]);
    let mut unused_environment = SequenceRng::new(&[]);
    super::super::path::trace_camera_path(
        scene,
        state,
        &mut super::super::types::CameraPathRngs {
            camera: &mut unused_camera,
            environment: &mut unused_environment,
            diffuse: rng,
            medium: &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(0),
        },
        light.0,
        light.1,
        (ground).map_or(Caustics::None, Caustics::Rgb),
        crate::optics::dispersion::IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
}

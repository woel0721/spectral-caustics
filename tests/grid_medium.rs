use nalgebra::Vector3;
use rand::{RngCore, SeedableRng, rngs::StdRng};
use spectral_caustics::{
    config::{Config, RenderJob, render::CameraMode},
    geometry::{ground::Ground, ray::Ray},
    integrator::camera::{
        integrate_rgb, integrate_spectral,
        path::trace_camera_path,
        transport::{CameraTransport, Caustics},
        types::{CameraPathRngs, CameraPathState},
    },
    material::lambertian::Lambertian,
    medium::{
        GridMedium, GridMediumConfig, HomogeneousMedium, HomogeneousMediumConfig, Medium,
        MediumCoefficients, MediumConfig, MediumSample, phase::HenyeyGreenstein,
    },
    optics::dispersion::IorMode,
    renderer::Renderer,
};

// Defined outside the library: a custom implementation needs no config variant
// or registration to participate in the shared camera pipeline.
struct CustomMedium {
    inner: HomogeneousMedium,
    sample_override: Option<MediumSample>,
}

impl Medium for CustomMedium {
    fn density_at(&self, position: Vector3<f64>) -> f64 {
        self.inner.density_at(position)
    }
    fn coefficients_at(&self, position: Vector3<f64>) -> MediumCoefficients {
        self.inner.coefficients_at(position)
    }
    fn sigma_t_majorant(&self) -> f64 {
        self.inner.sigma_t_majorant()
    }
    fn phase(&self) -> HenyeyGreenstein {
        self.inner.phase()
    }
    fn transmittance_ray(&self, ray: &Ray, max_distance: f64) -> f64 {
        self.inner.transmittance_ray(ray, max_distance)
    }
    fn sample_segment(
        &self,
        ray: &Ray,
        max_distance: f64,
        allow_scattering: bool,
        rng: &mut dyn RngCore,
    ) -> Result<MediumSample, String> {
        if let Some(sample) = self.sample_override {
            return Ok(sample);
        }
        self.inner
            .sample_segment(ray, max_distance, allow_scattering, rng)
    }
}

#[test]
fn custom_medium_runs_rgb_hero_packet_in_parallel_without_a_config_variant() {
    let mut c = Config::load("configs/homogeneous_fog.json").unwrap();
    c.render.width = 12;
    c.render.height = 8;
    c.render.camera_spp = 16;
    c.render.thread_count = Some(4);
    let Some(MediumConfig::Homogeneous(m)) = &mut c.scene.medium else {
        panic!("expected homogeneous medium");
    };
    m.sigma_a = 0.5;
    m.sigma_s = 0.0;
    let coefficients = *m;
    for (mode, packet) in [
        (CameraMode::Rgb, 1),
        (CameraMode::Spectral, 1),
        (CameraMode::Spectral, 4),
    ] {
        c.render.camera_mode = mode;
        c.render.spectral_packet_size = packet;
        let mut job = c.build().unwrap();
        let renderer = Renderer::new(c.render.clone()).unwrap();
        let builtin = renderer.render(&job).unwrap();
        job.scene.medium = Some(Box::new(CustomMedium {
            inner: HomogeneousMedium::try_from(&coefficients).unwrap(),
            sample_override: None,
        }));
        let custom = renderer.render(&job).unwrap();
        assert_eq!(custom.direct.pixels(), builtin.direct.pixels());
        assert_eq!(custom.caustics.pixels(), builtin.caustics.pixels());
        assert_eq!(custom.combined.pixels(), builtin.combined.pixels());
    }
}

#[test]
fn camera_rejects_invalid_custom_medium_samples_before_accumulating_radiance() {
    let inner = HomogeneousMedium::try_from(&HomogeneousMediumConfig {
        min_m: [-1.0; 3],
        max_m: [1.0; 3],
        sigma_a: 0.5,
        sigma_s: 0.0,
        g: 0.0,
    })
    .unwrap();
    let mut job = RenderJob::new();
    for (weight, position, budget) in [
        (-1.0, None, 1),
        (f64::NAN, None, 1),
        (f64::INFINITY, None, 1),
        (1.0, Some(Vector3::repeat(f64::NAN)), 1),
        (1.0, Some(Vector3::zeros()), 0),
    ] {
        job.camera_options.max_volume_scatters = budget;
        job.scene.medium = Some(Box::new(CustomMedium {
            inner,
            sample_override: Some(MediumSample { position, weight }),
        }));
        let mut rgb = CameraPathState::new(Ray::new(Vector3::z() * 2.0, -Vector3::z()).unwrap(), 0);
        let mut scalar = CameraPathState::spectral(rgb.ray, 550.0, 0).unwrap();
        let mut rng = StdRng::seed_from_u64(42);
        let mut untouched = rng.clone();
        assert!(
            trace(&job, &mut rgb, 0.0, &mut rng)
                .unwrap_err()
                .to_string()
                .contains("invalid medium sample")
        );
        assert!(
            trace(&job, &mut scalar, 0.0, &mut rng)
                .unwrap_err()
                .to_string()
                .contains("invalid medium sample")
        );
        assert_eq!(rgb.radiance.direct, Vector3::zeros());
        assert_eq!(scalar.radiance.direct, 0.0);
        assert_eq!(rng.next_u64(), untouched.next_u64());
    }
}

fn layers(a: f64, s: f64) -> GridMediumConfig {
    GridMediumConfig {
        min_m: [-100.0, -100.0, 1.0],
        max_m: [100.0, 100.0, 3.0],
        resolution: [1, 1, 2],
        densities: vec![1.0, 3.0],
        sigma_a_base: a,
        sigma_s_base: s,
        g: 0.7,
    }
}

fn trace<T: CameraTransport>(
    job: &RenderJob,
    state: &mut CameraPathState<T>,
    light: f64,
    rng: &mut StdRng,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    trace_camera_path(
        job,
        state,
        &mut CameraPathRngs {
            camera: &mut StdRng::seed_from_u64(1),
            environment: &mut StdRng::seed_from_u64(2),
            diffuse: &mut StdRng::seed_from_u64(3),
            medium: rng,
        },
        Vector3::z(),
        light,
        Caustics::None,
        IorMode::Constant,
        #[cfg(feature = "diagnostics")]
        None,
    )
}

#[test]
fn two_layer_grid_attenuates_camera_and_sun_shadow_once_and_preserves_rng_in_rgb_and_scalar() {
    for (a, s, budget) in [(0.5, 0.0, 8), (0.2, 0.3, 0)] {
        let mut job = RenderJob::new();
        job.scene.environment = None;
        job.scene.background_color = Vector3::repeat(1.0);
        job.camera_options.max_volume_scatters = budget;
        job.scene.medium = Some(Box::new(GridMedium::try_from(&layers(a, s)).unwrap()));
        let ray = Ray::new(Vector3::z() * 4.0, -Vector3::z()).unwrap();
        let mut rng = StdRng::seed_from_u64(42);
        let mut untouched = rng.clone();
        let mut rgb = CameraPathState::new(ray, 0);
        let mut scalar = CameraPathState::spectral(ray, 550.0, 0).unwrap();
        trace(&job, &mut rgb, 0.0, &mut rng).unwrap();
        trace(&job, &mut scalar, 0.0, &mut rng).unwrap();
        let tr = (-2.0_f64).exp(); // 0.5 * (1 m * 1 + 1 m * 3)
        assert_eq!(rgb.radiance.direct, Vector3::repeat(tr));
        let emission = scalar.transport.emission(Vector3::repeat(1.0)).unwrap();
        assert_eq!(scalar.radiance.direct, emission * tr);
        assert_eq!(rgb.volume_scatters, 0);
        assert_eq!(scalar.volume_scatters, 0);
        assert_eq!(rng.next_u64(), untouched.next_u64());
        job.scene.background_color = Vector3::zeros();
        job.photon.ground_id = Some(job.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(Lambertian::new(Vector3::repeat(0.5)).unwrap()),
        ));
        let mut surface = CameraPathState::new(ray, 0);
        trace(&job, &mut surface, 1.0, &mut rng).unwrap();
        let expected = 0.5 / std::f64::consts::PI * tr * tr;
        assert!((surface.radiance.direct.x - expected).abs() < 1e-15);
    }
}

#[test]
fn grid_json_roundtrips_builds_and_validates_shape_density_and_sampling_support() {
    let c = Config::load("configs/grid_fog.json").unwrap();
    c.build().unwrap();
    assert_eq!(Config::from_json(&c.to_json().unwrap()).unwrap(), c);
    let mut json = serde_json::to_value(&c).unwrap();
    for (field, value) in [
        ("densities", serde_json::json!(vec![-1.0; 24])),
        ("resolution", serde_json::json!([4, 2, 4])),
        ("resolution", serde_json::json!([0, 2, 3])),
        ("sigma_a_base", serde_json::json!(-0.1)),
    ] {
        let mut bad = json.clone();
        bad["scene"]["medium"][field] = value;
        let bad = Config::from_json(&bad.to_string()).unwrap();
        assert!(bad.validate().is_err());
        assert!(bad.build().is_err());
    }
    json["scene"]["medium"]["unknown_field"] = serde_json::json!(0);
    assert!(Config::from_json(&json.to_string()).is_err());
    for budget in [0, 1, 8] {
        let mut supported = c.clone();
        supported.scene.max_volume_scatters = budget;
        supported.validate().unwrap();
        supported.build().unwrap();
    }
}

#[test]
fn grid_scattering_is_supported_at_direct_rgb_and_spectral_camera_entries() {
    let mut c = Config::load("configs/grid_fog.json").unwrap();
    c.render.width = 1;
    c.render.height = 1;
    c.render.camera_spp = 4;
    let mut job = c.build().unwrap();
    job.camera_options.max_volume_scatters = 8;
    integrate_rgb(&job, &c.render, None).unwrap();
    integrate_spectral(&job, &c.render, None).unwrap();
    let mut state = CameraPathState::new(
        Ray::new(Vector3::new(0.0, -4.0, 2.0), Vector3::y()).unwrap(),
        0,
    );
    let mut rng = StdRng::seed_from_u64(42);
    trace(&job, &mut state, 1.0, &mut rng).unwrap();
    assert!(state.radiance.direct.iter().all(|v| v.is_finite()));
}

#[test]
fn configured_grid_repeats_across_rgb_hero_packet_threads_and_zero_density_is_vacuum() {
    let mut c = Config::load("configs/grid_fog.json").unwrap();
    c.render.width = 12;
    c.render.height = 8;
    c.render.camera_spp = 16;
    for (mode, packet) in [
        (CameraMode::Rgb, 1),
        (CameraMode::Spectral, 1),
        (CameraMode::Spectral, 4),
    ] {
        c.render.camera_mode = mode;
        c.render.spectral_packet_size = packet;
        let mut reference = None;
        for threads in [1, 2, 4] {
            c.render.thread_count = Some(threads);
            let images = Renderer::new(c.render.clone())
                .unwrap()
                .render(&c.build().unwrap())
                .unwrap();
            for ((d, ca), combined) in images
                .direct
                .pixels()
                .iter()
                .zip(images.caustics.pixels())
                .zip(images.combined.pixels())
            {
                assert_eq!(d + ca, *combined);
                assert!(combined.iter().all(|v| v.is_finite()));
            }
            let components = (
                images.direct.pixels().to_vec(),
                images.caustics.pixels().to_vec(),
                images.combined.pixels().to_vec(),
            );
            if let Some(reference) = &reference {
                assert_eq!(&components, reference);
            } else {
                reference = Some(components);
            }
        }
        let mut vacuum = c.clone();
        vacuum.scene.medium = None;
        let mut zero = c.clone();
        let Some(MediumConfig::Grid(m)) = &mut zero.scene.medium else {
            panic!("expected grid")
        };
        m.densities.fill(0.0);
        zero.scene.max_volume_scatters = 8; // zero effective sigma_s needs no sampler
        let render = |c: &Config| {
            Renderer::new(c.render.clone())
                .unwrap()
                .render(&c.build().unwrap())
                .unwrap()
        };
        assert_eq!(
            render(&zero).combined.pixels(),
            render(&vacuum).combined.pixels()
        );
    }
}

#[test]
fn uniform_grid_and_homogeneous_render_the_same_extinction_in_rgb_hero_and_packet() {
    let mut c = Config::load("configs/grid_fog.json").unwrap();
    c.render.width = 12;
    c.render.height = 8;
    c.render.camera_spp = 16;
    c.scene.max_volume_scatters = 0;
    let Some(MediumConfig::Grid(m)) = &mut c.scene.medium else {
        panic!("expected grid")
    };
    m.densities.fill(1.0);
    let homogeneous = HomogeneousMediumConfig {
        min_m: m.min_m,
        max_m: m.max_m,
        sigma_a: m.sigma_a_base,
        sigma_s: m.sigma_s_base,
        g: m.g,
    };
    for (mode, packet) in [
        (CameraMode::Rgb, 1),
        (CameraMode::Spectral, 1),
        (CameraMode::Spectral, 4),
    ] {
        c.render.camera_mode = mode;
        c.render.spectral_packet_size = packet;
        let grid = Renderer::new(c.render.clone())
            .unwrap()
            .render(&c.build().unwrap())
            .unwrap();
        let mut h = c.clone();
        h.scene.medium = Some(MediumConfig::Homogeneous(homogeneous));
        let uniform = Renderer::new(h.render.clone())
            .unwrap()
            .render(&h.build().unwrap())
            .unwrap();
        for (actual, expected) in grid.combined.pixels().iter().zip(uniform.combined.pixels()) {
            assert!((actual - expected).norm() < 1e-12);
        }
    }
}

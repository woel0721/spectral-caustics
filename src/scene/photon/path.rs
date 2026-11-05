//! 单个理想介质的有界通量路径，不分裂光子、不在界面存储或累积能量。

use super::{PhotonLanding, Scene};
use crate::{
    geometry::{hit::Hit, ray::Ray},
    material::dielectric::fresnel_dielectric,
    optics::refract,
};
use nalgebra::Vector3;
use rand::{Rng, RngCore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotonMedium {
    Air,
    /// 当前路径所选唯一玻璃，IOR 在整条单波长路径上固定。
    Glass,
}

#[derive(Debug, Clone, Copy)]
pub struct PhotonPathState {
    pub ray: Ray,
    pub medium: PhotonMedium,
    /// 相对于发射光谱通量 Φλ 的无量纲系数，不含 CIE、Δλ 或像素面积。
    pub flux_weight: f64,
    pub event_count: usize,
    pub reflections: usize,
    pub transmissions: usize,
}

impl PhotonPathState {
    fn new(ray: Ray) -> Self {
        Self {
            ray,
            medium: PhotonMedium::Air,
            flux_weight: 1.0,
            event_count: 0,
            reflections: 0,
            transmissions: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PhotonTermination {
    Ground(Vector3<f64>),
    Escaped,
    Blocked,
    EventLimit,
}

#[derive(Debug, Clone, Copy)]
pub struct PhotonPathResult {
    pub state: PhotonPathState,
    pub termination: PhotonTermination,
}

impl PhotonPathResult {
    /// 唯一可沉积的终态；逃逸、遮挡及深度截断均不重新分配通量。
    pub fn landing(&self) -> Option<PhotonLanding> {
        match self.termination {
            PhotonTermination::Ground(position) => Some(PhotonLanding {
                position,
                weight: self.state.flux_weight,
            }),
            _ => None,
        }
    }
}

pub(crate) struct PhotonTracer {
    pub object_id: usize,
    pub ground_id: usize,
    pub glass_ior: f64,
    pub max_events: usize,
}

impl PhotonTracer {
    pub fn trace(
        &self,
        scene: &Scene,
        ray: Ray,
        first_hit: Option<Hit>,
        rng: &mut dyn RngCore,
    ) -> Result<PhotonPathResult, &'static str> {
        if self.object_id == self.ground_id
            || self.object_id >= scene.objects.len()
            || self.ground_id >= scene.objects.len()
            || !scene.supports_ideal_caustics(self.object_id)
            || !self.glass_ior.is_finite()
            || self.glass_ior <= 0.0
            || !(1..=1024).contains(&self.max_events)
        {
            return Err("光子路径要求有效的单一理想介质、地面、IOR 和 1..1024 事件上限");
        }
        self.trace_state(scene, PhotonPathState::new(ray), first_hit, rng)
    }

    fn trace_state(
        &self,
        scene: &Scene,
        mut state: PhotonPathState,
        mut hit: Option<Hit>,
        rng: &mut dyn RngCore,
    ) -> Result<PhotonPathResult, &'static str> {
        let termination = loop {
            let Some(current) = hit else {
                break PhotonTermination::Escaped;
            };
            if current.object_id == Some(self.ground_id) && state.medium == PhotonMedium::Air {
                break PhotonTermination::Ground(current.coords);
            }
            if current.object_id != Some(self.object_id) {
                break PhotonTermination::Blocked;
            }
            // 允许恰好完成 max_events 次散射后的落地/逃逸，下一次散射才截断。
            if state.event_count == self.max_events {
                break PhotonTermination::EventLimit;
            }
            self.scatter(&mut state, current, rng)?;
            hit = scene.intersect(&state.ray);
        };
        Ok(PhotonPathResult { state, termination })
    }

    fn scatter(
        &self,
        state: &mut PhotonPathState,
        hit: Hit,
        rng: &mut dyn RngCore,
    ) -> Result<(), &'static str> {
        let (normal, eta_i, eta_t, transmitted_medium) = match state.medium {
            PhotonMedium::Air => (hit.normal, 1.0, self.glass_ior, PhotonMedium::Glass),
            PhotonMedium::Glass => (-hit.normal, self.glass_ior, 1.0, PhotonMedium::Air),
        };
        let cos_i = -state.ray.direction.dot(&normal);
        // 背面/切线与显式介质不匹配时不能偷偷翻转介质（例如从内部发射）。
        if !cos_i.is_finite() || cos_i <= 0.0 {
            return Err("光子方向与当前介质/界面法线不一致");
        }
        let f = fresnel_dielectric(cos_i, eta_i, eta_t).ok_or("光子 Fresnel 无效")?;
        // 离散 delta 事件的概率质量，不是立体角 PDF。TIR/F=0 不消耗 RNG。
        let reflected = f == 1.0 || (f > 0.0 && rng.random::<f64>() < f);
        let (direction, coefficient, probability, offset) = if reflected {
            (state.ray.direction + 2.0 * cos_i * normal, f, f, normal)
        } else {
            (
                refract(state.ray.direction, normal, eta_i, eta_t).ok_or("光子透射方向无效")?,
                1.0 - f,
                1.0 - f,
                -normal,
            )
        };
        // 光子通量采用 importance 传输：没有相机 radiance 的 (eta_i/eta_t)^2。
        // 按 Fresnel 采样时系数与概率相消；不能再额外乘 F 或 1-F。
        state.flux_weight *= coefficient / probability;
        if !state.flux_weight.is_finite() || state.flux_weight <= 0.0 {
            return Err("光子通量权重无效");
        }
        state.ray = Ray::new(hit.coords + offset * 1e-5, direction).ok_or("光子续追 Ray 无效")?;
        state.event_count += 1;
        if reflected {
            state.reflections += 1;
        } else {
            state.transmissions += 1;
            state.medium = transmitted_medium;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::{ground::Ground, object::Hittable, prism::Prism, sphere::Sphere},
        material::dielectric::Dielectric,
        optics::iormode::IorMode,
        scene::take_photon_landing,
    };
    use rand::SeedableRng;
    use rand_chacha::ChaCha12Rng;

    struct Sequence {
        values: Vec<u64>,
        draws: usize,
    }

    impl Sequence {
        fn new(values: &[u64]) -> Self {
            Self {
                values: values.to_vec(),
                draws: 0,
            }
        }
    }

    impl RngCore for Sequence {
        fn next_u32(&mut self) -> u32 {
            self.next_u64() as u32
        }
        fn next_u64(&mut self) -> u64 {
            let value = self.values[self.draws];
            self.draws += 1;
            value
        }
        fn fill_bytes(&mut self, _: &mut [u8]) {
            panic!("仅应采样 f64")
        }
    }

    struct Slab;
    impl Hittable for Slab {
        fn intersect(&self, ray: &Ray) -> Option<Hit> {
            let top = Ground::new(2.0)?.intersect(ray);
            let bottom = Ground::new(1.0)?.intersect(ray).map(|mut h| {
                h.normal = -h.normal;
                h
            });
            top.into_iter()
                .chain(bottom)
                .min_by(|a, b| a.distance.total_cmp(&b.distance))
        }
    }

    fn slab(ior: f64, max_events: usize) -> (Scene, PhotonTracer, Ray) {
        let mut scene = Scene::new();
        let object_id =
            scene.add_with_material(Box::new(Slab), Box::new(Dielectric::new(ior).unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        scene.ground_id = Some(ground_id);
        scene.select_caustics_object(object_id).unwrap();
        let ray = Ray::new(Vector3::new(0.0, 0.0, 3.0), -Vector3::z()).unwrap();
        (
            scene,
            PhotonTracer {
                object_id,
                ground_id,
                glass_ior: ior,
                max_events,
            },
            ray,
        )
    }

    #[test]
    fn four_internal_reflections_then_transmission_deposit_once_and_limit_drops_flux() {
        let (scene, mut tracer, ray) = slab(1.5, 6);
        let mut rng = Sequence::new(&[u64::MAX, 0, 0, 0, 0, u64::MAX]);
        let result = tracer
            .trace(&scene, ray, scene.intersect(&ray), &mut rng)
            .unwrap();
        assert_eq!(
            (
                result.state.event_count,
                result.state.reflections,
                result.state.transmissions
            ),
            (6, 4, 2)
        );
        assert_eq!(result.state.medium, PhotonMedium::Air);
        assert_eq!(
            result.landing().unwrap(),
            PhotonLanding {
                position: Vector3::zeros(),
                weight: 1.0
            }
        );
        assert_eq!(rng.draws, 6); // 落地后不再采样或产生第二份贡献。
        tracer.max_events = 2;
        let result = tracer
            .trace(
                &scene,
                ray,
                scene.intersect(&ray),
                &mut Sequence::new(&[u64::MAX, 0]),
            )
            .unwrap();
        assert_eq!(result.termination, PhotonTermination::EventLimit);
        assert_eq!(result.state.medium, PhotonMedium::Glass);
        assert_eq!(result.state.event_count, 2);
        assert!(result.landing().is_none());
    }

    #[test]
    fn tir_keeps_medium_flux_and_unit_direction_without_rng_until_limit() {
        let (scene, tracer, _) = slab(1.5, 8);
        for angle in [50.0_f64, 80.0] {
            let angle = angle.to_radians();
            let ray = Ray::new(
                Vector3::new(0.0, 0.0, 1.5),
                Vector3::new(angle.sin(), 0.0, -angle.cos()),
            )
            .unwrap();
            let mut state = PhotonPathState::new(ray);
            state.medium = PhotonMedium::Glass;
            state.flux_weight = 2.5;
            let mut rng = Sequence::new(&[]);
            let result = tracer
                .trace_state(&scene, state, scene.intersect(&ray), &mut rng)
                .unwrap();
            assert_eq!(result.termination, PhotonTermination::EventLimit);
            assert_eq!(
                (result.state.reflections, result.state.transmissions),
                (8, 0)
            );
            assert_eq!(result.state.medium, PhotonMedium::Glass);
            assert_eq!(result.state.flux_weight, 2.5);
            assert!((result.state.ray.direction.norm() - 1.0).abs() < 1e-14);
            assert_eq!(rng.draws, 0);
        }
    }

    #[test]
    fn prism_tir_continues_past_the_old_two_interface_failure() {
        let mut scene = Scene::new();
        let object_id = scene.add(Box::new(Prism::new().unwrap()));
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        let ray = Ray::new(Vector3::new(1.05, 0.2, 1.9), Vector3::new(-1.6, 0.0, -1.0)).unwrap();
        assert!(take_photon_landing(&scene, ray, object_id, ground_id, || Some(1.52)).is_none());
        let tracer = PhotonTracer {
            object_id,
            ground_id,
            glass_ior: 1.52,
            max_events: 16,
        };
        let result = tracer
            .trace(
                &scene,
                ray,
                scene.intersect(&ray),
                &mut Sequence::new(&[u64::MAX; 16]),
            )
            .unwrap();
        assert!(result.state.event_count > 2);
        assert!(result.state.reflections > 0);
        assert_eq!(result.state.transmissions, 2);
        assert_eq!(result.state.medium, PhotonMedium::Air);
        assert_ne!(result.termination, PhotonTermination::EventLimit);
        assert_eq!(result.state.flux_weight, 1.0);
    }

    #[test]
    fn two_transmission_geometry_and_probability_match_the_preserved_baseline() {
        for ior in [1.0, 1.5, 2.0] {
            let (scene, tracer, _) = slab(ior, 2);
            for angle in [0.0_f64, 60.0, 80.0] {
                let theta = angle.to_radians();
                let ray = Ray::new(
                    Vector3::new(0.0, 0.0, 3.0),
                    Vector3::new(theta.sin(), 0.0, -theta.cos()),
                )
                .unwrap();
                let reference =
                    take_photon_landing(&scene, ray, tracer.object_id, tracer.ground_id, || {
                        Some(ior)
                    })
                    .unwrap();
                let mut rng = Sequence::new(&[u64::MAX; 2]);
                let result = tracer
                    .trace(&scene, ray, scene.intersect(&ray), &mut rng)
                    .unwrap();
                assert!((result.landing().unwrap().position - reference.position).norm() < 1e-12);
                let f = fresnel_dielectric(theta.cos(), 1.0, ior).unwrap();
                assert!(
                    (reference.weight - (1.0 - f).powi(2) * result.state.flux_weight).abs() < 1e-12
                );
                assert_eq!(rng.draws, if ior == 1.0 { 0 } else { 2 });
            }
        }
    }

    #[test]
    fn sampled_slab_flux_matches_analytic_series_and_accounts_for_every_photon() {
        // 透射总和 (1-F)^2 Σ F^(2k) = (1-F)/(1+F)，包含所有内部往返。
        const N: usize = 50000;
        for (ior, angle) in [(1.5, 0.0_f64), (3.0, 0.0), (1.5, 80.0)] {
            let (scene, mut tracer, _) = slab(ior, 32);
            let theta = angle.to_radians();
            let ray = Ray::new(
                Vector3::new(0.0, 0.0, 3.0),
                Vector3::new(theta.sin(), 0.0, -theta.cos()),
            )
            .unwrap();
            let f = fresnel_dielectric(theta.cos(), 1.0, ior).unwrap();
            for limit in [2, 32] {
                tracer.max_events = limit;
                let mut rng = ChaCha12Rng::seed_from_u64(42);
                let mut totals = [0.0; 3]; // 接收、逃逸、截断；三个互斥终态。
                let mut two = 0.0;
                for _ in 0..N {
                    let result = tracer
                        .trace(&scene, ray, scene.intersect(&ray), &mut rng)
                        .unwrap();
                    let bucket = match result.termination {
                        PhotonTermination::Ground(_) => 0,
                        PhotonTermination::Escaped => 1,
                        PhotonTermination::EventLimit => 2,
                        PhotonTermination::Blocked => panic!("玻璃板无其他遮挡"),
                    };
                    totals[bucket] += result.state.flux_weight;
                    if bucket == 0 && result.state.event_count == 2 {
                        two += result.state.flux_weight;
                    }
                }
                assert_eq!(totals.iter().sum::<f64>(), N as f64);
                assert!((two / N as f64 - (1.0 - f).powi(2)).abs() < 0.008);
                let expected = if limit == 2 {
                    (1.0 - f).powi(2)
                } else {
                    (1.0 - f) / (1.0 + f)
                };
                assert!((totals[0] / N as f64 - expected).abs() < 0.008);
                if limit == 2 {
                    assert!((totals[1] / N as f64 - f).abs() < 0.008);
                    assert!((totals[2] / N as f64 - (1.0 - f) * f).abs() < 0.008);
                } else {
                    assert_eq!(totals[2], 0.0);
                }
            }
        }
    }

    #[test]
    fn external_reflection_can_reach_ground_and_other_objects_absorb() {
        let mut scene = Scene::new();
        let object_id = scene.add_with_material(
            Box::new(Sphere::new(Vector3::new(0.0, 0.0, 1.5), 0.8).unwrap()),
            Box::new(Dielectric::new(1.5).unwrap()),
        );
        let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
        scene.ground_id = Some(ground_id);
        scene.select_caustics_object(object_id).unwrap();
        let ray = Ray::new(Vector3::new(2.0, 0.0, 1.2), -Vector3::x()).unwrap();
        let result = scene
            .trace_forward_photon(ray, None, IorMode::Constant, 1, &mut Sequence::new(&[0]))
            .unwrap();
        assert_eq!(
            (result.state.reflections, result.state.transmissions),
            (1, 0)
        );
        assert_eq!(result.landing().unwrap().weight, 1.0);
        scene.add(Box::new(Ground::new(0.5).unwrap()));
        let result = scene
            .trace_forward_photon(ray, None, IorMode::Constant, 1, &mut Sequence::new(&[0]))
            .unwrap();
        assert_eq!(result.termination, PhotonTermination::Blocked);
        assert!(result.landing().is_none());
    }

    #[test]
    fn direct_miss_invalid_inputs_and_inside_launch_have_explicit_outcomes() {
        let (scene, mut tracer, _) = slab(1.5, 2);
        for (origin, direction, expected) in [
            (
                Vector3::new(0.0, 0.0, 0.5),
                -Vector3::z(),
                PhotonTermination::Ground(Vector3::zeros()),
            ),
            (
                Vector3::new(0.0, 0.0, 3.0),
                Vector3::z(),
                PhotonTermination::Escaped,
            ),
        ] {
            let ray = Ray::new(origin, direction).unwrap();
            let result = tracer
                .trace(&scene, ray, scene.intersect(&ray), &mut Sequence::new(&[]))
                .unwrap();
            assert_eq!(result.termination, expected);
            assert_eq!(result.state.event_count, 0);
        }
        let ray = Ray::new(Vector3::new(0.0, 0.0, 1.5), Vector3::z()).unwrap();
        assert!(
            tracer
                .trace(&scene, ray, scene.intersect(&ray), &mut Sequence::new(&[]))
                .is_err()
        );
        for ior in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            tracer.glass_ior = ior;
            assert!(
                tracer
                    .trace(&scene, ray, None, &mut Sequence::new(&[]))
                    .is_err()
            );
        }
        tracer.glass_ior = 1.5;
        for limit in [0, 1025, usize::MAX] {
            tracer.max_events = limit;
            assert!(
                tracer
                    .trace(&scene, ray, None, &mut Sequence::new(&[]))
                    .is_err()
            );
        }
    }
}

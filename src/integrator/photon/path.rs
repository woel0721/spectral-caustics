// 单个理想介质的有界通量路径，不分裂光子、不在界面存储或累积能量。

use super::PhotonLanding;
use crate::config::RenderJob;
use crate::geometry::hit::Hit;
use crate::geometry::ray::Ray;
use nalgebra::Vector3;
use rand::RngCore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotonMedium {
    /// 当前在空气中，折射率为 1。
    Air,
    /// 当前路径所选唯一玻璃，IOR 在整条单波长路径上固定。
    Glass,
}

#[derive(Debug, Clone, Copy)]
pub struct PhotonPathState {
    /// 当前向前续追的射线，从光源或上一个界面出发。
    pub ray: Ray,
    /// 射线当前所在的介质，用于决定界面两侧的折射率与法线方向。
    pub medium: PhotonMedium,
    /// 相对于发射光谱通量 Φλ 的无量纲系数，不含 CIE、Δλ 或像素面积。
    pub flux_weight: f64,
    /// 已发生的反射/透射总次数，用于事件上限截断。
    pub event_count: usize,
    /// 其中反射的次数，包含全反射。
    #[cfg(any(feature = "diagnostics", test))]
    pub reflections: usize,
    /// 其中穿过界面的次数，每次透射切换 Air/Glass。
    #[cfg(any(feature = "diagnostics", test))]
    pub transmissions: usize,
}

impl PhotonPathState {
    /// 从输入 Ray 初始化空气中的光子状态，通量比例为 1，事件/反射/透射计数均为 0，供 Forward 开始追踪。
    fn new(ray: Ray) -> Self {
        Self {
            ray,
            medium: PhotonMedium::Air,
            flux_weight: 1.0,
            event_count: 0,
            #[cfg(any(feature = "diagnostics", test))]
            reflections: 0,
            #[cfg(any(feature = "diagnostics", test))]
            transmissions: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PhotonTermination {
    /// 在空气中首次到达地面，坐标可用于沉积通量。
    Ground(Vector3<f64>),
    /// 没有下一交点，光子离开场景。
    Escaped,
    /// 命中选中玻璃和可接收地面之外的物体，通量被遮挡丢弃。
    Blocked,
    /// 达到散射次数上限，截断剩余通量。
    EventLimit,
}

#[derive(Debug, Clone, Copy)]
pub struct PhotonPathResult {
    /// 终止时的射线、介质、通量权重与事件统计。
    pub state: PhotonPathState,
    /// 停止的原因；只有 Ground 能通过 landing() 提取落地贡献。
    pub termination: PhotonTermination,
}

impl PhotonPathResult {
    /// 从追踪结果提取唯一可沉积的落地贡献。
    ///
    /// 输入：self；输出：Ground 终态对应的 PhotonLanding，其他终态为 None；逃逸、遮挡与截断通量不重分配。
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
    pub fn new(
        scene: &RenderJob,
        object_id: usize,
        ground_id: usize,
        glass_ior: f64,
        max_events: usize,
    ) -> Result<Self, &'static str> {
        if object_id == ground_id
            || object_id >= scene.scene.objects.len()
            || ground_id >= scene.scene.objects.len()
            || !scene.supports_ideal_caustics(object_id)
            || !glass_ior.is_finite()
            || glass_ior <= 0.0
            || !(1..=1024).contains(&max_events)
        {
            return Err("光子路径要求有效的单一理想介质、地面、IOR 和 1..1024 事件上限");
        }
        Ok(Self {
            object_id,
            ground_id,
            glass_ior,
            max_events,
        })
    }

    /// 校验参数并启动单条 Forward 光子路径，供 XYZ 和 Eλ 图集共用。
    ///
    /// 输入：scene、从空气出发的 ray、它的最近 first_hit（无命中为 None）及事件 RNG。
    ///
    /// 输出：最终 PhotonPathResult；目标/地面/IOR/事件预算无效或界面状态错误时 Err。
    pub fn trace(
        &self,
        scene: &RenderJob,
        ray: Ray,
        first_hit: Option<Hit>,
        rng: &mut dyn RngCore,
    ) -> Result<PhotonPathResult, &'static str> {
        self.trace_state(scene, PhotonPathState::new(ray), first_hit, rng)
    }

    /// 反复求交和散射直到光子落地或终止，供 Forward 追踪循环使用。
    ///
    /// 输入：场景、当前路径状态、当前最近命中和事件 RNG；输出：保留最终状态及落地/逃逸/遮挡/事件上限的结果。
    /// 路径状态或界面数值错误返回 Err；恰好完成 max_events 次后仍允许落地/逃逸，到下一次散射才截断。
    fn trace_state(
        &self,
        scene: &RenderJob,
        mut state: PhotonPathState,
        mut hit: Option<Hit>,
        rng: &mut dyn RngCore,
    ) -> Result<PhotonPathResult, &'static str> {
        let termination = loop {
            let Some(current) = hit else {
                break PhotonTermination::Escaped;
            };
            if current.object_id == (self.ground_id) && state.medium == PhotonMedium::Air {
                break PhotonTermination::Ground(current.coords);
            }
            if current.object_id != (self.object_id) {
                break PhotonTermination::Blocked;
            }
            // 允许恰好完成 max_events 次散射后的落地/逃逸，下一次散射才截断。
            if state.event_count == self.max_events {
                break PhotonTermination::EventLimit;
            }
            self.scatter(&mut state, current, rng)?;
            hit = scene.scene.intersect(&state.ray);
        };
        Ok(PhotonPathResult { state, termination })
    }

    /// 在当前玻璃界面抽取一次反射或透射，并原地更新光子 Ray、介质、通量和计数。
    ///
    /// 输入：可变 state、对应交点 hit、独立事件 RNG；输出：成功为 Ok(())，介质/法线不匹配或计算失败为 Err。
    ///
    /// 公式：反射概率 p_r=F、透射概率 p_t=1-F，通量 W←W·C/p，C 分别为 F 或 1-F。
    /// 按 Fresnel 抽样时 C/p=1；光子传输不乘相机的 (η_i/η_t)²，也不能再乘一次 F/1-F。
    /// 全反射与 F=0 的确定事件不抽 RNG；从真实交点续追，由几何 ε 排除自交，避免法线偏移跨过尖棱。
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
        let interface = crate::optics::interface::sample_dielectric(
            state.ray.direction,
            normal,
            eta_i,
            eta_t,
            rng,
        )
        .ok_or("invalid photon interface")?;
        let reflected = interface.event == crate::optics::interface::DielectricEvent::Reflection;
        state.flux_weight *= interface.flux_weight();
        // Starting at the real hit avoids crossing the adjacent face at a prism corner.
        state.ray = Ray::new(hit.coords, interface.direction).ok_or("invalid photon ray")?;
        state.event_count += 1;
        if reflected {
            #[cfg(any(feature = "diagnostics", test))]
            {
                state.reflections += 1;
            }
        } else {
            #[cfg(any(feature = "diagnostics", test))]
            {
                state.transmissions += 1;
            }
            state.medium = transmitted_medium;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::ground::Ground;
    use crate::geometry::hit::GeometryHit;
    use crate::geometry::object::Hittable;
    use crate::geometry::sphere::Sphere;
    use crate::material::dielectric::Dielectric;
    use crate::optics::dispersion::IorMode;
    use crate::optics::interface::{fresnel_dielectric, refract};
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
        fn intersect(&self, ray: &Ray) -> Option<GeometryHit> {
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

    fn slab(ior: f64, max_events: usize) -> (RenderJob, PhotonTracer, Ray) {
        let mut scene = RenderJob::new();
        let object_id = scene
            .scene
            .add(Box::new(Slab), Box::new(Dielectric::new(ior).unwrap()));
        let ground_id = scene.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        scene.photon.ground_id = Some(ground_id);
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
            .trace(&scene, ray, scene.scene.intersect(&ray), &mut rng)
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
                scene.scene.intersect(&ray),
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
                .trace_state(&scene, state, scene.scene.intersect(&ray), &mut rng)
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
    fn prism_corner_transmission_does_not_cross_the_adjacent_face_during_ray_spawn() {
        use crate::geometry::vertical_prism::VerticalPrism;
        use nalgebra::Vector2;

        let mut scene = RenderJob::new();
        let object_id = scene.scene.add(
            Box::new(
                VerticalPrism::new(
                    [
                        Vector2::new(-0.6, -0.34641016151377546),
                        Vector2::new(0.6, -0.34641016151377546),
                        Vector2::new(0.0, 0.6928203230275509),
                    ],
                    0.05,
                    1.85,
                )
                .unwrap(),
            ),
            Box::new(crate::material::dielectric::Dielectric::new(1.52).unwrap()),
        );
        let ground_id = scene.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        let tracer = PhotonTracer {
            object_id,
            ground_id,
            glass_ior: 1.5156702734839476,
            max_events: 32,
        };
        let normal = Vector3::new(3.0_f64.sqrt() / 2.0, 0.5, 0.0);
        // 高采样圆盘渲染中实际触发的棱边路径：法线偏移会越过 y_min 面。
        let point = Vector3::new(0.5999885804466438, -0.34641038226716264, 1.0474086403116099)
            + normal * 1e-5;
        let inside = Vector3::new(
            -0.9527120219424032,
            0.09838193295949721,
            -0.2875079103495607,
        );
        let reverse = refract(-inside, -normal, tracer.glass_ior, 1.0).unwrap();
        let ray = Ray::new(point + reverse, -reverse).unwrap();
        let entry = scene.scene.intersect(&ray).unwrap();
        assert!((entry.coords - point).norm() < 1e-12);
        let mut state = PhotonPathState::new(ray);
        tracer
            .scatter(&mut state, entry, &mut Sequence::new(&[u64::MAX]))
            .unwrap();
        assert_eq!(state.medium, PhotonMedium::Glass);
        let exit = scene.scene.intersect(&state.ray).unwrap();
        assert_eq!(exit.object_id, object_id);
        assert!(state.ray.direction.dot(&exit.normal) > 0.0);
        // 独立重现原偏移的错误介质一侧，避免用“跳过错误面”掩盖回归。
        let displaced = Ray::new(entry.coords - entry.normal * 1e-5, state.ray.direction).unwrap();
        let wrong = scene.scene.intersect(&displaced).unwrap();
        assert!(displaced.direction.dot(&wrong.normal) < 0.0);
        let result = tracer
            .trace(
                &scene,
                ray,
                Some(entry),
                &mut Sequence::new(&[u64::MAX; 32]),
            )
            .unwrap();
        assert_eq!(result.state.medium, PhotonMedium::Air);
        assert!(result.landing().is_some());
        assert_eq!(result.state.flux_weight, 1.0);
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
                        .trace(&scene, ray, scene.scene.intersect(&ray), &mut rng)
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
        let mut scene = RenderJob::new();
        let object_id = scene.scene.add(
            Box::new(Sphere::new(Vector3::new(0.0, 0.0, 1.5), 0.8).unwrap()),
            Box::new(Dielectric::new(1.5).unwrap()),
        );
        let ground_id = scene.scene.add(
            Box::new(Ground::new(0.0).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        scene.photon.ground_id = Some(ground_id);
        scene.select_caustics_object(object_id).unwrap();
        let ray = Ray::new(Vector3::new(2.0, 0.0, 1.2), -Vector3::x()).unwrap();
        let result = crate::integrator::photon::trace_photon(
            &scene,
            ray,
            None,
            IorMode::Constant,
            1,
            &mut Sequence::new(&[0]),
        )
        .unwrap();
        assert_eq!(
            (result.state.reflections, result.state.transmissions),
            (1, 0)
        );
        assert_eq!(result.landing().unwrap().weight, 1.0);
        scene.scene.add(
            Box::new(Ground::new(0.5).unwrap()),
            Box::new(
                crate::material::lambertian::Lambertian::new(nalgebra::Vector3::repeat(0.8))
                    .unwrap(),
            ),
        );
        let result = crate::integrator::photon::trace_photon(
            &scene,
            ray,
            None,
            IorMode::Constant,
            1,
            &mut Sequence::new(&[0]),
        )
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
                .trace(
                    &scene,
                    ray,
                    scene.scene.intersect(&ray),
                    &mut Sequence::new(&[]),
                )
                .unwrap();
            assert_eq!(result.termination, expected);
            assert_eq!(result.state.event_count, 0);
        }
        let ray = Ray::new(Vector3::new(0.0, 0.0, 1.5), Vector3::z()).unwrap();
        assert!(
            tracer
                .trace(
                    &scene,
                    ray,
                    scene.scene.intersect(&ray),
                    &mut Sequence::new(&[])
                )
                .is_err()
        );
        for ior in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            tracer.glass_ior = ior;
            assert!(
                PhotonTracer::new(
                    &scene,
                    tracer.object_id,
                    tracer.ground_id,
                    tracer.glass_ior,
                    tracer.max_events
                )
                .is_err()
            );
        }
        tracer.glass_ior = 1.5;
        for limit in [0, 1025, usize::MAX] {
            tracer.max_events = limit;
            assert!(
                PhotonTracer::new(
                    &scene,
                    tracer.object_id,
                    tracer.ground_id,
                    tracer.glass_ior,
                    tracer.max_events
                )
                .is_err()
            );
        }
    }
    #[test]
    fn transmitted_slab_matches_analytic_snell_landing_without_eta_squared() {
        for ior in [1.0, 1.5, 2.0] {
            let (scene, tracer, _) = slab(ior, 2);
            for angle in [0.0_f64, 60.0, 80.0] {
                let theta = angle.to_radians();
                let ray = Ray::new(
                    Vector3::new(0.0, 0.0, 3.0),
                    Vector3::new(theta.sin(), 0.0, -theta.cos()),
                )
                .unwrap();
                let mut rng = Sequence::new(&[u64::MAX; 2]);
                let result = tracer
                    .trace(&scene, ray, scene.scene.intersect(&ray), &mut rng)
                    .unwrap();
                let sin_t = theta.sin() / ior;
                let tan_t = sin_t / (1.0 - sin_t * sin_t).sqrt();
                let analytic = Vector3::new(2.0 * theta.tan() + tan_t, 0.0, 0.0);
                assert!((result.landing().unwrap().position - analytic).norm() < 1e-12);
                assert_eq!(result.state.flux_weight, 1.0);
                assert_eq!(rng.draws, if ior == 1.0 { 0 } else { 2 });
            }
        }
    }
}

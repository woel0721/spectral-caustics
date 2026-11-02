//! Scene 的原有单元测试。
use super::*;
use crate::{
    config::sceneconfig::{CheckerboardConfig, SceneConfig},
    geometry::{hit::Hit, object::Hittable, ray::PARALLEL_EPSILON},
    optics::{iormode::IorMode, refract},
};
use nalgebra::Vector2;

#[test]
fn checkerboard_uses_world_xy_floor_for_both_sides_of_the_origin() {
    let checkerboard = Checkerboard::try_from(&CheckerboardConfig::default()).unwrap();
    for (x, y, expected) in [
        (0.25, 0.25, 1.0),
        (0.75, 0.25, 0.25),
        (0.75, 0.75, 1.0),
        (-0.25, 0.25, 0.25),
        (-0.25, -0.25, 1.0),
        (0.5, 0.25, 0.25),
        (-0.5, 0.25, 0.25),
        (-0.500000001, 0.25, 1.0),
    ] {
        assert_eq!(
            checkerboard.scale_at(Vector3::new(x, y, 2.0)),
            Some(expected)
        );
    }
    for position in [
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::INFINITY),
        Vector3::repeat(f64::MAX),
        Vector3::new((1_u64 << 53) as f64, 0.0, 0.0),
    ] {
        assert!(checkerboard.scale_at(position).is_none());
    }
    let tiny = Checkerboard::try_from(&CheckerboardConfig {
        cell_size_m: f64::MIN_POSITIVE,
        ..Default::default()
    })
    .unwrap();
    assert!(tiny.scale_at(Vector3::new(f64::MAX, 0.0, 0.0)).is_none());
}

#[test]
fn checkerboard_only_modulates_the_bound_ground() {
    let mut scene = Scene::from_config(&SceneConfig::default()).unwrap();
    let mut hit = Hit {
        coords: Vector3::new(0.75, 0.25, 0.0),
        object_id: scene.ground_id,
        ..Hit::default()
    };
    assert_eq!(scene.ground_reflectance_scale(&hit), Some(0.25));
    for object_id in [scene.prism_id, None, Some(999)] {
        hit.object_id = object_id;
        assert_eq!(scene.ground_reflectance_scale(&hit), Some(1.0));
    }
    hit.object_id = scene.ground_id;
    scene.ground_checkerboard = None;
    assert_eq!(scene.ground_reflectance_scale(&hit), Some(1.0));
}

#[test]
fn sky_gradient_uses_world_z_hdr_and_constant_background() {
    let mut scene = Scene::new();
    let sky = scene.environment.as_mut().unwrap();
    sky.horizon_color = Vector3::new(1.0, 2.0, 3.0);
    sky.zenith_color = Vector3::new(3.0, 4.0, 5.0);
    let sky = scene.environment.as_ref().unwrap();
    for (direction, expected) in [
        (Vector3::z(), sky.zenith_color),
        (Vector3::x(), sky.horizon_color),
        (-Vector3::z(), sky.horizon_color),
        (
            Vector3::new(3.0_f64.sqrt(), 0.0, 1.0),
            Vector3::new(2.0, 3.0, 4.0),
        ),
    ] {
        assert!((scene.background_radiance(direction).unwrap() - expected).norm() < 1e-12);
        assert_eq!(
            scene.background_radiance(direction),
            sky.radiance(direction)
        );
        assert!((scene.background_radiance(direction * 3.0).unwrap() - expected).norm() < 1e-12);
    }
    for direction in [
        Vector3::zeros(),
        Vector3::repeat(f64::NAN),
        Vector3::repeat(f64::MAX),
    ] {
        assert!(scene.background_radiance(direction).is_none());
    }
    scene.environment = None;
    assert_eq!(
        scene.background_radiance(Vector3::z()),
        Some(scene.background_color)
    );
    scene.background_color.x = f64::NAN;
    assert!(scene.background_radiance(Vector3::z()).is_none());
}

#[test]
fn sunlight_uses_projected_window_area_and_rejects_degenerate_inputs() {
    let mut light = Scene::new().light;
    light.direction = -Vector3::z();
    assert_eq!(light.sunlight(), Some((Vector3::z(), 0.5)));
    light.direction = Vector3::new(-3.0, 0.0, -4.0);
    let (wi, strength) = light.sunlight().unwrap();
    assert!((wi - Vector3::new(0.6, 0.0, 0.8)).norm() < 1e-12);
    assert_eq!(strength, 0.625);
    light.direction *= 10.0;
    assert_eq!(light.sunlight(), Some((wi, strength)));
    light.aperture_min_m *= 2.0;
    light.aperture_max_m *= 2.0;
    light.total_flux *= 4.0;
    assert_eq!(light.sunlight(), Some((wi, strength)));
    light.total_flux = 0.0;
    assert_eq!(light.sunlight(), Some((wi, 0.0)));
    for flux in [-1.0, f64::NAN, f64::INFINITY] {
        light.total_flux = flux;
        assert!(light.sunlight().is_none());
    }
    light.total_flux = 8.0;
    light.direction = Vector3::new(1.0, 0.0, -PARALLEL_EPSILON / 2.0);
    assert!(light.sunlight().is_none());
    light.direction = -Vector3::z();
    light.aperture_max_m = light.aperture_min_m;
    assert!(light.sunlight().is_none());
    light.aperture_max_m = Vector2::repeat(f64::INFINITY);
    assert!(light.sunlight().is_none());
}
use crate::geometry::{ground::Ground, prism::Prism, ray::Ray};

#[test]
fn constant_index_ignores_wavelength() {
    for wavelength_nm in [None, Some(450.0), Some(650.0), Some(f64::NAN)] {
        assert_eq!(
            refractive_index(wavelength_nm, IorMode::Constant),
            Some(1.52)
        );
    }
}

#[test]
fn cauchy_index_converts_nm_to_um_and_decreases_with_wavelength() {
    for (wavelength_nm, expected) in [
        (450.0, 1.5253407407407407),
        (500.0, 1.5214),
        (650.0, 1.514540828402367),
    ] {
        assert!(
            (refractive_index(Some(wavelength_nm), IorMode::Dispersion).unwrap() - expected).abs()
                < 1e-12
        );
    }
    // 第一周使用的可见光区间内，短波折射率更高，且均为有限值。
    let indices: Vec<_> = (380..=780)
        .step_by(10)
        .map(|nm| refractive_index(Some(f64::from(nm)), IorMode::Dispersion).unwrap())
        .collect();
    assert!(indices.windows(2).all(|pair| pair[0] > pair[1]));
}

#[test]
fn cauchy_index_rejects_invalid_wavelengths_and_overflow() {
    assert_eq!(refractive_index(None, IorMode::Dispersion), None);
    for wavelength_nm in [
        0.0,
        -0.0,
        -450.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MIN_POSITIVE,
        f64::from_bits(1),
    ] {
        assert_eq!(
            refractive_index(Some(wavelength_nm), IorMode::Dispersion),
            None
        );
    }
}

#[test]
fn added_objects_keep_their_own_intersection_behavior() {
    let mut scene = Scene::new();
    scene.add(Box::new(Prism::new().unwrap()));
    scene.add(Box::new(Ground::new(0.0).unwrap()));
    let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();

    // 这里只验证同一集合中的不同物体仍调用各自的求交算法。
    // 场景级的遍历、最近命中选择由 Scene::intersect 负责。
    let prism_hit = scene.objects[0].geometry.intersect(&ray).unwrap();
    let ground_hit = scene.objects[1].geometry.intersect(&ray).unwrap();
    assert!((prism_hit.coords - Vector3::new(0.25, 0.2, 1.4)).norm() < 1e-12);
    assert!((ground_hit.coords - Vector3::new(0.25, 0.2, 0.0)).norm() < 1e-12);
}

#[test]
fn scene_returns_nearest_hit_and_object_id_independently_of_order() {
    assert!(
        Scene::new()
            .intersect(
                &Ray::new(Vector3::new(0.0, 0.0, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap()
            )
            .is_none()
    );
    for ground_first in [false, true] {
        let mut scene = Scene::new();
        let (prism_id, ground_id) = if ground_first {
            let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
            (scene.add(Box::new(Prism::new().unwrap())), ground_id)
        } else {
            let prism_id = scene.add(Box::new(Prism::new().unwrap()));
            (prism_id, scene.add(Box::new(Ground::new(0.0).unwrap())))
        };
        let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
        let hit = scene.intersect(&ray).unwrap();
        assert_eq!(hit.object_id, Some(prism_id));
        assert!((hit.distance - 1.6).abs() < 1e-12);
        assert!((hit.coords - Vector3::new(0.25, 0.2, 1.4)).norm() < 1e-12);
        let ray = Ray::new(Vector3::new(1.0, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
        let hit = scene.intersect(&ray).unwrap();
        assert_eq!(hit.object_id, Some(ground_id));
        assert_eq!(hit.distance, 3.0);
    }
}

#[test]
fn single_ray_refracts_through_two_prism_surfaces_and_reaches_ground() {
    let prism = Prism::new().unwrap();
    let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
    let entry = prism.intersect(&ray).unwrap();
    assert!((entry.coords - Vector3::new(0.25, 0.2, 1.4)).norm() < 1e-12);
    assert!(ray.direction.dot(&entry.normal) < 0.0);
    let inside_direction = refract(ray.direction, entry.normal, 1.0, 1.52).unwrap();
    // 已知入射角对应 cos(theta_i)=1/sqrt(1.6²+1)，独立检查 Snell 定律。
    let sin_incident = 1.6 / (1.6_f64 * 1.6 + 1.0).sqrt();
    let sin_inside = (1.0 - inside_direction.dot(&entry.normal).powi(2)).sqrt();
    assert!((sin_incident - 1.52 * sin_inside).abs() < 1e-12);
    assert!((inside_direction.norm() - 1.0).abs() < 1e-12);
    let inside_ray = Ray::new(entry.coords - entry.normal * 1e-5, inside_direction).unwrap();
    let exit = prism.intersect(&inside_ray).unwrap();
    assert!((exit.coords.z - 1.0).abs() < 1e-12);
    assert_eq!(exit.normal, Vector3::new(0.0, 0.0, -1.0));
    assert!(inside_ray.direction.dot(&exit.normal) > 0.0);
    let outside_direction = refract(inside_ray.direction, -exit.normal, 1.52, 1.0).unwrap();
    assert!((outside_direction.norm() - 1.0).abs() < 1e-12);
    assert!((outside_direction.x - 1.52 * inside_ray.direction.x).abs() < 1e-12);
    let outside_ray = Ray::new(exit.coords + exit.normal * 1e-5, outside_direction).unwrap();
    assert!(prism.intersect(&outside_ray).is_none());
    let ground_hit = Ground::new(0.0).unwrap().intersect(&outside_ray).unwrap();
    assert!((ground_hit.coords.z).abs() < 1e-12);
    assert!((ground_hit.coords.y - 0.2).abs() < 1e-12);
    // 不加玻璃时落在 x=0.25；经过两次折射后向 -x 偏移。
    assert!((-0.8..-0.7).contains(&ground_hit.coords.x));
}

fn photon_scene() -> (Scene, usize, usize) {
    let mut scene = Scene::new();
    let prism_id = scene.add(Box::new(Prism::new().unwrap()));
    let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
    (scene, prism_id, ground_id)
}

// 两个平行界面便于使用已知 Fresnel 系数验证通量，而不依赖棱镜聚焦分布。
struct GlassSlab {
    top: Ground,
    bottom: Ground,
}

impl Hittable for GlassSlab {
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        let top = self.top.intersect(ray);
        let bottom = self.bottom.intersect(ray).map(|mut hit| {
            hit.normal = -hit.normal;
            hit
        });
        top.into_iter()
            .chain(bottom)
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}

fn slab_scene() -> (Scene, usize, usize) {
    let mut scene = Scene::new();
    let glass = scene.add(Box::new(GlassSlab {
        top: Ground::new(2.0).unwrap(),
        bottom: Ground::new(1.0).unwrap(),
    }));
    let ground = scene.add(Box::new(Ground::new(0.0).unwrap()));
    (scene, glass, ground)
}

#[test]
fn photon_landing_direct_and_index_matched_paths_keep_unit_flux() {
    let (scene, prism, ground) = photon_scene();
    let direct = Ray::new(Vector3::new(1.0, 0.2, 3.0), -Vector3::z()).unwrap();
    let landing = take_photon_landing(&scene, direct, prism, ground, || Some(1.52)).unwrap();
    assert_eq!(landing.position, Vector3::new(1.0, 0.2, 0.0));
    assert_eq!(landing.weight, 1.0);
    let through = Ray::new(Vector3::new(0.25, 0.2, 3.0), -Vector3::z()).unwrap();
    let landing = take_photon_landing(&scene, through, prism, ground, || Some(1.0)).unwrap();
    assert!((landing.position.x - 0.25).abs() < 2e-5);
    assert_eq!(landing.weight, 1.0);
}

#[test]
fn photon_landing_slab_matches_known_normal_oblique_and_grazing_transmission() {
    let (scene, glass, ground) = slab_scene();
    // eta=1.5 的独立 Fresnel 参考值；平行界面的两次能量反射率相同。
    for (angle, reflectance) in [
        (0.0_f64, 0.04_f64),
        (60.0, 0.08918671280221276),
        (80.0, 0.3877043546914726),
    ] {
        let angle = angle.to_radians();
        let ray = Ray::new(
            Vector3::new(0.0, 0.2, 3.0),
            Vector3::new(angle.sin(), 0.0, -angle.cos()),
        )
        .unwrap();
        let landing = take_photon_landing(&scene, ray, glass, ground, || Some(1.5)).unwrap();
        assert!((landing.weight - (1.0 - reflectance).powi(2)).abs() < 1e-13);
        assert!(landing.weight.is_finite() && (0.0..=1.0).contains(&landing.weight));
        assert!(landing.position.z.abs() < 1e-12);
    }
}

#[test]
fn photon_landing_dispersion_computes_a_distinct_flux_for_each_wavelength() {
    let (scene, glass, ground) = slab_scene();
    let ray = Ray::new(Vector3::new(0.0, 0.2, 3.0), -Vector3::z()).unwrap();
    let mut weights = Vec::new();
    for wavelength in [450.0, 550.0, 650.0] {
        let n = refractive_index(Some(wavelength), IorMode::Dispersion).unwrap();
        let calls = std::cell::Cell::new(0);
        let landing = take_photon_landing(&scene, ray, glass, ground, || {
            calls.set(calls.get() + 1);
            Some(n)
        })
        .unwrap();
        let r0 = ((n - 1.0) / (n + 1.0)).powi(2);
        assert!((landing.weight - (1.0 - r0).powi(2)).abs() < 1e-14);
        assert_eq!(calls.get(), 1);
        weights.push(landing.weight);
    }
    assert!(weights[0] < weights[1] && weights[1] < weights[2]);
}

#[test]
fn photon_landing_prism_uses_the_actual_incidence_at_each_interface() {
    let (scene, prism, ground) = photon_scene();
    let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), -Vector3::z()).unwrap();
    let landing = take_photon_landing(&scene, ray, prism, ground, || Some(1.52)).unwrap();
    // 独立代入右斜面与水平底面的 Snell/Fresnel 方程：
    // cos_enter=0.5299989400031799，cos_exit=0.912945573777874，两个 F 不相同。
    let f_enter = 0.08346779578839593;
    let f_exit = 0.04737538624172876;
    assert!((landing.weight - 0.8731111370340963).abs() < 1e-13);
    assert!((landing.weight - (1.0 - f_enter) * (1.0 - f_exit)).abs() < 1e-13);
    assert!((landing.position - Vector3::new(-0.719593104236608, 0.2, 0.0)).norm() < 1e-12);

    // 底面全反射时不再有确定透射落点；被遮挡的路径也不能贡献通量。
    let tir = Ray::new(Vector3::new(1.05, 0.2, 1.9), Vector3::new(-1.6, 0.0, -1.0)).unwrap();
    assert!(take_photon_landing(&scene, tir, prism, ground, || Some(1.52)).is_none());
    for height in [2.5, 1.2, 0.5] {
        let (mut blocked, prism, ground) = photon_scene();
        blocked.add(Box::new(Ground::new(height).unwrap()));
        assert!(take_photon_landing(&blocked, ray, prism, ground, || Some(1.52)).is_none());
    }
}

#[test]
fn photon_returns_direct_ground_hit_or_none_when_ray_misses() {
    let (scene, prism_id, ground_id) = photon_scene();
    let ray = Ray::new(Vector3::new(1.0, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
    assert_eq!(
        take_photon_landing(&scene, ray, prism_id, ground_id, || Some(1.52))
            .map(|landing| landing.position),
        Some(Vector3::new(1.0, 0.2, 0.0))
    );
    let ray = Ray::new(Vector3::new(1.0, 0.2, 3.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
    assert!(take_photon_landing(&scene, ray, prism_id, ground_id, || Some(1.52)).is_none());
}

#[test]
fn photon_reaches_expected_refracted_landing_point_in_either_object_order() {
    for ground_first in [false, true] {
        let mut scene = Scene::new();
        let (prism_id, ground_id) = if ground_first {
            let ground_id = scene.add(Box::new(Ground::new(0.0).unwrap()));
            (scene.add(Box::new(Prism::new().unwrap())), ground_id)
        } else {
            let prism_id = scene.add(Box::new(Prism::new().unwrap()));
            (prism_id, scene.add(Box::new(Ground::new(0.0).unwrap())))
        };
        let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
        let position = take_photon_landing(&scene, ray, prism_id, ground_id, || {
            refractive_index(None, IorMode::Constant)
        })
        .map(|landing| landing.position)
        .unwrap();
        // 与独立单光线示例的已知落点比较；直接落地时 x 应为 0.25。
        assert!((position - Vector3::new(-0.719593104236608, 0.2, 0.0)).norm() < 1e-12);
    }
}

#[test]
fn photon_uses_wavelength_dependent_index_at_both_interfaces() {
    let (scene, prism_id, ground_id) = photon_scene();
    let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
    let mut positions = Vec::new();
    // 将斜面入射与水平底面出射分别代入 Snell 定律所得的参考落点，包含原点偏移。
    for (wavelength_nm, expected_x) in [(450.0, -0.7321163297863346), (650.0, -0.7069588738883907)]
    {
        let position = take_photon_landing(&scene, ray, prism_id, ground_id, || {
            refractive_index(Some(wavelength_nm), IorMode::Dispersion)
        })
        .map(|landing| landing.position)
        .unwrap();
        assert!((position - Vector3::new(expected_x, 0.2, 0.0)).norm() < 1e-12);
        assert_eq!(
            take_photon_landing(&scene, ray, prism_id, ground_id, || {
                refractive_index(Some(wavelength_nm), IorMode::Dispersion)
            })
            .map(|landing| landing.position),
            Some(position)
        );
        let constant_position = take_photon_landing(&scene, ray, prism_id, ground_id, || {
            refractive_index(Some(wavelength_nm), IorMode::Constant)
        })
        .map(|landing| landing.position)
        .unwrap();
        assert!((constant_position - Vector3::new(-0.719593104236608, 0.2, 0.0)).norm() < 1e-12);
        assert!((position - constant_position).norm() > 0.01);
        positions.push(position);
    }
    // 蓝光偏折更大，两波长沿同一条初始空间光线产生不同落点。
    assert!(positions[0].x < positions[1].x);
}

#[test]
fn photon_rejects_invalid_wavelengths_even_for_direct_ground_hits() {
    let (scene, prism_id, ground_id) = photon_scene();
    for origin_x in [0.25, 1.0] {
        let ray = Ray::new(
            Vector3::new(origin_x, 0.2, 3.0),
            Vector3::new(0.0, 0.0, -1.0),
        )
        .unwrap();
        for wavelength_nm in [
            0.0,
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MIN_POSITIVE,
        ] {
            assert!(
                take_photon_landing(&scene, ray, prism_id, ground_id, || {
                    refractive_index(Some(wavelength_nm), IorMode::Dispersion)
                })
                .is_none()
            );
        }
    }
}

#[test]
fn photon_evaluates_a_consuming_ior_closure_once_for_both_interfaces() {
    let (scene, prism_id, ground_id) = photon_scene();
    let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
    let calls = std::cell::Cell::new(0);
    let indices = vec![1.52];
    let position = take_photon_landing(&scene, ray, prism_id, ground_id, || {
        calls.set(calls.get() + 1);
        // 消耗捕获的 Vec，使闭包只实现 FnOnce。
        indices.into_iter().next()
    })
    .map(|landing| landing.position)
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert!((position - Vector3::new(-0.719593104236608, 0.2, 0.0)).norm() < 1e-12);
}

#[test]
fn photon_rejects_missing_or_invalid_ior_even_for_direct_ground_hits() {
    let (scene, prism_id, ground_id) = photon_scene();
    for origin_x in [0.25, 1.0] {
        let ray = Ray::new(
            Vector3::new(origin_x, 0.2, 3.0),
            Vector3::new(0.0, 0.0, -1.0),
        )
        .unwrap();
        for glass_ior in [
            None,
            Some(0.0),
            Some(-1.0),
            Some(f64::NAN),
            Some(f64::INFINITY),
            Some(f64::NEG_INFINITY),
        ] {
            assert!(take_photon_landing(&scene, ray, prism_id, ground_id, || glass_ior).is_none());
        }
    }
}

#[test]
fn photon_stops_at_other_objects_before_inside_or_after_glass() {
    // 三个高度分别遮挡入射段、玻璃内部段和出射后的空气段。
    for height in [2.5, 1.2, 0.5] {
        let (mut scene, prism_id, ground_id) = photon_scene();
        scene.add(Box::new(Ground::new(height).unwrap()));
        let ray = Ray::new(Vector3::new(0.25, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
        assert!(take_photon_landing(&scene, ray, prism_id, ground_id, || Some(1.52)).is_none());
    }
}

#[test]
fn photon_terminates_on_total_internal_reflection_at_prism_bottom() {
    let (scene, prism_id, ground_id) = photon_scene();
    // 垂直于右斜面入射，进入后方向不变；与底面法线夹角约 58°，超过临界角。
    let ray = Ray::new(Vector3::new(1.05, 0.2, 1.9), Vector3::new(-1.6, 0.0, -1.0)).unwrap();
    assert_eq!(scene.intersect(&ray).unwrap().object_id, Some(prism_id));
    assert!(take_photon_landing(&scene, ray, prism_id, ground_id, || Some(1.52)).is_none());
}

#[test]
fn photon_rejects_invalid_or_identical_object_ids() {
    let (scene, prism_id, ground_id) = photon_scene();
    let ray = Ray::new(Vector3::new(1.0, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
    assert!(take_photon_landing(&scene, ray, usize::MAX, ground_id, || Some(1.52)).is_none());
    assert!(take_photon_landing(&scene, ray, prism_id, usize::MAX, || Some(1.52)).is_none());
    assert!(take_photon_landing(&scene, ray, ground_id, ground_id, || Some(1.52)).is_none());
}

#[test]
fn empty_and_configured_scenes_preserve_distinct_defaults_and_shared_fields() {
    let empty = Scene::new();
    let configured = Scene::from_config(&SceneConfig::default()).unwrap();
    assert!(empty.objects.is_empty());
    assert!(empty.material(0).is_none());
    assert_eq!((empty.prism_id, empty.ground_id), (None, None));
    assert_eq!(configured.objects.len(), 2);
    assert_eq!(
        (configured.prism_id, configured.ground_id),
        (Some(0), Some(1))
    );
    assert_eq!(empty.background_color, configured.background_color);
    assert_eq!(empty.light.direction, configured.light.direction);
    assert_eq!(empty.light.height_m, configured.light.height_m);
    assert_eq!(empty.light.aperture_min_m, configured.light.aperture_min_m);
    assert_eq!(empty.light.aperture_max_m, configured.light.aperture_max_m);
    assert_eq!(empty.light.total_flux, configured.light.total_flux);
    assert_eq!(empty.receiver.min_m, configured.receiver.min_m);
    assert_eq!(empty.receiver.max_m, configured.receiver.max_m);
    for direction in [Vector3::x(), Vector3::y(), Vector3::z()] {
        assert_eq!(
            empty.background_radiance(direction),
            configured.background_radiance(direction)
        );
    }
    let position = Vector3::new(0.75, 0.25, 0.0);
    assert_eq!(
        empty.ground_checkerboard.unwrap().scale_at(position),
        configured.ground_checkerboard.unwrap().scale_at(position)
    );
    for (x, y) in [(0, 0), (1, 1), (2, 2)] {
        let a = empty.camera.ray_for_pixel(x, y, 3, 3).unwrap();
        let b = configured.camera.ray_for_pixel(x, y, 3, 3).unwrap();
        assert_eq!(a.origin, b.origin);
        assert_eq!(a.direction, b.direction);
    }
    let explicit =
        crate::config::Config::from_json(include_str!("../../configs/default.json")).unwrap();
    let preset = Scene::from_config(&explicit.scene).unwrap();
    use crate::config::sceneconfig::EnvironmentMisHeuristic::{Balance, Power};
    assert_eq!(
        (
            empty.environment_samples,
            empty.environment_mis_heuristic,
            empty.max_diffuse_bounces
        ),
        (1, Balance, 0)
    );
    assert_eq!(
        (
            preset.environment_samples,
            preset.environment_mis_heuristic,
            preset.max_diffuse_bounces
        ),
        (8, Power, 4)
    );
    assert!(!empty.environment.as_ref().unwrap().has_map());
    assert!(preset.environment.as_ref().unwrap().has_map());
}

#[test]
fn scene_validation_still_precedes_environment_loading_and_geometry_construction() {
    let mut config = SceneConfig::default();
    config.environment.as_mut().unwrap().map_path =
        Some("missing-scene-builder-regression.hdr".into());
    config.max_diffuse_bounces = crate::config::sceneconfig::MAX_DIFFUSE_BOUNCES + 1;
    assert_eq!(
        Scene::from_config(&config).err().unwrap().to_string(),
        "scene.max_diffuse_bounces 不能超过 1024"
    );
    config.max_diffuse_bounces = 0;
    assert!(
        Scene::from_config(&config)
            .err()
            .unwrap()
            .to_string()
            .contains("missing-scene-builder-regression.hdr")
    );
    config.environment = None;
    config.objects.clear();
    assert_eq!(
        Scene::from_config(&config).err().unwrap().to_string(),
        "场景必须包含一个 ground 接收地面"
    );
}

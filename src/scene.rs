//! 场景参数与物体集合；intersect 查询所有物体中的最近交点。

use crate::{
    camera::Camera,
    config::sceneconfig::{ObjectConfig, SceneConfig},
    geometry::{ground::Ground, hit::Hit, object::Hittable, prism::Prism, ray::Ray},
    material::{self, Material},
    optics::{iormode::IorMode, refract},
};
use nalgebra::{Vector2, Vector3};
use std::error::Error;

/// 几何与场景材质编号的绑定；几何本身不依赖任何材质类型。
pub struct SceneObject {
    pub geometry: Box<dyn Hittable>,
    material_id: Option<usize>,
}

impl SceneObject {
    pub fn material_id(&self) -> Option<usize> {
        self.material_id
    }
}

pub struct Scene {
    /// 相机法线调试与直接光照使用的主光线相机；地面焦散仍从光源发射光线。
    pub camera: Camera,
    /// 相机光照使用的线性 RGB 背景色，写入 XYZ Framebuffer 前转换颜色空间。
    pub background_color: Vector3<f64>,
    /// 每项绑定一个几何和可选材质编号，查询期间不能重排集合。
    pub objects: Vec<SceneObject>,
    /// 场景拥有材质，Hit.material_id 只保存本表的下标。
    materials: Vec<Box<dyn Material>>,
    pub light: ParallelLight,
    pub receiver: GroundReceiver,
    /// 配置构建时记录的物体编号，手动 add 的场景也可显式传编号给追踪入口。
    pub prism_id: Option<usize>,
    pub ground_id: Option<usize>,
}

/// 运行时光源：使用数学向量，方向已归一化。
pub struct ParallelLight {
    pub height_m: f64,
    pub direction: Vector3<f64>,
    pub aperture_min_m: Vector2<f64>,
    pub aperture_max_m: Vector2<f64>,
    pub total_flux: f64,
}

pub struct GroundReceiver {
    pub min_m: Vector2<f64>,
    pub max_m: Vector2<f64>,
}

impl GroundReceiver {
    pub fn area(&self) -> f64 {
        let span = self.max_m - self.min_m;
        span.x * span.y
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    /// 创建空场景，使用默认相机、黑色背景。
    pub fn new() -> Self {
        let config = SceneConfig::default();
        Self {
            camera: Camera::from_config(&config.camera).expect("内置默认相机参数必须有效"),
            background_color: Vector3::zeros(),
            objects: Vec::new(),
            materials: Vec::new(),
            light: ParallelLight {
                height_m: config.light.height_m,
                direction: Vector3::from(config.light.direction),
                aperture_min_m: Vector2::from(config.light.aperture_min_m),
                aperture_max_m: Vector2::from(config.light.aperture_max_m),
                total_flux: config.light.total_flux,
            },
            receiver: GroundReceiver {
                min_m: Vector2::from(config.receiver.min_m),
                max_m: Vector2::from(config.receiver.max_m),
            },
            prism_id: None,
            ground_id: None,
        }
    }

    /// 从纯数据描述建立真实物体和数学对象；当前支持一个地面、至多一个三棱镜。
    pub fn from_config(config: &SceneConfig) -> Result<Self, Box<dyn Error>> {
        config.validate()?;
        let mut scene = Self::new();
        scene.camera = Camera::from_config(&config.camera)?;
        scene.background_color = Vector3::from(config.background_linear_rgb);
        scene.light = ParallelLight {
            height_m: config.light.height_m,
            direction: Ray::new(Vector3::zeros(), Vector3::from(config.light.direction))
                .ok_or("光源方向无法归一化")?
                .direction,
            aperture_min_m: Vector2::from(config.light.aperture_min_m),
            aperture_max_m: Vector2::from(config.light.aperture_max_m),
            total_flux: config.light.total_flux,
        };
        scene.receiver = GroundReceiver {
            min_m: Vector2::from(config.receiver.min_m),
            max_m: Vector2::from(config.receiver.max_m),
        };
        for object in &config.objects {
            match object {
                ObjectConfig::Prism {
                    vertices_m,
                    material,
                } => {
                    if scene.prism_id.is_some() {
                        return Err("当前渲染只支持一个三棱镜".into());
                    }
                    let prism = Prism::from_vertices(vertices_m.map(Vector3::from))
                        .ok_or("三棱镜顶点无效：需要按示例顺序描述非退化、朝外法线的凸三棱镜")?;
                    scene.prism_id = Some(
                        scene.add_with_material(Box::new(prism), material::from_config(material)?),
                    );
                }
                ObjectConfig::Ground { height_m, material } => {
                    if scene.ground_id.is_some() {
                        return Err("当前场景只支持一个接收地面".into());
                    }
                    if *height_m >= config.light.height_m {
                        return Err("光源发射面必须高于接收地面".into());
                    }
                    scene.ground_id = Some(scene.add_with_material(
                        Box::new(Ground::new(*height_m).ok_or("地面高度无效")?),
                        material::from_config(material)?,
                    ));
                }
            }
        }
        if scene.ground_id.is_none() {
            return Err("场景必须包含一个 ground 接收地面".into());
        }
        Ok(scene)
    }

    /// 将物体的所有权交给场景，返回用于 Hit.object_id 的集合下标。
    pub fn add(&mut self, object: Box<dyn Hittable>) -> usize {
        let id = self.objects.len();
        self.objects.push(SceneObject {
            geometry: object,
            material_id: None,
        });
        id
    }

    /// 添加带材质的物体；接受任意实现 Material 的具体类型。
    pub fn add_with_material(
        &mut self,
        object: Box<dyn Hittable>,
        material: Box<dyn Material>,
    ) -> usize {
        let object_id = self.add(object);
        let material_id = self.materials.len();
        self.materials.push(material);
        self.objects[object_id].material_id = Some(material_id);
        object_id
    }

    /// 通过 Hit.material_id 查询配置构建的运行时材质；非法编号返回 None。
    pub fn material(&self, material_id: usize) -> Option<&dyn Material> {
        self.materials.get(material_id).map(Box::as_ref)
    }

    /// 返回最近命中，并填写 object_id 与可选的 material_id。
    pub fn intersect(&self, ray: &Ray) -> Option<Hit> {
        self.objects
            .iter()
            .enumerate()
            .filter_map(|(id, object)| {
                object.geometry.intersect(ray).map(|mut hit| {
                    hit.object_id = Some(id);
                    hit.material_id = object.material_id;
                    hit
                })
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}

/// Constant 忽略波长并返回 1.52；Dispersion 要求 Some(wavelength_nm)。
/// Cauchy 色散模型：n(λ) = A + B / λ_μm²，输入波长单位为 nm。
/// A = 1.5046、B = 0.0042 μm² 是 ROADMAP 中的演示参数，不代表特定真实玻璃。
/// 非有限、非正波长或计算溢出返回 None。
pub fn refractive_index(wavelength_nm: Option<f64>, mode: IorMode) -> Option<f64> {
    const DEFAULT_GLASS_IOR: f64 = 1.52;
    const A: f64 = 1.5046;
    const B: f64 = 0.0042;

    let wavelength_nm = match mode {
        IorMode::Constant => return Some(DEFAULT_GLASS_IOR),
        IorMode::Dispersion => wavelength_nm?,
    };
    if !wavelength_nm.is_finite() || wavelength_nm <= 0.0 {
        return None;
    }
    let wavelength_um = wavelength_nm / 1000.0;
    let index = A + B / wavelength_um.powi(2);
    index.is_finite().then_some(index)
}

/// 追踪从空气出发的单条光线，返回最终地面落点。
/// glass_ior 是无参数闭包，例如 || refractive_index(Some(450.0), IorMode::Dispersion)。
/// 闭包只调用一次，返回的正有限折射率用于两个界面；也可传入 || Some(1.52)。
/// 只支持一个三棱镜的两次透射；闭包返回 None、折射率非法、全反射、其他遮挡或未命中地面时返回 None。
pub fn take_photon(
    scene: &Scene,
    ray: Ray,
    prism_id: usize,
    ground_id: usize,
    glass_ior: impl FnOnce() -> Option<f64>,
) -> Option<Vector3<f64>> {
    let glass_ior = glass_ior()?;
    if !glass_ior.is_finite() || glass_ior <= 0.0 {
        return None;
    }
    if prism_id == ground_id || prism_id >= scene.objects.len() || ground_id >= scene.objects.len()
    {
        return None;
    }

    let first_hit = scene.intersect(&ray)?;
    take_photon_from_hit(scene, ray, first_hit, prism_id, ground_id, glass_ior)
}

/// 从已知首次交点继续追踪，供批量波长追踪复用同一份求交结果。
/// 调用方须校验物体编号和折射率，并传入同一 ray 在该场景中的最近交点。
pub(crate) fn take_photon_from_hit(
    scene: &Scene,
    ray: Ray,
    first_hit: Hit,
    prism_id: usize,
    ground_id: usize,
    glass_ior: f64,
) -> Option<Vector3<f64>> {
    const AIR_IOR: f64 = 1.0;
    const EPSILON: f64 = 1e-5;

    // 没有经过玻璃，直接落地。
    if first_hit.object_id == Some(ground_id) {
        return Some(first_hit.coords);
    }
    if first_hit.object_id != Some(prism_id) {
        return None;
    }

    // 空气 -> 玻璃：朝外法线朝向入射介质。
    let inside_direction = refract(ray.direction, first_hit.normal, AIR_IOR, glass_ior)?;
    let inside_ray = Ray::new(
        first_hit.coords - first_hit.normal * EPSILON,
        inside_direction,
    )?;

    let exit_hit = scene.intersect(&inside_ray)?;
    if exit_hit.object_id != Some(prism_id) {
        return None;
    }

    // 玻璃 -> 空气：法线取反，朝向玻璃内部；全反射由 ? 提前返回 None。
    let outside_direction = refract(inside_ray.direction, -exit_hit.normal, glass_ior, AIR_IOR)?;
    let outside_ray = Ray::new(
        exit_hit.coords + exit_hit.normal * EPSILON,
        outside_direction,
    )?;

    let ground_hit = scene.intersect(&outside_ray)?;
    if ground_hit.object_id == Some(ground_id) {
        Some(ground_hit.coords)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                (refractive_index(Some(wavelength_nm), IorMode::Dispersion).unwrap() - expected)
                    .abs()
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

    #[test]
    fn photon_returns_direct_ground_hit_or_none_when_ray_misses() {
        let (scene, prism_id, ground_id) = photon_scene();
        let ray = Ray::new(Vector3::new(1.0, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
        assert_eq!(
            take_photon(&scene, ray, prism_id, ground_id, || Some(1.52)),
            Some(Vector3::new(1.0, 0.2, 0.0))
        );
        let ray = Ray::new(Vector3::new(1.0, 0.2, 3.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        assert!(take_photon(&scene, ray, prism_id, ground_id, || Some(1.52)).is_none());
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
            let position = take_photon(&scene, ray, prism_id, ground_id, || {
                refractive_index(None, IorMode::Constant)
            })
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
        for (wavelength_nm, expected_x) in
            [(450.0, -0.7321163297863346), (650.0, -0.7069588738883907)]
        {
            let position = take_photon(&scene, ray, prism_id, ground_id, || {
                refractive_index(Some(wavelength_nm), IorMode::Dispersion)
            })
            .unwrap();
            assert!((position - Vector3::new(expected_x, 0.2, 0.0)).norm() < 1e-12);
            assert_eq!(
                take_photon(&scene, ray, prism_id, ground_id, || {
                    refractive_index(Some(wavelength_nm), IorMode::Dispersion)
                }),
                Some(position)
            );
            let constant_position = take_photon(&scene, ray, prism_id, ground_id, || {
                refractive_index(Some(wavelength_nm), IorMode::Constant)
            })
            .unwrap();
            assert!(
                (constant_position - Vector3::new(-0.719593104236608, 0.2, 0.0)).norm() < 1e-12
            );
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
                    take_photon(&scene, ray, prism_id, ground_id, || {
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
        let position = take_photon(&scene, ray, prism_id, ground_id, || {
            calls.set(calls.get() + 1);
            // 消耗捕获的 Vec，使闭包只实现 FnOnce。
            indices.into_iter().next()
        })
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
                assert!(take_photon(&scene, ray, prism_id, ground_id, || glass_ior).is_none());
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
            assert!(take_photon(&scene, ray, prism_id, ground_id, || Some(1.52)).is_none());
        }
    }

    #[test]
    fn photon_terminates_on_total_internal_reflection_at_prism_bottom() {
        let (scene, prism_id, ground_id) = photon_scene();
        // 垂直于右斜面入射，进入后方向不变；与底面法线夹角约 58°，超过临界角。
        let ray = Ray::new(Vector3::new(1.05, 0.2, 1.9), Vector3::new(-1.6, 0.0, -1.0)).unwrap();
        assert_eq!(scene.intersect(&ray).unwrap().object_id, Some(prism_id));
        assert!(take_photon(&scene, ray, prism_id, ground_id, || Some(1.52)).is_none());
    }

    #[test]
    fn photon_rejects_invalid_or_identical_object_ids() {
        let (scene, prism_id, ground_id) = photon_scene();
        let ray = Ray::new(Vector3::new(1.0, 0.2, 3.0), Vector3::new(0.0, 0.0, -1.0)).unwrap();
        assert!(take_photon(&scene, ray, usize::MAX, ground_id, || Some(1.52)).is_none());
        assert!(take_photon(&scene, ray, prism_id, usize::MAX, || Some(1.52)).is_none());
        assert!(take_photon(&scene, ray, ground_id, ground_id, || Some(1.52)).is_none());
    }
}

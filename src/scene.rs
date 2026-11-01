//! 场景参数与物体集合；intersect 查询所有物体中的最近交点。

use crate::{
    camera::Camera,
    config::sceneconfig::{CheckerboardConfig, EnvironmentMisHeuristic, ObjectConfig, SceneConfig},
    geometry::{
        ground::Ground,
        hit::Hit,
        object::Hittable,
        prism::Prism,
        ray::{PARALLEL_EPSILON, Ray},
    },
    material::{self, Material, dielectric::fresnel_dielectric},
    optics::{iormode::IorMode, refract},
};
use nalgebra::{Vector2, Vector3};
use std::error::Error;

// 保留原 scene::Environment 的公共访问路径。
pub use crate::environment::Environment;

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
    /// 关闭天空时使用的线性 RGB 常量背景色，写入 Framebuffer 前转换为 XYZ。
    pub background_color: Vector3<f64>,
    /// 独立于解析太阳的环境；None 使用原常量背景且没有环境照明。
    pub environment: Option<Environment>,
    /// 每个漫反射终点的环境方向样本数，与相机 SPP 独立。
    pub environment_samples: usize,
    /// HDRI 固定配额的 MIS 启发式，不影响单样本或渐变天空。
    pub environment_mis_heuristic: EnvironmentMisHeuristic,
    /// 只调制唯一接收地面的漫反射率，光源辐照度图保持独立。
    pub ground_checkerboard: Option<Checkerboard>,
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

/// 世界 x/y 坐标中的程序化灰度棋盘格，输出地面 albedo 的乘数。
#[derive(Debug, Clone, Copy)]
pub struct Checkerboard {
    cell_size_m: f64,
    even_scale: f64,
    odd_scale: f64,
}

impl TryFrom<&CheckerboardConfig> for Checkerboard {
    type Error = String;

    fn try_from(config: &CheckerboardConfig) -> Result<Self, Self::Error> {
        config.validate()?;
        Ok(Self {
            cell_size_m: config.cell_size_m,
            even_scale: config.even_scale,
            odd_scale: config.odd_scale,
        })
    }
}

impl Checkerboard {
    /// 每格为 [i*s,(i+1)*s)，负坐标同样向下取整；不使用会饱和的整数转换。
    /// 拒绝非法坐标、除法溢出或超出 f64 整数奇偶可分辨范围的格子。
    pub fn scale_at(&self, position: Vector3<f64>) -> Option<f64> {
        if !position.iter().all(|value| value.is_finite()) {
            return None;
        }
        let x = (position.x / self.cell_size_m).floor();
        let y = (position.y / self.cell_size_m).floor();
        const MAX_EXACT_CELL: f64 = (1_u64 << 53) as f64;
        if !x.is_finite()
            || !y.is_finite()
            || x.abs() >= MAX_EXACT_CELL
            || y.abs() >= MAX_EXACT_CELL
        {
            return None;
        }
        Some(if x.rem_euclid(2.0) == y.rem_euclid(2.0) {
            self.even_scale
        } else {
            self.odd_scale
        })
    }
}

/// 平行太阳光；水平窗口仅用于有限数量的光子采样，不裁切解析直射光。
pub struct ParallelLight {
    pub height_m: f64,
    pub direction: Vector3<f64>,
    pub aperture_min_m: Vector2<f64>,
    pub aperture_max_m: Vector2<f64>,
    /// 穿过水平窗口的相对通量 Phi，D65 积分以 Y=1 归一化，无绝对瓦特标定。
    /// 水平面未遮挡辐照度为 Phi/A；垂直于光束的辐照度为 Phi/(A*|direction.z|)。
    pub total_flux: f64,
}

impl ParallelLight {
    /// 阶段 14 的太阳光约定：(指向太阳的单位方向，光束法向相对辐照度)。
    /// 水平发射面要求光束向下；水平/近水平/向上方向无法表示此约定，返回 None。
    pub fn sunlight(&self) -> Option<(Vector3<f64>, f64)> {
        let length = self.direction.norm();
        let span = self.aperture_max_m - self.aperture_min_m;
        let area = span.x * span.y;
        if !self.height_m.is_finite()
            || !length.is_finite()
            || length == 0.0
            || self
                .aperture_min_m
                .iter()
                .chain(self.aperture_max_m.iter())
                .any(|v| !v.is_finite())
            || span.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || !area.is_finite()
            || area <= 0.0
            || !self.total_flux.is_finite()
            || self.total_flux < 0.0
        {
            return None;
        }
        let wi = -self.direction / length;
        if wi.z <= PARALLEL_EPSILON {
            return None;
        }
        let irradiance = (self.total_flux / area) / wi.z;
        irradiance.is_finite().then_some((wi, irradiance))
    }
}

#[derive(Debug, Clone, Copy)]
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
    /// 创建空场景，使用默认配置中的相机、天空与备用常量背景。
    pub fn new() -> Self {
        let config = SceneConfig::default();
        Self {
            camera: Camera::from_config(&config.camera).expect("内置默认相机参数必须有效"),
            background_color: Vector3::from(config.background_linear_rgb),
            environment: config
                .environment
                .as_ref()
                .map(Environment::try_from)
                .transpose()
                .expect("内置默认环境参数必须有效"),
            environment_samples: config.environment_samples,
            environment_mis_heuristic: config.environment_mis_heuristic,
            ground_checkerboard: config
                .ground_checkerboard
                .as_ref()
                .map(Checkerboard::try_from)
                .transpose()
                .expect("内置默认棋盘格参数必须有效"),
            objects: Vec::new(),
            materials: Vec::new(),
            light: ParallelLight {
                height_m: config.light.height_m,
                direction: Ray::new(Vector3::zeros(), Vector3::from(config.light.direction))
                    .expect("内置默认光源方向必须有效")
                    .direction,
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
        // 每次场景构建只加载一次 HDR；所有背景和照明查询共用内存中的像素。
        scene.environment = config
            .environment
            .as_ref()
            .map(Environment::try_from)
            .transpose()?;
        scene.environment_samples = config.environment_samples;
        scene.environment_mis_heuristic = config.environment_mis_heuristic;
        scene.ground_checkerboard = config
            .ground_checkerboard
            .as_ref()
            .map(Checkerboard::try_from)
            .transpose()?;
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
        let ground_bounds = config.ground_bounds()?;
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
                    scene.ground_id = Some(
                        scene.add_with_material(
                            Box::new(
                                Ground::with_bounds(
                                    *height_m,
                                    Vector2::from(ground_bounds.min_m),
                                    Vector2::from(ground_bounds.max_m),
                                )
                                .ok_or("地面高度或范围无效")?,
                            ),
                            material::from_config(material)?,
                        ),
                    );
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

    /// 在相机的漫反射终点调用；反射率调制与光照线性等价，不能写回入射 E 图。
    pub fn ground_reflectance_scale(&self, hit: &Hit) -> Option<f64> {
        if self.ground_id.is_some()
            && hit.object_id == self.ground_id
            && let Some(checkerboard) = &self.ground_checkerboard
        {
            return checkerboard.scale_at(hit.coords);
        }
        Some(1.0)
    }

    /// 使用当前射线方向查询天空；关闭天空时回退到旧常量背景。
    pub fn background_radiance(&self, direction: Vector3<f64>) -> Option<Vector3<f64>> {
        match &self.environment {
            Some(environment) => environment.radiance(direction),
            None => self
                .background_color
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)
                .then_some(self.background_color),
        }
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

/// 正向光子路径的落地结果；weight 是到达地面的通量比例，不是相机 radiance 权重。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotonLanding {
    pub position: Vector3<f64>,
    /// 直射为 1；双透射为 (1-F_enter)*(1-F_exit)，不含折射率平方因子。
    pub weight: f64,
}

/// 兼容的几何落点接口；能量累积应使用 take_photon_landing 获取透射权重。
pub fn take_photon(
    scene: &Scene,
    ray: Ray,
    prism_id: usize,
    ground_id: usize,
    glass_ior: impl FnOnce() -> Option<f64>,
) -> Option<Vector3<f64>> {
    take_photon_landing(scene, ray, prism_id, ground_id, glass_ior).map(|landing| landing.position)
}

/// 追踪从空气出发的单条光线，返回地面落点及确定双透射路径的通量权重。
/// glass_ior 是无参数闭包，例如 || refractive_index(Some(450.0), IorMode::Dispersion)。
/// 闭包只调用一次，返回的正有限折射率用于两个界面；也可传入 || Some(1.52)。
/// 只支持一个三棱镜的两次透射；闭包返回 None、折射率非法、全反射、其他遮挡或未命中地面时返回 None。
/// 被 Fresnel 反射的通量暂不追踪、不重新分配给透射路径。
pub fn take_photon_landing(
    scene: &Scene,
    ray: Ray,
    prism_id: usize,
    ground_id: usize,
    glass_ior: impl FnOnce() -> Option<f64>,
) -> Option<PhotonLanding> {
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
) -> Option<PhotonLanding> {
    const AIR_IOR: f64 = 1.0;
    const EPSILON: f64 = 1e-5;

    // 没有经过玻璃，直接落地。
    if first_hit.object_id == Some(ground_id) {
        return Some(PhotonLanding {
            position: first_hit.coords,
            weight: 1.0,
        });
    }
    if first_hit.object_id != Some(prism_id) {
        return None;
    }

    // 空气 -> 玻璃：朝外法线朝向入射介质。
    let cos_enter = -ray.direction.dot(&first_hit.normal);
    let f_enter = fresnel_dielectric(cos_enter, AIR_IOR, glass_ior)?;
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
    // 入射方向来自玻璃内部，故 cos_exit = -d_inside·(-n_outward)。
    let cos_exit = inside_ray.direction.dot(&exit_hit.normal);
    let f_exit = fresnel_dielectric(cos_exit, glass_ior, AIR_IOR)?;
    let outside_direction = refract(inside_ray.direction, -exit_hit.normal, glass_ior, AIR_IOR)?;
    let outside_ray = Ray::new(
        exit_hit.coords + exit_hit.normal * EPSILON,
        outside_direction,
    )?;

    let ground_hit = scene.intersect(&outside_ray)?;
    if ground_hit.object_id == Some(ground_id) {
        Some(PhotonLanding {
            position: ground_hit.coords,
            weight: (1.0 - f_enter) * (1.0 - f_exit),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn sky_gradient_uses_world_z_and_keeps_hdr_and_legacy_background() {
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
            assert!(
                (scene.background_radiance(direction * 3.0).unwrap() - expected).norm() < 1e-12
            );
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
            assert_eq!(
                Some(landing.position),
                take_photon(&scene, ray, glass, ground, || Some(1.5))
            );
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

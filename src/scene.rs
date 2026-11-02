//! 场景主类型；构建、物体、光照、棋盘格与正向光子追踪按职责实现。

mod builder;
mod checkerboard;
mod lighting;
mod objects;
mod photon;

use crate::{camera::Camera, config::sceneconfig::EnvironmentMisHeuristic, material::Material};
use nalgebra::Vector3;

pub use checkerboard::Checkerboard;
pub use lighting::{Environment, GroundReceiver, ParallelLight};
pub use objects::SceneObject;
pub(crate) use photon::take_photon_from_hit;
pub use photon::{PhotonLanding, refractive_index, take_photon_landing};

pub struct Scene {
    /// 玻璃与漫反射路径使用的主光线相机；地面焦散仍从光源发射光线。
    pub camera: Camera,
    /// 关闭天空时使用的线性 RGB 常量背景色，写入 Framebuffer 前转换为 XYZ。
    pub background_color: Vector3<f64>,
    /// 独立于解析太阳的环境；None 使用原常量背景且没有环境照明。
    pub environment: Option<Environment>,
    /// 每个漫反射终点的环境方向样本数，与相机 SPP 独立。
    pub environment_samples: usize,
    /// HDRI 固定配额的 MIS 启发式，不影响单样本或渐变天空。
    pub environment_mis_heuristic: EnvironmentMisHeuristic,
    /// 有限的额外 Lambertian 反弹数；独立于玻璃上限，缺省为 0。
    pub max_diffuse_bounces: usize,
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

#[cfg(test)]
mod tests;

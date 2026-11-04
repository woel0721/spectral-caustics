//! 有限面光源的面积 NEE；RGB/光谱共享几何与有限线段可见性。
//! 圆盘不注册为可命中的发光几何，故此项不与 BSDF 命中或环境 MIS 重叠。

use super::offset_ray_origin;
use crate::{
    geometry::{hit::Hit, ray::Ray},
    material::{BsdfValue, EvaluateMode, Material, unit_vector},
    optics::emission::{d65_spectrum, d65_white_xyz},
    render::utils::xyz_to_linear_srgb,
    scene::Scene,
};
use nalgebra::Vector3;
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::{error::Error, sync::LazyLock};

static D65_RGB: LazyLock<Vector3<f64>> =
    LazyLock::new(|| xyz_to_linear_srgb(d65_white_xyz()).expect("D65 白点须可转换"));

pub(super) struct AreaLightSampler(Option<StdRng>);

impl AreaLightSampler {
    /// 仅圆盘路径从前缀抽一次固定种子，面积采样在独立流内进行。
    /// 改变 NEE 配额不改变玻璃、首次环境或续追序列；Packet CRN 的各 Lane 得到同一种子。
    /// 旧 Parallel 路径不抽随机数，保留其序列。
    pub(super) fn new(scene: &Scene, prefix: &mut dyn rand::RngCore) -> Self {
        Self(
            scene
                .light
                .as_disk()
                .map(|_| StdRng::seed_from_u64(prefix.next_u64())),
        )
    }

    pub(super) fn lighting(
        &mut self,
        scene: &Scene,
        hit: &Hit,
        material: &dyn Material,
        wo: Vector3<f64>,
        mode: EvaluateMode,
    ) -> Result<BsdfValue, Box<dyn Error>> {
        let mut rgb = Vector3::zeros();
        let mut spectral = 0.0;
        if let Some(rng) = &mut self.0 {
            let disk = scene.light.as_disk().ok_or("面积光源采样器与场景不匹配")?;
            if scene.light_samples == 0 {
                return Err("面积光源样本数须为正".into());
            }
            let normal = unit_vector(hit.normal).ok_or("面积光源着色法线无效")?;
            for _ in 0..scene.light_samples {
                let Some(sample) = disk.sample_direct(hit.coords, [rng.random(), rng.random()])?
                else {
                    continue;
                };
                let cosine = normal.dot(&sample.wi).max(0.0);
                if cosine == 0.0 || sample.incident_weight == 0.0 {
                    continue;
                }
                let factor = sample.incident_weight * cosine / scene.light_samples as f64;
                if !factor.is_finite() {
                    return Err("面积光源几何项溢出".into());
                }
                if !visible_to_point(scene, hit.coords, normal, sample.position)? {
                    continue;
                }
                match material.evaluate(sample.wi, wo, normal, mode) {
                    BsdfValue::Rgb(f) if matches!(mode, EvaluateMode::Rgb) => {
                        if f.iter().any(|v| !v.is_finite() || *v < 0.0) {
                            return Err("面积光源 RGB BSDF 无效".into());
                        }
                        rgb += f.component_mul(&D65_RGB) * factor;
                    }
                    BsdfValue::Spectral(f) if matches!(mode, EvaluateMode::Spectral { .. }) => {
                        if !f.is_finite() || f < 0.0 {
                            return Err("面积光源光谱 BSDF 无效".into());
                        }
                        let EvaluateMode::Spectral { wavelength_nm } = mode else {
                            unreachable!()
                        };
                        spectral +=
                            f * factor * d65_spectrum(wavelength_nm).ok_or("面积光源 D65 无效")?;
                    }
                    _ => return Err("面积光源 BSDF 模式不匹配".into()),
                }
            }
        }
        if rgb.iter().any(|v| !v.is_finite()) || !spectral.is_finite() {
            return Err("面积光源辐亮度溢出".into());
        }
        Ok(match mode {
            EvaluateMode::Rgb => BsdfValue::Rgb(rgb),
            EvaluateMode::Spectral { .. } => BsdfValue::Spectral(spectral),
        })
    }
}

fn visible_to_point(
    scene: &Scene,
    position: Vector3<f64>,
    normal: Vector3<f64>,
    target: Vector3<f64>,
) -> Result<bool, Box<dyn Error>> {
    let origin = offset_ray_origin(position, normal, 1.0);
    let delta = target - origin;
    let distance = delta.norm();
    let ray = Ray::new(origin, delta).ok_or("面积光源阴影线段无效")?;
    Ok(scene
        .intersect(&ray)
        .is_none_or(|hit| hit.distance >= distance))
}

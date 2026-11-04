use super::super::{
    direct::visible_to_light,
    environment::{ENVIRONMENT_BSDF_FRACTION, power_heuristic},
};
use crate::{
    config::sceneconfig::EnvironmentMisHeuristic,
    geometry::hit::Hit,
    material::{BsdfValue, EvaluateMode, Material, unit_vector},
    optics::{
        emission::{d65_spectrum, rgb_emission},
        iormode::IorMode,
    },
    scene::Scene,
};
use nalgebra::Vector3;
use rand::Rng;
use std::error::Error;

pub(super) fn evaluate(
    material: &dyn Material,
    wi: Vector3<f64>,
    wo: Vector3<f64>,
    normal: Vector3<f64>,
    wavelength_nm: f64,
) -> Result<f64, Box<dyn Error>> {
    let BsdfValue::Spectral(f) =
        material.evaluate(wi, wo, normal, EvaluateMode::Spectral { wavelength_nm })
    else {
        return Err("光谱求值必须返回标量 BSDF".into());
    };
    if !f.is_finite() || f < 0.0 {
        return Err("光谱 BSDF 必须有限且非负".into());
    }
    Ok(f)
}

pub(super) fn direct_sun(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    wavelength_nm: f64,
) -> Result<f64, Box<dyn Error>> {
    let Some((wi, strength)) = scene.light.analytic_sun()? else {
        return Ok(0.0);
    };
    let normal = unit_vector(hit.normal).ok_or("光谱表面法线无效")?;
    let cosine = normal.dot(&wi).max(0.0);
    let f = evaluate(material, wi, wo, normal, wavelength_nm)?;
    if f == 0.0 || cosine == 0.0 || strength == 0.0 {
        return Ok(0.0);
    }
    if !visible_to_light(scene, hit.coords, normal, wi)? {
        return Ok(0.0);
    }
    let result = f * cosine * strength * d65_spectrum(wavelength_nm).ok_or("太阳光谱无效")?;
    if !result.is_finite() {
        return Err("光谱太阳直射溢出".into());
    }
    Ok(result)
}

/// 保留 RGB 的固定配额 q_i=n_i*p_i、N=1 随机混合、渐变纯 BSDF 的约定。
/// HDRI 的亮度方向分布只是 proposal；BSDF 与 Li 则均在当前 λ 求值。
pub(super) fn environment_lighting(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    wavelength_nm: f64,
    ior_mode: IorMode,
    rng: &mut dyn rand::RngCore,
) -> Result<f64, Box<dyn Error>> {
    let Some(environment) = &scene.environment else {
        return Ok(0.0);
    };
    let normal = unit_vector(hit.normal).ok_or("光谱环境法线无效")?;
    let wo = unit_vector(wo).ok_or("光谱环境出射方向无效")?;
    let samples = scene.environment_samples;
    if samples == 0 {
        return Err("环境样本数必须为正".into());
    }
    let has_map = environment.has_map();
    let stratified = has_map && samples >= 2;
    let power = stratified && scene.environment_mis_heuristic == EnvironmentMisHeuristic::Power;
    let bsdf_count = samples / 2;
    let average_count = if stratified { 1.0 } else { samples as f64 };
    let mut result = 0.0;
    for index in 0..samples {
        let use_bsdf = if stratified {
            index < bsdf_count
        } else {
            !has_map || rng.random::<f64>() < ENVIRONMENT_BSDF_FRACTION
        };
        let wi = if use_bsdf {
            let Some(sample) = material.sample_spectral(wo, normal, wavelength_nm, ior_mode, rng)
            else {
                continue;
            };
            if sample.event.is_delta() {
                continue;
            }
            if sample.pdf.is_none_or(|p| !p.is_finite() || p <= 0.0) {
                return Err("光谱环境 BSDF 样本 PDF 无效".into());
            }
            sample.wi
        } else {
            environment
                .sample_direction(rng)
                .ok_or("HDRI 方向采样失败")?
                .direction
        };
        let wi = unit_vector(wi).ok_or("光谱环境采样方向无效")?;
        let p_b = material.pdf_spectral(wi, wo, normal, wavelength_nm);
        if !p_b.is_finite() || p_b < 0.0 {
            return Err("光谱 BSDF PDF 无效".into());
        }
        let inverse_pdf = if has_map {
            let p_e = environment.pdf_direction(wi).ok_or("HDRI PDF 无效")?;
            if !p_e.is_finite() || p_e < 0.0 {
                return Err("HDRI PDF 无效".into());
            }
            let (n_b, n_e) = if stratified {
                (bsdf_count as f64, (samples - bsdf_count) as f64)
            } else {
                (ENVIRONMENT_BSDF_FRACTION, 1.0 - ENVIRONMENT_BSDF_FRACTION)
            };
            let (q_b, q_e) = (n_b * p_b, n_e * p_e);
            if power {
                let (sampled, other) = if use_bsdf { (q_b, q_e) } else { (q_e, q_b) };
                power_heuristic(sampled, other)
                    .ok_or("光谱 Power MIS 无效")?
                    .1
            } else {
                let denominator = q_b + q_e;
                if !denominator.is_finite() || denominator <= 0.0 {
                    return Err("光谱 Balance MIS 分母无效".into());
                }
                1.0 / denominator
            }
        } else {
            if p_b <= 0.0 {
                return Err("光谱环境 PDF 必须为正".into());
            }
            1.0 / p_b
        };
        if !inverse_pdf.is_finite() {
            return Err("光谱 MIS 权重溢出".into());
        }
        let cosine = normal.dot(&wi).max(0.0);
        // 被挡或背半球的样本保持原始配额，不重采样或重新归一化。
        if cosine == 0.0 || !visible_to_light(scene, hit.coords, normal, wi)? {
            continue;
        }
        let f = evaluate(material, wi, wo, normal, wavelength_nm)?;
        let li = rgb_emission(
            environment.radiance(wi).ok_or("环境 RGB 辐亮度无效")?,
            wavelength_nm,
        )
        .ok_or("HDR 发光光谱重建失败或溢出")?;
        result += f * cosine * li * inverse_pdf / average_count;
    }
    if !result.is_finite() || result < 0.0 {
        return Err("光谱环境照明溢出".into());
    }
    Ok(result)
}

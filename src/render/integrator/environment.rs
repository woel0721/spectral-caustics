//! 环境方向积分、固定配额 Balance/Power MIS 与方向贡献。

use super::direct::visible_to_light;
use crate::{
    config::sceneconfig::EnvironmentMisHeuristic,
    geometry::hit::Hit,
    material::{BsdfValue, EvaluateMode, Material, unit_vector},
    scene::Scene,
};
use nalgebra::Vector3;
use rand::Rng;
use std::error::Error;

// HDRI 环境的方向策略概率；Environment 内部的 95%/5% 分布已经包含在 p_env 中。
pub(super) const ENVIRONMENT_BSDF_FRACTION: f64 = 0.5;

#[derive(Clone, Copy)]
enum EnvironmentStrategy {
    Bsdf,
    Environment,
}

#[derive(Clone, Copy)]
struct EnvironmentMisWeights {
    bsdf: f64,
    environment: f64,
    /// None 使用 Balance；Power 必须知道当前样本由哪种策略生成。
    power_source: Option<EnvironmentStrategy>,
}

/// HDRI 多样本用固定配额 Balance/Power MIS；单样本使用随机 50/50。
/// 渐变天空仍用纯 BSDF 采样；零贡献样本不补采，也不重新分配配额。
/// 仅查询环境，不包含太阳或地面光子图；玻璃也按二值环境遮挡处理。
pub(super) fn environment_lighting(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    rng: &mut dyn rand::RngCore,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let Some(environment) = &scene.environment else {
        return Ok(Vector3::zeros());
    };
    if !material.has_non_delta() {
        return Ok(Vector3::zeros());
    }
    if scene.environment_samples == 0 {
        return Err("环境方向样本数必须大于零".into());
    }
    let normal = unit_vector(hit.normal).ok_or("环境采样的命中法线无效")?;
    let wo = unit_vector(wo).ok_or("环境采样的出射方向无效")?;
    let has_map = environment.has_map();
    let samples = scene.environment_samples;
    let stratified = has_map && samples >= 2;
    let use_power = stratified && scene.environment_mis_heuristic == EnvironmentMisHeuristic::Power;
    let bsdf_samples = samples / 2;
    let pdf_weights = has_map.then_some(if stratified {
        (bsdf_samples as f64, (samples - bsdf_samples) as f64)
    } else {
        (ENVIRONMENT_BSDF_FRACTION, 1.0 - ENVIRONMENT_BSDF_FRACTION)
    });
    // 固定配额时 PDF 权重已包含样本数，贡献求和后不能再除以 N。
    let average_count = if stratified { 1.0 } else { samples as f64 };
    let mut rgb = Vector3::zeros();
    for index in 0..samples {
        let use_bsdf = if stratified {
            index < bsdf_samples
        } else {
            // N=1 随机选择策略；渐变不消耗额外随机数。
            !has_map || rng.random::<f64>() < ENVIRONMENT_BSDF_FRACTION
        };
        let wi = if use_bsdf {
            let Some(sample) = material.sample(wo, normal, rng) else {
                continue;
            };
            if sample.event.is_delta() {
                // 混合材质中此配额抽到 Delta：局部连续 NEE 不估计它，也不重采样。
                continue;
            }
            if sample.pdf.is_none_or(|p| !p.is_finite() || p <= 0.0) {
                return Err("环境 BSDF 样本的立体角 PDF 无效".into());
            }
            sample.wi
        } else {
            environment
                .sample_direction(rng)
                .ok_or("HDRI 环境方向采样失败")?
                .direction
        };
        let weights = pdf_weights.map(|(bsdf, environment)| EnvironmentMisWeights {
            bsdf,
            environment,
            power_source: use_power.then_some(if use_bsdf {
                EnvironmentStrategy::Bsdf
            } else {
                EnvironmentStrategy::Environment
            }),
        });
        rgb += environment_direction_contribution(scene, hit, material, wo, normal, wi, weights)?
            / average_count;
    }
    if !rgb.iter().all(|value| value.is_finite()) {
        return Err("天空照明颜色非法或计算溢出".into());
    }
    Ok(rgb)
}

/// 共用环境贡献：Balance 为 f/(n_b*p_b+n_e*p_e)，Power 为 f*w_i/(n_i*p_i)。
/// N=1 使用 50/50 混合，None 保留渐变天空的纯 BSDF PDF。
fn environment_direction_contribution(
    scene: &Scene,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    normal: Vector3<f64>,
    wi: Vector3<f64>,
    pdf_weights: Option<EnvironmentMisWeights>,
) -> Result<Vector3<f64>, Box<dyn Error>> {
    let environment = scene.environment.as_ref().ok_or("环境不存在")?;
    let wi = unit_vector(wi).ok_or("环境采样方向无效")?;
    let cosine = normal.dot(&wi).clamp(0.0, 1.0);
    let pdf_bsdf = material.pdf(wi, wo, normal);
    if !pdf_bsdf.is_finite() || pdf_bsdf < 0.0 {
        return Err("环境采样的 BSDF PDF 无效".into());
    }
    let cosine_weight = if let Some(weights) = pdf_weights {
        let pdf_env = environment
            .pdf_direction(wi)
            .ok_or("HDRI 环境 PDF 查询失败")?;
        if !pdf_env.is_finite() || pdf_env < 0.0 {
            return Err("HDRI 环境 PDF 无效".into());
        }
        let q_bsdf = weights.bsdf * pdf_bsdf;
        let q_env = weights.environment * pdf_env;
        if let Some(source) = weights.power_source {
            let (sampled, other) = match source {
                EnvironmentStrategy::Bsdf => (q_bsdf, q_env),
                EnvironmentStrategy::Environment => (q_env, q_bsdf),
            };
            let (_, weight_over_pdf) =
                power_heuristic(sampled, other).ok_or("Power MIS 概率或权重无效")?;
            cosine * weight_over_pdf
        } else {
            let denominator = q_bsdf + q_env;
            if !denominator.is_finite() || denominator <= 0.0 {
                return Err("环境采样的加权分母必须有限且为正".into());
            }
            cosine / denominator
        }
    } else {
        if pdf_bsdf <= 0.0 {
            return Err("环境采样的 PDF 必须有限且为正".into());
        }
        cosine / pdf_bsdf
    };
    if !cosine_weight.is_finite() {
        return Err("环境采样的余弦权重计算溢出".into());
    }
    if cosine <= 0.0 {
        if pdf_weights.is_some() {
            return Ok(Vector3::zeros());
        }
        return Err("环境采样的入射半球无效".into());
    }
    let BsdfValue::Rgb(f_r) = material.evaluate(wi, wo, normal, EvaluateMode::Rgb) else {
        return Err("材质 RGB 求值必须返回 RGB BSDF".into());
    };
    if f_r.iter().any(|value| !value.is_finite() || *value < 0.0) {
        return Err("环境采样的 BSDF 求值无效".into());
    }
    let li = environment.radiance(wi).ok_or("环境辐亮度无效")?;
    if li.iter().any(|value| !value.is_finite()) {
        return Err("环境辐亮度必须有限".into());
    }
    let contribution = (f_r * cosine_weight).component_mul(&li);
    if contribution.iter().any(|value| !value.is_finite()) {
        return Err("环境采样贡献计算溢出".into());
    }
    if visible_to_light(scene, hit.coords, normal, wi)? {
        Ok(contribution)
    } else {
        Ok(Vector3::zeros())
    }
}

/// β=2；输入 q_i=n_i*p_i，返回 (w_i, w_i/q_i)。
/// 先按最大 q 缩放再平方，避免平方上溢/下溢；q_i=0 时两项均为 0。
pub(super) fn power_heuristic(sampled: f64, other: f64) -> Option<(f64, f64)> {
    if !sampled.is_finite() || !other.is_finite() || sampled < 0.0 || other < 0.0 {
        return None;
    }
    let scale = sampled.max(other);
    if scale == 0.0 {
        return None;
    }
    let a = sampled / scale;
    let b = other / scale;
    let sum = a * a + b * b;
    let weight = a * a / sum;
    // 等价于 weight/sampled；避免 0/0 及先平方后下溢。
    let weight_over_pdf = (a / sum) / scale;
    weight_over_pdf
        .is_finite()
        .then_some((weight, weight_over_pdf))
}

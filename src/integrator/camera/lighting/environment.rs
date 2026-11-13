//! 环境方向积分、固定配额 Balance/Power MIS 与方向贡献。

use super::super::transport::CameraTransport;
use super::transmittance_to_light;
use crate::config::RenderJob;
use crate::config::scene::EnvironmentMisHeuristic;
use crate::geometry::hit::Hit;
use crate::material::Material;
use crate::material::unit_vector;
use crate::optics::dispersion::IorMode;
use nalgebra::Vector3;
use rand::Rng;
use std::error::Error;

// HDRI 环境的方向策略概率；Environment 内部的 95%/5% 分布已经包含在 p_env 中。
pub(in crate::integrator::camera) const ENVIRONMENT_BSDF_FRACTION: f64 = 0.5;

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

/// 估计当前表面从整个环境接收到的 RGB 反射光，供相机每个连续顶点着色。
///
/// 输入：scene、Hit、材质、wo 和本顶点使用的 RNG；输出：线性 RGB L_env，配置/数值错误时 Err。
/// 目标积分：L_env=∫f_r(wi,wo)·L_i(wi)·max(n·wi,0)·V(wi)dω。
/// HDR 多样本固定分配材质/环境策略并用 Balance 或 Power MIS；单样本随机 50/50，渐变只用材质方向采样。
/// 无环境/无连续分量返回零；遮挡与零贡献样本不补采，玻璃按二值遮挡处理，不含太阳或焦散图。
pub(in crate::integrator::camera) fn environment_lighting<T: CameraTransport>(
    scene: &RenderJob,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    rng: &mut dyn rand::RngCore,
    transport: T,
    ior: IorMode,
) -> Result<T::Value, Box<dyn Error + Send + Sync>> {
    let Some(environment) = &scene.scene.environment else {
        return Ok(T::Value::default());
    };
    if !material.has_non_delta() {
        return Ok(T::Value::default());
    }
    if scene.camera_options.environment_samples == 0 {
        return Err("环境方向样本数必须大于零".into());
    }
    let normal = unit_vector(hit.normal).ok_or("环境采样的命中法线无效")?;
    let wo = unit_vector(wo).ok_or("环境采样的出射方向无效")?;
    let has_map = environment.has_map();
    let samples = scene.camera_options.environment_samples;
    let stratified = has_map && samples >= 2;
    let use_power = stratified
        && scene.camera_options.environment_mis_heuristic == EnvironmentMisHeuristic::Power;
    let bsdf_samples = samples / 2;
    let pdf_weights = has_map.then_some(if stratified {
        (bsdf_samples as f64, (samples - bsdf_samples) as f64)
    } else {
        (ENVIRONMENT_BSDF_FRACTION, 1.0 - ENVIRONMENT_BSDF_FRACTION)
    });
    // 固定配额时 PDF 权重已包含样本数，贡献求和后不能再除以 N。
    let average_count = if stratified { 1.0 } else { samples as f64 };
    let mut rgb = T::Value::default();
    for index in 0..samples {
        let use_bsdf = if stratified {
            index < bsdf_samples
        } else {
            // N=1 随机选择策略；渐变不消耗额外随机数。
            !has_map || rng.random::<f64>() < ENVIRONMENT_BSDF_FRACTION
        };
        let wi = if use_bsdf {
            let Some(sample) = transport.sample(material, wo, normal, ior, rng) else {
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
        rgb += environment_direction_contribution(
            scene, hit, material, wo, normal, wi, weights, transport,
        )? / average_count;
    }
    if !transport.finite(rgb) {
        return Err("天空照明颜色非法或计算溢出".into());
    }
    Ok(rgb)
}

/// 计算一个已抽方向的环境积分贡献，供不同采样策略共用求值和可见性。
///
/// 输入：场景、交点、材质、wo、单位法线、方向 wi 和可选策略权重；输出：RGB 贡献，错误返回 Err。
/// 令 g=f_r·L_i·cosθ·V、q_b=n_b p_b、q_e=n_e p_e；Balance 为 g/(q_b+q_e)，
/// Power 为 g·w_i/q_i，w_i=q_i²/(q_b²+q_e²)；None 时只用材质分布 g/p_b。
/// 固定配额的 q 已含样本数，贡献求和后不再除总数；单样本 n_b=n_e=0.5 表示策略选择概率。
#[allow(clippy::too_many_arguments)]
fn environment_direction_contribution<T: CameraTransport>(
    scene: &RenderJob,
    hit: &Hit,
    material: &dyn Material,
    wo: Vector3<f64>,
    normal: Vector3<f64>,
    wi: Vector3<f64>,
    pdf_weights: Option<EnvironmentMisWeights>,
    transport: T,
) -> Result<T::Value, Box<dyn Error + Send + Sync>> {
    let environment = scene.scene.environment.as_ref().ok_or("环境不存在")?;
    let wi = unit_vector(wi).ok_or("环境采样方向无效")?;
    let cosine = normal.dot(&wi).clamp(0.0, 1.0);
    let pdf_bsdf = transport.pdf(material, wi, wo, normal);
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
            return Ok(T::Value::default());
        }
        return Err("环境采样的入射半球无效".into());
    }
    let f_r = transport.evaluate(material, wi, wo, normal);
    if !transport.valid_weight(f_r) {
        return Err("invalid environment BSDF".into());
    }
    let li = transport
        .emission(environment.radiance(wi).ok_or("环境辐亮度无效")?)
        .ok_or("emission overflow")?;
    if !transport.finite(li) {
        return Err("环境辐亮度必须有限".into());
    }
    let contribution = transport.multiply(f_r, li) * cosine_weight;
    if !transport.finite(contribution) {
        return Err("环境采样贡献计算溢出".into());
    }
    Ok(contribution * transmittance_to_light(scene, hit.coords, normal, wi)?)
}

/// 计算 Power MIS 的策略权重及其除采样密度后的值，供 RGB/光谱环境共用。
///
/// 输入：sampled=q_i、other=q_j，固定配额时 q=n·p，均须非负有限。
///
/// 输出：(w_i,w_i/q_i)，w_i=q_i²/(q_i²+q_j²)；q_i=0<q_j 时两项为零，全零/非法/结果溢出时 None。
/// 先除 max(q_i,q_j) 再平方，避免上溢/下溢。
pub(in crate::integrator::camera) fn power_heuristic(
    sampled: f64,
    other: f64,
) -> Option<(f64, f64)> {
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

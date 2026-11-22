//! Ideal dielectric interface geometry and discrete event sampling.
use nalgebra::Vector3;
use rand::{Rng, RngCore};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DielectricEvent {
    Reflection,
    Transmission,
}
pub struct InterfaceSample {
    pub direction: Vector3<f64>,
    pub event: DielectricEvent,
    pub coefficient: f64,
    pub probability: f64,
    pub eta_ratio: f64,
}
impl InterfaceSample {
    /// Flux/importance coefficient divided by the chosen discrete probability.
    pub fn flux_weight(&self) -> f64 {
        self.coefficient / self.probability
    }
    /// Camera radiance adds the refractive solid-angle Jacobian on transmission only.
    pub fn radiance_weight(&self) -> f64 {
        self.flux_weight()
            * if self.event == DielectricEvent::Transmission {
                self.eta_ratio * self.eta_ratio
            } else {
                1.0
            }
    }
}
pub fn sample_dielectric(
    direction: Vector3<f64>,
    normal: Vector3<f64>,
    eta_i: f64,
    eta_t: f64,
    rng: &mut dyn RngCore,
) -> Option<InterfaceSample> {
    if !direction.iter().chain(normal.iter()).all(|v| v.is_finite()) {
        return None;
    }
    let cosine = -direction.dot(&normal);
    if !cosine.is_finite() || cosine <= 0.0 {
        return None;
    }
    let (f, cos_t) = dielectric_fresnel_and_cos_t(cosine, eta_i, eta_t)?;
    let eta_ratio = eta_i / eta_t;
    let reflection = f == 1.0 || (f > 0.0 && rng.random::<f64>() < f);
    let (direction, event, probability) = if reflection {
        let reflected = direction + 2.0 * cosine * normal;
        if !reflected.iter().all(|v| v.is_finite()) {
            return None;
        }
        (reflected, DielectricEvent::Reflection, f)
    } else {
        let cos_t = cos_t?;
        let cosi = cosine.min(1.0);
        let refracted = eta_ratio * direction + (eta_ratio * cosi - cos_t) * normal;
        if !refracted.iter().all(|v| v.is_finite()) {
            return None;
        }
        (refracted, DielectricEvent::Transmission, 1.0 - f)
    };
    Some(InterfaceSample {
        direction,
        event,
        coefficient: probability,
        probability,
        eta_ratio,
    })
}

fn dielectric_fresnel_and_cos_t(cos_i: f64, eta_i: f64, eta_t: f64) -> Option<(f64, Option<f64>)> {
    if !cos_i.is_finite()
        || !eta_i.is_finite()
        || eta_i <= 0.0
        || !eta_t.is_finite()
        || eta_t <= 0.0
    {
        return None;
    }
    let cos_i = cos_i.abs().min(1.0);
    // 相同介质没有界面；先处理它，避免掠射时出现 0/0。
    if eta_i == eta_t {
        return Some((0.0, Some(cos_i)));
    }
    if cos_i == 0.0 {
        return Some((1.0, None));
    }

    // 同比缩放不改变 Snell 或 Fresnel 方程，避免大折射率的乘积/和溢出。
    let scale = eta_i.max(eta_t);
    let eta_i = eta_i / scale;
    let eta_t = eta_t / scale;
    let sin_i = (1.0 - cos_i * cos_i).sqrt();
    let eta_sin_i = eta_i * sin_i;
    // Snell: sin(theta_t) = eta_i * sin(theta_i) / eta_t。
    // 先比较再相除，避免全反射时极大的折射率比值溢出。
    if eta_sin_i >= eta_t {
        return Some((1.0, None));
    }
    let sin_t = eta_sin_i / eta_t;
    let cos_t = (1.0 - sin_t * sin_t).max(0.0).sqrt();
    let r_s = (eta_i * cos_i - eta_t * cos_t) / (eta_i * cos_i + eta_t * cos_t);
    let r_p = (eta_t * cos_i - eta_i * cos_t) / (eta_t * cos_i + eta_i * cos_t);
    Some(((0.5 * (r_s * r_s + r_p * r_p)).clamp(0.0, 1.0), Some(cos_t)))
}

/// 计算非偏振光在理想介质界面上的能量反射比例，用于决定反射/透射。
///
/// 输入：入射角余弦 cos_i，以及正有限入射/透射折射率 η_i/η_t；cos_i 取绝对值并夹到 `[0,1]`。
///
/// 输出：F∈`[0,1]`；全反射返回 Some(1)，相同介质返回 Some(0)，非法输入返回 None。
///
/// 公式：sinθ_t=(η_i/η_t)sinθ_i，c_i=cosθ_i，c_t=cosθ_t；
/// R_s=((η_i c_i-η_t c_t)/(η_i c_i+η_t c_t))²，R_p=((η_t c_i-η_i c_t)/(η_t c_i+η_i c_t))²，F=(R_s+R_p)/2。
/// 调用方按传播方向指定两侧折射率，本函数不会根据 cos_i 的符号交换它们。
pub fn fresnel_dielectric(cos_i: f64, eta_i: f64, eta_t: f64) -> Option<f64> {
    dielectric_fresnel_and_cos_t(cos_i, eta_i, eta_t).map(|(f, _)| f)
}

pub fn refract(
    direction: Vector3<f64>,
    normal: Vector3<f64>,
    eta_i: f64,
    eta_t: f64,
) -> Option<Vector3<f64>> {
    if !direction.iter().chain(normal.iter()).all(|v| v.is_finite())
        || !eta_i.is_finite()
        || eta_i <= 0.0
        || !eta_t.is_finite()
        || eta_t <= 0.0
    {
        return None;
    }
    let cosi_raw = -normal.dot(&direction);
    if !cosi_raw.is_finite() || cosi_raw < 0.0 {
        return None;
    }
    let cosi = cosi_raw.min(1.0);
    let cos_t = if eta_i == eta_t {
        cosi
    } else {
        // 与 fresnel_dielectric 共用相同的缩放 Snell 临界角判据。
        let scale = eta_i.max(eta_t);
        let scaled_i = eta_i / scale;
        let scaled_t = eta_t / scale;
        let sin_i = (1.0 - cosi * cosi).sqrt();
        let eta_sin_i = scaled_i * sin_i;
        if eta_sin_i >= scaled_t {
            return None;
        }
        let sin_t = eta_sin_i / scaled_t;
        (1.0 - sin_t * sin_t).max(0.0).sqrt()
    };
    let eta = eta_i / eta_t;
    let refracted = eta * direction + (eta * cosi - cos_t) * normal;
    refracted.iter().all(|v| v.is_finite()).then_some(refracted)
}

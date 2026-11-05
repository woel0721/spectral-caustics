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
    let cosine = -direction.dot(&normal);
    if cosine <= 0.0 {
        return None;
    }
    let f = fresnel_dielectric(cosine, eta_i, eta_t)?;
    let reflection = f == 1.0 || (f > 0.0 && rng.random::<f64>() < f);
    let (direction, event, probability) = if reflection {
        (
            direction + 2.0 * cosine * normal,
            DielectricEvent::Reflection,
            f,
        )
    } else {
        (
            refract(direction, normal, eta_i, eta_t)?,
            DielectricEvent::Transmission,
            1.0 - f,
        )
    };
    Some(InterfaceSample {
        direction,
        event,
        coefficient: probability,
        probability,
        eta_ratio: eta_i / eta_t,
    })
}
pub fn fresnel_dielectric(cos_i: f64, eta_i: f64, eta_t: f64) -> Option<f64> {
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
        return Some(0.0);
    }
    if cos_i == 0.0 {
        return Some(1.0);
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
        return Some(1.0);
    }
    let sin_t = eta_sin_i / eta_t;
    let cos_t = (1.0 - sin_t * sin_t).max(0.0).sqrt();
    let r_s = (eta_i * cos_i - eta_t * cos_t) / (eta_i * cos_i + eta_t * cos_t);
    let r_p = (eta_t * cos_i - eta_i * cos_t) / (eta_t * cos_i + eta_i * cos_t);
    Some((0.5 * (r_s * r_s + r_p * r_p)).clamp(0.0, 1.0))
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
    let cosi = -normal.dot(&direction);
    if cosi < 0.0 {
        return None;
    }
    let eta = eta_i / eta_t;
    let k = 1.0 - eta * eta * (1.0 - cosi * cosi);
    if !k.is_finite() || k < 0.0 {
        None
    } else {
        let refracted = eta * direction + (eta * cosi - k.sqrt()) * normal;
        refracted.iter().all(|v| v.is_finite()).then_some(refracted)
    }
}

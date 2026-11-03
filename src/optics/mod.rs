//! Snell 折射、折射率模式与 CIE/D65 光谱积分。

pub mod dispersion;
pub mod iormode;
pub mod spectrum;

use nalgebra::Vector3;

/// Snell 折射：direction、normal 应为单位向量，normal 朝向入射介质。
/// eta_i 是入射介质折射率，eta_t 是出射介质折射率；全反射或非法数值返回 None。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_incidence_is_unchanged_and_total_internal_reflection_returns_none() {
        let direction = Vector3::new(0.0, 0.0, -1.0);
        let normal = Vector3::new(0.0, 0.0, 1.0);
        for (eta_i, eta_t) in [(1.0, 1.52), (1.52, 1.0)] {
            assert!((refract(direction, normal, eta_i, eta_t).unwrap() - direction).norm() < 1e-12);
        }
        let angle = 50.0_f64.to_radians();
        let direction = Vector3::new(angle.sin(), 0.0, -angle.cos());
        assert!(refract(direction, normal, 1.52, 1.0).is_none());
    }

    #[test]
    fn refraction_rejects_non_finite_values_and_nonpositive_indices() {
        let direction = Vector3::new(0.0, 0.0, -1.0);
        let normal = Vector3::new(0.0, 0.0, 1.0);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
            assert!(refract(direction, normal, value, 1.0).is_none());
            assert!(refract(direction, normal, 1.0, value).is_none());
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for axis in 0..3 {
                let mut invalid = direction;
                invalid[axis] = value;
                assert!(refract(invalid, normal, 1.0, 1.52).is_none());
                let mut invalid = normal;
                invalid[axis] = value;
                assert!(refract(direction, invalid, 1.0, 1.52).is_none());
            }
        }
    }

    #[test]
    fn refraction_handles_near_normal_near_critical_and_reversed_normals() {
        let normal = Vector3::new(0.0, 0.0, 1.0);
        let angle = 1e-9_f64;
        let direction = Vector3::new(angle.sin(), 0.0, -angle.cos());
        let refracted = refract(direction, normal, 1.0, 1.52).unwrap();
        assert!((refracted.x - direction.x / 1.52).abs() < 1e-15);
        assert!((refracted.norm() - 1.0).abs() < 1e-12);
        assert!(refract(direction, -normal, 1.0, 1.52).is_none());

        let critical = (1.0_f64 / 1.52).asin();
        for (offset, transmitted) in [(-1e-8, true), (1e-8, false)] {
            let angle = critical + offset;
            let direction = Vector3::new(angle.sin(), 0.0, -angle.cos());
            let result = refract(direction, normal, 1.52, 1.0);
            assert_eq!(result.is_some(), transmitted);
            if let Some(direction) = result {
                assert!(direction.iter().all(|value| value.is_finite()));
                assert!((direction.norm() - 1.0).abs() < 1e-12);
            }
        }
    }
}

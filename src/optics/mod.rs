//! Snell 折射、折射率模式与 CIE/D65 光谱积分。

pub mod dispersion;
pub mod interface;

#[cfg(test)]
mod tests {
    use super::interface::refract;

    use nalgebra::Vector3;

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

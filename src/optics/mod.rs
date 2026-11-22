//! 介质界面光学（Snell 折射、Fresnel 反射）与色散折射率模型。

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

    #[test]
    fn critical_angle_fresnel_refract_and_sample_dielectric_agree() {
        use super::interface::{DielectricEvent, fresnel_dielectric, sample_dielectric};
        use rand::{SeedableRng, rngs::StdRng};

        let eta_i = 1.0;
        let eta_t = 0.4372308018907408_f64;
        let cos_i = 0.8993493347292696_f64;
        let sin_i = (1.0 - cos_i * cos_i).sqrt();
        let direction = Vector3::new(sin_i, 0.0, -cos_i);
        let normal = Vector3::z();
        let f = fresnel_dielectric(cos_i, eta_i, eta_t).unwrap();
        assert!(f < 1.0);
        let refracted = refract(direction, normal, eta_i, eta_t).unwrap();
        assert!(refracted.iter().all(|v| v.is_finite()));

        struct MaxU64Rng;
        impl rand::RngCore for MaxU64Rng {
            fn next_u32(&mut self) -> u32 {
                u32::MAX
            }
            fn next_u64(&mut self) -> u64 {
                u64::MAX
            }
            fn fill_bytes(&mut self, dest: &mut [u8]) {
                dest.fill(255);
            }
        }
        let sample = sample_dielectric(direction, normal, eta_i, eta_t, &mut MaxU64Rng).unwrap();
        assert_eq!(sample.event, DielectricEvent::Transmission);
        assert!(sample.direction.iter().all(|v| v.is_finite()));

        let mut rng = StdRng::seed_from_u64(1);
        assert!(sample_dielectric(Vector3::repeat(f64::NAN), normal, 1.0, 1.5, &mut rng).is_none());
        assert!(
            sample_dielectric(
                Vector3::new(0.0, 0.0, -1.0),
                Vector3::new(0.0, 0.0, f64::INFINITY),
                1.0,
                1.5,
                &mut rng,
            )
            .is_none()
        );
    }
}

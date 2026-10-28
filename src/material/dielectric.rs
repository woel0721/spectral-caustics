//! 理想光滑介质：普通方向的 BSDF 求值为零，delta 贡献留给后续散射采样。

use super::Material;
use nalgebra::Vector3;

#[derive(Debug, Clone, PartialEq)]
pub struct Dielectric {
    ior: f64,
}

impl Dielectric {
    /// ior 为介质内部的无量纲折射率，必须有限且大于零。
    pub fn new(ior: f64) -> Result<Self, String> {
        if !ior.is_finite() || ior <= 0.0 {
            return Err("material.ior 必须有限且大于零".into());
        }
        Ok(Self { ior })
    }
}

impl Material for Dielectric {
    fn evaluate_rgb(
        &self,
        _wi: Vector3<f64>,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
    ) -> Vector3<f64> {
        // Dirac delta 不能在镜面方向返回一个人为的有限值。
        // 与 PBRT 的理想 SpecularReflection/SpecularTransmission::f 一致。
        Vector3::zeros()
    }

    fn evaluate_spectral(
        &self,
        _wi: Vector3<f64>,
        _wo: Vector3<f64>,
        _normal: Vector3<f64>,
        _wavelength_nm: f64,
    ) -> f64 {
        0.0
    }

    fn is_delta(&self) -> bool {
        true
    }

    fn ior(&self) -> Option<f64> {
        Some(self.ior)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng, rngs::StdRng};

    #[test]
    fn ideal_dielectric_preserves_ior_and_has_no_regular_bsdf_component() {
        let glass: Box<dyn Material> = Box::new(Dielectric::new(1.33).unwrap());
        assert_eq!(glass.ior(), Some(1.33));
        assert!(glass.is_delta());
        let mut rng = StdRng::seed_from_u64(42);
        let mut untouched = rng.clone();
        for wi in [Vector3::z(), -Vector3::z(), Vector3::repeat(f64::NAN)] {
            assert_eq!(
                glass.evaluate_rgb(wi, Vector3::z(), Vector3::z()),
                Vector3::zeros()
            );
            assert_eq!(glass.sample(wi, Vector3::z(), &mut rng), None);
            assert_eq!(glass.pdf(wi, Vector3::z(), Vector3::z()), 0.0);
            for nm in [450.0, 550.0, 650.0, f64::NAN] {
                assert_eq!(
                    glass.evaluate_spectral(wi, Vector3::z(), Vector3::z(), nm),
                    0.0
                );
            }
        }
        assert_eq!(rng.random::<u64>(), untouched.random::<u64>());
    }

    #[test]
    fn dielectric_constructor_rejects_invalid_indices() {
        for ior in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Dielectric::new(ior).is_err());
        }
        assert!(Dielectric::new(1.0).is_ok());
    }
}

//! 单面 Lambertian 漫反射，正半球内 f_r = albedo / π。

use super::{Material, unit_vector};
use nalgebra::Vector3;
use std::f64::consts::PI;

#[derive(Debug, Clone, PartialEq)]
pub struct Lambertian {
    albedo: Vector3<f64>,
}

impl Lambertian {
    /// albedo 为线性 RGB 反射率，各分量必须有限且位于 [0, 1]。
    pub fn new(albedo: Vector3<f64>) -> Result<Self, String> {
        if albedo
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err("material.albedo 的三个线性 RGB 分量必须有限且位于 [0, 1]".into());
        }
        Ok(Self { albedo })
    }

    pub fn albedo(&self) -> Vector3<f64> {
        self.albedo
    }
}

impl Material for Lambertian {
    fn evaluate(&self, wi: Vector3<f64>, wo: Vector3<f64>, normal: Vector3<f64>) -> Vector3<f64> {
        let (Some(wi), Some(wo), Some(normal)) =
            (unit_vector(wi), unit_vector(wo), unit_vector(normal))
        else {
            return Vector3::zeros();
        };
        if normal.dot(&wi) <= 0.0 || normal.dot(&wo) <= 0.0 {
            return Vector3::zeros();
        }
        self.albedo / PI
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffuse_bsdf_is_albedo_over_pi_without_a_cosine_factor() {
        let albedo = Vector3::new(0.2, 0.5, 0.8);
        let material = Lambertian::new(albedo).unwrap();
        assert_eq!(material.albedo(), albedo);
        for wi in [Vector3::z(), Vector3::new(3.0, 0.0, 4.0)] {
            let value = material.evaluate(wi, Vector3::z() * 2.0, Vector3::z() * 3.0);
            assert!((value - albedo / PI).norm() < 1e-14);
        }
        // 求值跟随传入的世界空间法线，不能硬编码为 +z 半球。
        let normal = Vector3::new(1.0, 2.0, 3.0);
        assert_eq!(material.evaluate(normal, normal * 2.0, normal), albedo / PI);
        assert_eq!(material.evaluate(-normal, normal, normal), Vector3::zeros());

        // 积分 f_r cosθ dω 应恢复 albedo，而非 π × albedo 或更暗的值。
        let steps = 64;
        let mut reflected = Vector3::zeros();
        for index in 0..steps {
            let cos_theta = (f64::from(index) + 0.5) / f64::from(steps);
            let wi = Vector3::new((1.0 - cos_theta * cos_theta).sqrt(), 0.0, cos_theta);
            reflected += material.evaluate(wi, Vector3::z(), Vector3::z())
                * cos_theta
                * (2.0 * PI / f64::from(steps));
        }
        assert!((reflected - albedo).norm() < 1e-12);
    }

    #[test]
    fn diffuse_bsdf_rejects_backfaces_tangents_and_invalid_vectors() {
        let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
        for invalid in [
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
            Vector3::repeat(f64::INFINITY),
            Vector3::repeat(f64::MAX),
            Vector3::repeat(f64::from_bits(1)),
        ] {
            for (wi, wo, normal) in [
                (invalid, Vector3::z(), Vector3::z()),
                (Vector3::z(), invalid, Vector3::z()),
                (Vector3::z(), Vector3::z(), invalid),
            ] {
                assert_eq!(material.evaluate(wi, wo, normal), Vector3::zeros());
            }
        }
        for (wi, wo) in [
            (-Vector3::z(), Vector3::z()),
            (Vector3::z(), -Vector3::z()),
            (-Vector3::z(), -Vector3::z()),
            (Vector3::x(), Vector3::z()),
            (Vector3::z(), Vector3::x()),
        ] {
            assert_eq!(material.evaluate(wi, wo, Vector3::z()), Vector3::zeros());
        }
        // 从背面观察时，调用方可显式翻转着色法线。
        assert_eq!(
            material.evaluate(-Vector3::z(), -Vector3::z(), -Vector3::z()),
            Vector3::repeat(0.8 / PI)
        );
    }

    #[test]
    fn diffuse_constructor_rejects_invalid_reflectance() {
        for value in [-0.1, 1.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for channel in 0..3 {
                let mut albedo = Vector3::repeat(0.5);
                albedo[channel] = value;
                assert!(Lambertian::new(albedo).is_err());
            }
        }
        for value in [0.0, 1.0] {
            assert!(Lambertian::new(Vector3::repeat(value)).is_ok());
        }
    }
}

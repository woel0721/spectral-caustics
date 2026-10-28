//! 单面 Lambertian 漫反射，正半球内 f_r = albedo / π。

use super::{Material, unit_vector};
use crate::optics::spectrum::{cie_xyz, rgb_reflectance};
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
    fn evaluate_rgb(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
    ) -> Vector3<f64> {
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

    fn evaluate_xyz(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        wavelength_nm: f64,
    ) -> Vector3<f64> {
        // 复用 RGB 接口的方向校验；黑色和无效反射方向都没有贡献。
        if self.evaluate_rgb(wi, wo, normal) == Vector3::zeros() {
            return Vector3::zeros();
        }
        let (Some(reflectance), Some(cmf)) = (
            rgb_reflectance(self.albedo, wavelength_nm),
            cie_xyz(wavelength_nm),
        ) else {
            return Vector3::zeros();
        };
        cmf * (reflectance / PI)
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
            let value = material.evaluate_rgb(wi, Vector3::z() * 2.0, Vector3::z() * 3.0);
            assert!((value - albedo / PI).norm() < 1e-14);
        }
        // 求值跟随传入的世界空间法线，不能硬编码为 +z 半球。
        let normal = Vector3::new(1.0, 2.0, 3.0);
        assert_eq!(
            material.evaluate_rgb(normal, normal * 2.0, normal),
            albedo / PI
        );
        assert_eq!(
            material.evaluate_rgb(-normal, normal, normal),
            Vector3::zeros()
        );

        // 积分 f_r cosθ dω 应恢复 albedo，而非 π × albedo 或更暗的值。
        let steps = 64;
        let mut reflected = Vector3::zeros();
        for index in 0..steps {
            let cos_theta = (f64::from(index) + 0.5) / f64::from(steps);
            let wi = Vector3::new((1.0 - cos_theta * cos_theta).sqrt(), 0.0, cos_theta);
            reflected += material.evaluate_rgb(wi, Vector3::z(), Vector3::z())
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
                assert_eq!(material.evaluate_rgb(wi, wo, normal), Vector3::zeros());
            }
        }
        for (wi, wo) in [
            (-Vector3::z(), Vector3::z()),
            (Vector3::z(), -Vector3::z()),
            (-Vector3::z(), -Vector3::z()),
            (Vector3::x(), Vector3::z()),
            (Vector3::z(), Vector3::x()),
        ] {
            assert_eq!(
                material.evaluate_rgb(wi, wo, Vector3::z()),
                Vector3::zeros()
            );
        }
        // 从背面观察时，调用方可显式翻转着色法线。
        assert_eq!(
            material.evaluate_rgb(-Vector3::z(), -Vector3::z(), -Vector3::z()),
            Vector3::repeat(0.8 / PI)
        );
    }

    #[test]
    fn spectral_diffuse_response_uses_wavelength_dependent_reflectance_and_cie_matching() {
        let red = Lambertian::new(Vector3::x()).unwrap();
        let blue = Lambertian::new(Vector3::z()).unwrap();
        let n = Vector3::z();
        let response =
            |material: &dyn Material, nm| material.evaluate_xyz(n * 2.0, n * 3.0, n * 4.0, nm);
        // 同一波长的 CIE 匹配函数相同，比较响应可验证反射率本身随波长变化。
        assert!(response(&red, 650.0).y > 8.0 * response(&blue, 650.0).y);
        assert!(response(&blue, 450.0).z > 16.0 * response(&red, 450.0).z);
        let red_at_380 = 0.94 * 1.657_560_486_708_618e-1 / PI;
        let cmf_380 = Vector3::new(0.001368, 0.000039, 0.006450001);
        assert!((response(&red, 380.0) - cmf_380 * red_at_380).norm() < 1e-14);

        // 正入射与斜入射 BSDF 一样；余弦项不能在材质接口中重复计算。
        assert_eq!(
            red.evaluate_xyz(Vector3::new(3.0, 0.0, 4.0), n, n, 650.0),
            response(&red, 650.0)
        );
        let normal = Vector3::x();
        assert_eq!(
            red.evaluate_xyz(normal, normal, normal, 650.0),
            response(&red, 650.0)
        );
    }

    #[test]
    fn spectral_diffuse_response_rejects_invalid_directions_and_wavelengths() {
        let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
        let n = Vector3::z();
        for invalid in [
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
            Vector3::repeat(f64::INFINITY),
            Vector3::repeat(f64::MAX),
        ] {
            for (wi, wo, normal) in [(invalid, n, n), (n, invalid, n), (n, n, invalid)] {
                assert_eq!(
                    material.evaluate_xyz(wi, wo, normal, 550.0),
                    Vector3::zeros()
                );
            }
        }
        for (wi, wo) in [(-n, n), (n, -n), (Vector3::x(), n), (n, Vector3::x())] {
            assert_eq!(material.evaluate_xyz(wi, wo, n, 550.0), Vector3::zeros());
        }
        for nm in [
            0.0,
            -550.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            359.0,
            831.0,
        ] {
            assert_eq!(material.evaluate_xyz(n, n, n, nm), Vector3::zeros());
        }
        let black = Lambertian::new(Vector3::zeros()).unwrap();
        assert_eq!(black.evaluate_xyz(n, n, n, 550.0), Vector3::zeros());
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

//! 单面 Lambertian 漫反射，正半球内 f_r = albedo / π。

use super::{BsdfSample, BsdfValue, EvaluateMode, Material, ScatterType, to_world, unit_vector};
use crate::optics::spectrum::rgb_reflectance;
use nalgebra::Vector3;
use rand::{Rng, RngCore};
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
    fn supports_spectral(&self) -> bool {
        true
    }

    fn sample_spectral(
        &self,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        wavelength_nm: f64,
        _: crate::optics::iormode::IorMode,
        rng: &mut dyn RngCore,
    ) -> Option<super::SpectralBsdfSample> {
        let weight = rgb_reflectance(self.albedo, wavelength_nm)?;
        // 只复用余弦方向和 PDF，光谱权重独立由 rho(λ) 计算。
        let sample = self.sample(wo, normal, rng)?;
        Some(super::SpectralBsdfSample {
            wi: sample.wi,
            weight,
            pdf: sample.pdf,
            event: sample.event,
        })
    }

    fn irradiance_reflectance(&self) -> Option<Vector3<f64>> {
        Some(self.albedo)
    }

    fn evaluate(
        &self,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        mode: EvaluateMode,
    ) -> BsdfValue {
        let (Some(wi), Some(wo), Some(normal)) =
            (unit_vector(wi), unit_vector(wo), unit_vector(normal))
        else {
            return BsdfValue::zero(mode);
        };
        if normal.dot(&wi) <= 0.0 || normal.dot(&wo) <= 0.0 {
            return BsdfValue::zero(mode);
        }
        match mode {
            EvaluateMode::Rgb => BsdfValue::Rgb(self.albedo / PI),
            EvaluateMode::Spectral { wavelength_nm } => {
                BsdfValue::Spectral(rgb_reflectance(self.albedo, wavelength_nm).unwrap_or(0.0) / PI)
            }
        }
    }

    fn sample(
        &self,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        rng: &mut dyn RngCore,
    ) -> Option<BsdfSample> {
        let (wo, normal) = (unit_vector(wo)?, unit_vector(normal)?);
        if normal.dot(&wo) <= 0.0 {
            return None;
        }

        // 均匀圆盘采样再投影到半球：p(wi) = cosθ / π（相对于立体角）。
        // https://www.pbr-book.org/4ed/Sampling_Algorithms/Sampling_Multidimensional_Functions#Cosine-WeightedHemisphereSampling
        let u = rng.random::<f64>();
        let phi = 2.0 * PI * rng.random::<f64>();
        let radius = u.sqrt();
        let local = Vector3::new(radius * phi.cos(), radius * phi.sin(), (1.0 - u).sqrt());
        let wi = unit_vector(to_world(local, normal)?)?;
        Some(BsdfSample {
            wi,
            weight: self.albedo,
            pdf: Some(self.pdf(wi, wo, normal)),
            event: ScatterType::DiffuseReflection,
        })
    }

    fn pdf(&self, wi: Vector3<f64>, wo: Vector3<f64>, normal: Vector3<f64>) -> f64 {
        let (Some(wi), Some(wo), Some(normal)) =
            (unit_vector(wi), unit_vector(wo), unit_vector(normal))
        else {
            return 0.0;
        };
        if normal.dot(&wo) <= 0.0 {
            return 0.0;
        }
        normal.dot(&wi).clamp(0.0, 1.0) / PI
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{SeedableRng, rngs::StdRng};

    fn rgb_bsdf(
        material: &dyn Material,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
    ) -> Vector3<f64> {
        let BsdfValue::Rgb(value) = material.evaluate(wi, wo, normal, EvaluateMode::Rgb) else {
            panic!("RGB 模式必须返回 RGB BSDF");
        };
        value
    }

    fn spectral_bsdf(
        material: &dyn Material,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        normal: Vector3<f64>,
        wavelength_nm: f64,
    ) -> f64 {
        let BsdfValue::Spectral(value) =
            material.evaluate(wi, wo, normal, EvaluateMode::Spectral { wavelength_nm })
        else {
            panic!("光谱模式必须返回标量 BSDF");
        };
        value
    }

    #[test]
    fn cosine_pdf_matches_known_directions_and_integrates_to_one() {
        let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
        let n = Vector3::z();
        assert!((material.pdf(n, n, n) - 1.0 / PI).abs() < 1e-14);
        assert!(
            (material.pdf(Vector3::new(3.0, 0.0, 4.0), n * 2.0, n * 3.0) - 0.8 / PI).abs() < 1e-14
        );

        // 用 cosθ ∈ [-1, 1] 的均匀网格对整个球面积分，dω = dφ d(cosθ)。
        let mut integral = 0.0;
        for index in 0..128 {
            let cos_theta = -1.0 + (f64::from(index) + 0.5) * 2.0 / 128.0;
            let wi = Vector3::new((1.0 - cos_theta * cos_theta).sqrt(), 0.0, cos_theta);
            integral += material.pdf(wi, n, n) * (4.0 * PI / 128.0);
        }
        assert!((integral - 1.0).abs() < 1e-12);
    }

    #[test]
    fn cosine_samples_repeat_follow_the_pdf_and_recover_reflectance() {
        let albedo = Vector3::new(0.2, 0.5, 0.8);
        let material = Lambertian::new(albedo).unwrap();
        let n = Vector3::new(1.0, 2.0, 3.0).normalize();
        let wo = n * 3.0;
        let mut rng = StdRng::seed_from_u64(42);
        let mut replay = StdRng::seed_from_u64(42);
        let mut mean = Vector3::zeros();
        let mut mean_cos_squared = 0.0;
        for _ in 0..16384 {
            let sample = material.sample(wo, n, &mut rng).unwrap();
            let wi = sample.wi;
            assert_eq!(material.sample(wo, n, &mut replay).unwrap().wi, wi);
            assert!((wi.norm() - 1.0).abs() < 1e-12);
            let cos_theta = n.dot(&wi);
            let pdf = material.pdf(wi, wo, n);
            assert!(cos_theta > 0.0 && pdf > 0.0);
            assert!((pdf - cos_theta / PI).abs() < 1e-14);
            // 正确的 Lambertian 重要性采样权重 f_r cosθ / p 应恰好恢复 albedo。
            assert_eq!(sample.pdf, Some(pdf));
            assert_eq!(sample.event, ScatterType::DiffuseReflection);
            assert_eq!(sample.weight, albedo);
            let weight = rgb_bsdf(&material, wi, wo, n) * (cos_theta / pdf);
            assert!((weight - albedo).norm() < 1e-12);
            let spectral_weight = spectral_bsdf(&material, wi, wo, n, 550.0) * (cos_theta / pdf);
            assert!((spectral_weight - rgb_reflectance(albedo, 550.0).unwrap()).abs() < 1e-12);
            mean += wi;
            mean_cos_squared += cos_theta * cos_theta;
        }
        // 余弦分布的 E[wi] = (2/3)n、E[cos²θ] = 1/2，可区别于均匀半球采样。
        assert!((mean / 16384.0 - n * (2.0 / 3.0)).norm() < 0.015);
        assert!((mean_cos_squared / 16384.0 - 0.5).abs() < 0.01);
    }

    #[test]
    fn sampling_is_independent_of_albedo_and_respects_flipped_normals() {
        let white = Lambertian::new(Vector3::repeat(1.0)).unwrap();
        let black = Lambertian::new(Vector3::zeros()).unwrap();
        for normal in [Vector3::x(), Vector3::y(), -Vector3::z()] {
            let mut white_rng = StdRng::seed_from_u64(7);
            let mut black_rng = StdRng::seed_from_u64(7);
            for _ in 0..64 {
                let wi = white
                    .sample(normal * 2.0, normal * 3.0, &mut white_rng)
                    .unwrap()
                    .wi;
                assert_eq!(black.sample(normal, normal, &mut black_rng).unwrap().wi, wi);
                assert!(wi.dot(&normal) > 0.0);
                assert_eq!(white.pdf(wi, normal, normal), black.pdf(wi, normal, normal));
            }
        }
    }

    #[test]
    fn sampling_and_pdf_reject_invalid_geometry_without_consuming_randomness() {
        let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
        let n = Vector3::z();
        let mut rng = StdRng::seed_from_u64(42);
        let mut untouched = rng.clone();
        for invalid in [
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
            Vector3::repeat(f64::INFINITY),
            Vector3::repeat(f64::MAX),
        ] {
            assert!(material.sample(invalid, n, &mut rng).is_none());
            assert!(material.sample(n, invalid, &mut rng).is_none());
            for (wi, wo, normal) in [(invalid, n, n), (n, invalid, n), (n, n, invalid)] {
                assert_eq!(material.pdf(wi, wo, normal), 0.0);
            }
        }
        for wo in [-n, Vector3::x()] {
            assert!(material.sample(wo, n, &mut rng).is_none());
            assert_eq!(material.pdf(n, wo, n), 0.0);
        }
        for wi in [-n, Vector3::x()] {
            assert_eq!(material.pdf(wi, n, n), 0.0);
        }
        assert_eq!(rng.random::<u64>(), untouched.random::<u64>());
    }

    #[test]
    fn diffuse_bsdf_is_albedo_over_pi_without_a_cosine_factor() {
        let albedo = Vector3::new(0.2, 0.5, 0.8);
        let material = Lambertian::new(albedo).unwrap();
        assert_eq!(material.albedo(), albedo);
        for wi in [Vector3::z(), Vector3::new(3.0, 0.0, 4.0)] {
            let value = rgb_bsdf(&material, wi, Vector3::z() * 2.0, Vector3::z() * 3.0);
            assert!((value - albedo / PI).norm() < 1e-14);
        }
        // 求值跟随传入的世界空间法线，不能硬编码为 +z 半球。
        let normal = Vector3::new(1.0, 2.0, 3.0);
        assert_eq!(
            rgb_bsdf(&material, normal, normal * 2.0, normal),
            albedo / PI
        );
        assert_eq!(
            rgb_bsdf(&material, -normal, normal, normal),
            Vector3::zeros()
        );

        // 积分 f_r cosθ dω 应恢复 albedo，而非 π × albedo 或更暗的值。
        let steps = 64;
        let mut reflected = Vector3::zeros();
        for index in 0..steps {
            let cos_theta = (f64::from(index) + 0.5) / f64::from(steps);
            let wi = Vector3::new((1.0 - cos_theta * cos_theta).sqrt(), 0.0, cos_theta);
            reflected += rgb_bsdf(&material, wi, Vector3::z(), Vector3::z())
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
                assert_eq!(rgb_bsdf(&material, wi, wo, normal), Vector3::zeros());
            }
        }
        for (wi, wo) in [
            (-Vector3::z(), Vector3::z()),
            (Vector3::z(), -Vector3::z()),
            (-Vector3::z(), -Vector3::z()),
            (Vector3::x(), Vector3::z()),
            (Vector3::z(), Vector3::x()),
        ] {
            assert_eq!(rgb_bsdf(&material, wi, wo, Vector3::z()), Vector3::zeros());
        }
        // 从背面观察时，调用方可显式翻转着色法线。
        assert_eq!(
            rgb_bsdf(&material, -Vector3::z(), -Vector3::z(), -Vector3::z()),
            Vector3::repeat(0.8 / PI)
        );
    }

    #[test]
    fn spectral_diffuse_bsdf_is_wavelength_dependent_reflectance_over_pi() {
        let red = Lambertian::new(Vector3::x()).unwrap();
        let blue = Lambertian::new(Vector3::z()).unwrap();
        let n = Vector3::z();
        let bsdf =
            |material: &dyn Material, nm| spectral_bsdf(material, n * 2.0, n * 3.0, n * 4.0, nm);
        assert!(bsdf(&red, 650.0) > 8.0 * bsdf(&blue, 650.0));
        assert!(bsdf(&blue, 450.0) > 16.0 * bsdf(&red, 450.0));
        let red_at_380 = 0.94 * 1.657_560_486_708_618e-1 / PI;
        assert!((bsdf(&red, 380.0) - red_at_380).abs() < 1e-14);

        // 正入射与斜入射 BSDF 一样；余弦项不能在材质接口中重复计算。
        assert_eq!(
            spectral_bsdf(&red, Vector3::new(3.0, 0.0, 4.0), n, n, 650.0),
            bsdf(&red, 650.0)
        );
        let normal = Vector3::x();
        assert_eq!(
            spectral_bsdf(&red, normal, normal, normal, 650.0),
            bsdf(&red, 650.0)
        );

        // 光谱 BSDF 不受 CIE 表范围限制，沿用反射基谱的端点延拓。
        assert_eq!(bsdf(&red, 359.0), bsdf(&red, 380.0));
        assert_eq!(bsdf(&red, 831.0), bsdf(&red, 720.0));

        // 半球积分 f_r cosθ dω 恢复 rho(λ)，且不超过入射能量。
        let mut reflected = 0.0;
        for index in 0..64 {
            let cos_theta = (f64::from(index) + 0.5) / 64.0;
            let wi = Vector3::new((1.0 - cos_theta * cos_theta).sqrt(), 0.0, cos_theta);
            reflected += spectral_bsdf(&red, wi, n, n, 380.0) * cos_theta * (2.0 * PI / 64.0);
        }
        assert!((reflected - red_at_380 * PI).abs() < 1e-14);
        assert!((0.0..=1.0).contains(&reflected));
    }

    #[test]
    fn spectral_diffuse_bsdf_rejects_invalid_directions_and_wavelengths() {
        let material = Lambertian::new(Vector3::repeat(0.8)).unwrap();
        let n = Vector3::z();
        for invalid in [
            Vector3::zeros(),
            Vector3::repeat(f64::NAN),
            Vector3::repeat(f64::INFINITY),
            Vector3::repeat(f64::MAX),
            Vector3::repeat(f64::from_bits(1)),
        ] {
            for (wi, wo, normal) in [(invalid, n, n), (n, invalid, n), (n, n, invalid)] {
                assert_eq!(spectral_bsdf(&material, wi, wo, normal, 550.0), 0.0);
            }
        }
        for (wi, wo) in [(-n, n), (n, -n), (Vector3::x(), n), (n, Vector3::x())] {
            assert_eq!(spectral_bsdf(&material, wi, wo, n, 550.0), 0.0);
        }
        for nm in [0.0, -550.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(spectral_bsdf(&material, n, n, n, nm), 0.0);
        }
        let black = Lambertian::new(Vector3::zeros()).unwrap();
        assert_eq!(spectral_bsdf(&black, n, n, n, 550.0), 0.0);
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

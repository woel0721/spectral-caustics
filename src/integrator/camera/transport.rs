//! The two camera number representations. Geometry, event order and estimators are shared.
use super::ground::{GroundLighting, sample_ground_irradiance};
use crate::caustics::SpectralCausticsMap;
use crate::{
    color::xyz_to_linear_srgb,
    geometry::hit::Hit,
    material::{BsdfSample, Material},
    optics::dispersion::IorMode,
    spectrum::{
        cie_xyz,
        emission::{d65_spectrum, rgb_emission},
    },
};
use nalgebra::Vector3;
use rand::RngCore;
use std::ops::{Add, AddAssign, Div, Mul};

#[derive(Clone, Copy, Debug)]
pub struct Rgb {
    pub sun_color: Vector3<f64>,
}
impl Default for Rgb {
    fn default() -> Self {
        Self {
            sun_color: Vector3::repeat(1.0),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Spectral {
    pub wavelength_nm: f64,
}
#[derive(Clone, Copy)]
pub enum Caustics<'a> {
    None,
    Rgb(&'a GroundLighting<'a>),
    Spectral(&'a SpectralCausticsMap),
}
pub trait CameraTransport: Copy {
    type Value: Copy
        + Default
        + PartialEq
        + Add<Output = Self::Value>
        + AddAssign
        + Mul<f64, Output = Self::Value>
        + Div<f64, Output = Self::Value>;
    fn one(self) -> Self::Value;
    fn multiply(self, a: Self::Value, b: Self::Value) -> Self::Value;
    fn finite(self, v: Self::Value) -> bool;
    fn valid_weight(self, v: Self::Value) -> bool;
    fn importance(self, v: Self::Value) -> f64;
    fn emission(self, rgb: Vector3<f64>) -> Option<Self::Value>;
    fn sunlight(
        self,
        wi: Vector3<f64>,
        strength: f64,
        map: Caustics<'_>,
    ) -> Result<(Vector3<f64>, Self::Value), &'static str>;
    fn disk_emission(self) -> Self::Value;
    fn evaluate(
        self,
        m: &dyn Material,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        n: Vector3<f64>,
    ) -> Self::Value;
    fn sample(
        self,
        m: &dyn Material,
        wo: Vector3<f64>,
        n: Vector3<f64>,
        ior: IorMode,
        rng: &mut dyn RngCore,
    ) -> Option<BsdfSample<Self::Value>>;
    fn pdf(self, m: &dyn Material, wi: Vector3<f64>, wo: Vector3<f64>, n: Vector3<f64>) -> f64;
    fn caustics(
        self,
        hit: &Hit,
        m: &dyn Material,
        wo: Vector3<f64>,
        map: Caustics<'_>,
    ) -> Result<Self::Value, &'static str>;
    fn xyz(self, v: Self::Value) -> Option<Vector3<f64>>;
    fn valid(self) -> bool {
        true
    }
}
impl CameraTransport for Rgb {
    type Value = Vector3<f64>;
    fn one(self) -> Self::Value {
        Vector3::repeat(1.0)
    }
    fn multiply(self, a: Self::Value, b: Self::Value) -> Self::Value {
        a.component_mul(&b)
    }
    fn finite(self, v: Self::Value) -> bool {
        v.iter().all(|v| v.is_finite())
    }
    fn valid_weight(self, v: Self::Value) -> bool {
        v.iter().all(|v| v.is_finite() && *v >= 0.0)
    }
    fn importance(self, v: Self::Value) -> f64 {
        v.max()
    }
    fn emission(self, rgb: Vector3<f64>) -> Option<Self::Value> {
        Some(rgb)
    }
    fn sunlight(
        self,
        wi: Vector3<f64>,
        strength: f64,
        map: Caustics<'_>,
    ) -> Result<(Vector3<f64>, Self::Value), &'static str> {
        if let Caustics::Rgb(g) = map {
            Ok((
                g.light_wi,
                xyz_to_linear_srgb(g.sun_irradiance_xyz).ok_or("sun XYZ overflow")?,
            ))
        } else {
            Ok((wi, self.sun_color * strength))
        }
    }
    fn disk_emission(self) -> Self::Value {
        xyz_to_linear_srgb(crate::spectrum::emission::d65_white_xyz()).expect("embedded D65 white")
    }
    fn evaluate(
        self,
        m: &dyn Material,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        n: Vector3<f64>,
    ) -> Self::Value {
        m.evaluate_rgb(wi, wo, n)
    }
    fn sample(
        self,
        m: &dyn Material,
        wo: Vector3<f64>,
        n: Vector3<f64>,
        _: IorMode,
        rng: &mut dyn RngCore,
    ) -> Option<BsdfSample<Self::Value>> {
        m.sample(wo, n, rng)
    }
    fn pdf(self, m: &dyn Material, wi: Vector3<f64>, wo: Vector3<f64>, n: Vector3<f64>) -> f64 {
        m.pdf(wi, wo, n)
    }
    fn caustics(
        self,
        hit: &Hit,
        _m: &dyn Material,
        _wo: Vector3<f64>,
        map: Caustics<'_>,
    ) -> Result<Self::Value, &'static str> {
        if let Caustics::Rgb(g) = map
            && hit.object_id == (g.object_id)
        {
            let xyz = sample_ground_irradiance(g.irradiance, g.receiver, hit.coords)
                .ok_or("caustic query overflow")?;
            xyz_to_linear_srgb(xyz * (g.rho / std::f64::consts::PI)).ok_or("caustic RGB overflow")
        } else {
            Ok(Vector3::zeros())
        }
    }
    fn xyz(self, v: Self::Value) -> Option<Vector3<f64>> {
        crate::color::linear_srgb_to_xyz(v)
    }
}
impl CameraTransport for Spectral {
    type Value = f64;
    fn one(self) -> f64 {
        1.0
    }
    fn multiply(self, a: f64, b: f64) -> f64 {
        a * b
    }
    fn finite(self, v: f64) -> bool {
        v.is_finite()
    }
    fn valid_weight(self, v: f64) -> bool {
        v.is_finite() && v >= 0.0
    }
    fn importance(self, v: f64) -> f64 {
        v
    }
    fn emission(self, rgb: Vector3<f64>) -> Option<f64> {
        rgb_emission(rgb, self.wavelength_nm)
    }
    fn sunlight(
        self,
        wi: Vector3<f64>,
        strength: f64,
        _map: Caustics<'_>,
    ) -> Result<(Vector3<f64>, f64), &'static str> {
        Ok((
            wi,
            strength * d65_spectrum(self.wavelength_nm).ok_or("invalid wavelength")?,
        ))
    }
    fn disk_emission(self) -> f64 {
        d65_spectrum(self.wavelength_nm).expect("validated wavelength")
    }
    fn evaluate(
        self,
        m: &dyn Material,
        wi: Vector3<f64>,
        wo: Vector3<f64>,
        n: Vector3<f64>,
    ) -> f64 {
        m.evaluate_spectral(wi, wo, n, self.wavelength_nm)
    }
    fn sample(
        self,
        m: &dyn Material,
        wo: Vector3<f64>,
        n: Vector3<f64>,
        ior: IorMode,
        rng: &mut dyn RngCore,
    ) -> Option<BsdfSample<f64>> {
        m.sample_spectral(wo, n, self.wavelength_nm, ior, rng)
    }
    fn pdf(self, m: &dyn Material, wi: Vector3<f64>, wo: Vector3<f64>, n: Vector3<f64>) -> f64 {
        m.pdf_spectral(wi, wo, n, self.wavelength_nm)
    }
    fn caustics(
        self,
        hit: &Hit,
        m: &dyn Material,
        wo: Vector3<f64>,
        map: Caustics<'_>,
    ) -> Result<f64, &'static str> {
        if let Caustics::Spectral(g) = map
            && hit.object_id == g.ground_id()
        {
            Ok(self.evaluate(m, hit.normal, wo, hit.normal)
                * g.sample(hit.coords, self.wavelength_nm)
                    .ok_or("caustic spectral query overflow")?)
        } else {
            Ok(0.0)
        }
    }
    fn xyz(self, v: f64) -> Option<Vector3<f64>> {
        cie_xyz(self.wavelength_nm).map(|cmf| cmf * v)
    }
    fn valid(self) -> bool {
        (380.0..=780.0).contains(&self.wavelength_nm)
    }
}

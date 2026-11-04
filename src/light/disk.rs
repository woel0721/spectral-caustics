//! 单面、均匀 Lambertian 发光圆盘；通量采用项目的 D65/Y 相对标尺。

use crate::{
    geometry::ray::Ray,
    material::{to_world, unit_vector},
};
use nalgebra::Vector3;
use std::f64::consts::{PI, TAU};

#[derive(Debug, Clone, Copy)]
pub struct DiskAreaLight {
    pub center: Vector3<f64>,
    pub normal: Vector3<f64>,
    pub radius: f64,
    pub total_flux: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct DiskPositionSample {
    pub position: Vector3<f64>,
    pub normal: Vector3<f64>,
    /// 相对于面积 dA，单位 m⁻²。
    pub pdf_area: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct DiskEmissionSample {
    pub ray: Ray,
    pub normal: Vector3<f64>,
    pub pdf_area: f64,
    /// 相对于立体角 dω，单位 sr⁻¹；cos(theta)/π。
    pub pdf_direction: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct DiskDirectSample {
    pub position: Vector3<f64>,
    pub wi: Vector3<f64>,
    pub distance: f64,
    pub cosine_light: f64,
    pub pdf_area: f64,
    /// Le*cos(theta_light)/(distance²*pdf_area)，尚未乘 BSDF、接收余弦或 D65。
    pub incident_weight: f64,
}

fn valid_uv(u: [f64; 2]) -> bool {
    u.iter().all(|v| (0.0..1.0).contains(v))
}

impl DiskAreaLight {
    pub fn new(
        center: Vector3<f64>,
        normal: Vector3<f64>,
        radius: f64,
        total_flux: f64,
    ) -> Result<Self, String> {
        let light = Self {
            center,
            normal: unit_vector(normal).ok_or("圆盘光源法线须有限且非零")?,
            radius,
            total_flux,
        };
        light.validate()?;
        Ok(light)
    }

    pub fn area(&self) -> Option<f64> {
        let area = PI * self.radius * self.radius;
        (self.radius.is_finite()
            && self.radius > 0.0
            && area.is_finite()
            && area > 0.0
            && area.recip().is_finite())
        .then_some(area)
    }

    /// 单面总通量 Φ = A*π*Le，返回不含 D65 密度的辐亮度系数。
    pub fn radiance(&self) -> Option<f64> {
        let le = (self.total_flux / self.area()?) / PI;
        (self.total_flux.is_finite() && self.total_flux >= 0.0 && le.is_finite()).then_some(le)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.center.iter().any(|v| !v.is_finite())
            || unit_vector(self.normal).is_none()
            || self.radiance().is_none()
        {
            return Err(
                "disk_area 的 center/normal/radius/total_flux 无效或面积/PDF/辐亮度溢出".into(),
            );
        }
        // 确保圆盘的世界范围可表示，避免有限中心加半径后溢出。
        if self
            .center
            .iter()
            .any(|v| !(v.abs() + self.radius).is_finite())
        {
            return Err("disk_area 世界范围溢出".into());
        }
        Ok(())
    }

    pub fn sample_position(&self, u: [f64; 2]) -> Option<DiskPositionSample> {
        if !valid_uv(u) {
            return None;
        }
        let n = unit_vector(self.normal)?;
        let r = self.radius * u[0].sqrt();
        let phi = TAU * u[1];
        let position = self.center + to_world(Vector3::new(r * phi.cos(), r * phi.sin(), 0.0), n)?;
        position
            .iter()
            .all(|v| v.is_finite())
            .then_some(DiskPositionSample {
                position,
                normal: n,
                pdf_area: self.area()?.recip(),
            })
    }

    /// 全半球余弦发射，不裁剪方向到玻璃。每条匹配采样的通量为 Φ/N：
    /// Le*cos(theta)/(N*p_A*p_ω) = [Φ/(πA)]*cos(theta)/[N*(1/A)*(cos(theta)/π)]。
    pub fn sample_emission(
        &self,
        position_u: [f64; 2],
        direction_u: [f64; 2],
    ) -> Option<DiskEmissionSample> {
        if !valid_uv(direction_u) {
            return None;
        }
        let p = self.sample_position(position_u)?;
        let r = direction_u[0].sqrt();
        let phi = TAU * direction_u[1];
        let local = Vector3::new(r * phi.cos(), r * phi.sin(), (1.0 - direction_u[0]).sqrt());
        let ray = Ray::new(p.position, to_world(local, p.normal)?)?;
        let pdf_direction = p.normal.dot(&ray.direction).max(0.0) / PI;
        (pdf_direction > 0.0 && pdf_direction.is_finite()).then_some(DiskEmissionSample {
            ray,
            normal: p.normal,
            pdf_area: p.pdf_area,
            pdf_direction,
        })
    }

    /// 圆盘均匀面积 NEE；背面返回零贡献（None），不补采。非法几何/数值溢出明确报错。
    pub fn sample_direct(
        &self,
        position: Vector3<f64>,
        u: [f64; 2],
    ) -> Result<Option<DiskDirectSample>, String> {
        let p = self.sample_position(u).ok_or("圆盘 NEE 位置采样无效")?;
        let delta = p.position - position;
        let distance_squared = delta.norm_squared();
        if !distance_squared.is_finite() || distance_squared <= 0.0 {
            return Err("圆盘 NEE 距离无效、重合或平方溢出".into());
        }
        let distance = distance_squared.sqrt();
        let wi = delta / distance;
        let cosine_light = p.normal.dot(&-wi).max(0.0);
        if cosine_light == 0.0 {
            return Ok(None);
        }
        // Le/p_A = Φ/π；先消去面积可避免小圆盘的巨大 Le 与巨大 PDF 相除。
        let incident_weight = (self.total_flux / PI) * cosine_light / distance_squared;
        if !incident_weight.is_finite() || incident_weight < 0.0 {
            return Err("圆盘 NEE 几何权重溢出".into());
        }
        Ok(Some(DiskDirectSample {
            position: p.position,
            wi,
            distance,
            cosine_light,
            pdf_area: p.pdf_area,
            incident_weight,
        }))
    }
}

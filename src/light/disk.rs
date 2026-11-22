//! 单面、均匀 Lambertian 发光圆盘；通量采用项目的 D65/Y 相对标尺。

use crate::geometry::ray::Ray;
use crate::material::unit_vector;
use nalgebra::Vector3;
use std::f64::consts::PI;
use std::f64::consts::TAU;

#[derive(Debug, Clone, Copy)]
pub struct DiskAreaLight {
    /// 圆盘中心的世界坐标，单位米。
    pub(crate) center: Vector3<f64>,
    /// 指向发光半球的法线，构造时归一化。
    pub(crate) normal: Vector3<f64>,
    tangent: Vector3<f64>,
    bitangent: Vector3<f64>,
    /// 圆盘半径，单位米；面积 A=π·radius²。
    pub(crate) radius: f64,
    /// 单面发出的相对总通量 Φ，采用 D65/Y 标尺，未标定为瓦特。
    pub(crate) total_flux: f64,
    inv_area: f64,
    radiance: f64,
    flux_over_pi: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct DiskPositionSample {
    /// 抽到的发光位置，世界坐标，单位米。
    pub position: Vector3<f64>,
    /// 该位置朝向发光半球的单位法线。
    pub normal: Vector3<f64>,
    /// 相对于面积 dA，单位 m⁻²。
    pub pdf_area: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct DiskEmissionSample {
    /// 从圆盘位置向发光半球发出的射线。
    pub ray: Ray,
    /// 发射位置的单位法线，供余弦与 PDF 核对。
    pub normal: Vector3<f64>,
    /// 发射位置的面积采样密度 p_A=1/A，单位 m⁻²。
    pub pdf_area: f64,
    /// 相对于立体角 dω，单位 sr⁻¹；cos(theta)/π。
    pub pdf_direction: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct DiskDirectSample {
    /// 抽到的发光位置，世界坐标，单位米。
    pub position: Vector3<f64>,
    /// 从着色点指向该发光位置的单位方向。
    pub wi: Vector3<f64>,
    /// 着色点与发光位置之间的距离，单位米。
    pub distance: f64,
    /// 发光侧余弦 max(n_light·(-wi),0)，背面不贡献照明。
    pub cosine_light: f64,
    /// 发光位置的面积采样密度 p_A=1/A，单位 m⁻²。
    pub pdf_area: f64,
    /// Le*cos(theta_light)/(distance²*pdf_area)，尚未乘 BSDF、接收余弦或 D65。
    pub incident_weight: f64,
}

/// 检查输入的两个采样值是否都在 [0,1)；返回 bool，供圆盘位置和发射方向采样共用。
fn valid_uv(u: [f64; 2]) -> bool {
    u.iter().all(|v| (0.0..1.0).contains(v))
}

impl DiskAreaLight {
    /// 创建单面均匀发光圆盘，供面积直接光和光子发射共用。
    ///
    /// 输入：世界中心 center（米）、朝发光半球的 normal、正半径 radius（米）和非负相对总通量 Φ。
    ///
    /// 输出：法线归一化且参数有效的 DiskAreaLight；几何、面积、PDF 或辐亮度无效返回 Err。
    pub fn new(
        center: Vector3<f64>,
        normal: Vector3<f64>,
        radius: f64,
        total_flux: f64,
    ) -> Result<Self, String> {
        let normal = unit_vector(normal).ok_or("disk normal must be finite and nonzero")?;
        let area = PI * radius * radius;
        let inv_area = area.recip();
        let radiance = (total_flux / area) / PI;
        let flux_over_pi = total_flux / PI;
        if !radius.is_finite()
            || radius <= 0.0
            || !area.is_finite()
            || area <= 0.0
            || !inv_area.is_finite()
            || !total_flux.is_finite()
            || total_flux < 0.0
            || !radiance.is_finite()
            || center
                .iter()
                .any(|v| !v.is_finite() || !(v.abs() + radius).is_finite())
        {
            return Err("invalid disk geometry, area, flux or radiance".into());
        }
        let helper = if normal.x.abs() > normal.y.abs() {
            Vector3::y()
        } else {
            Vector3::x()
        };
        let bitangent = unit_vector(normal.cross(&helper))
            .ok_or("disk tangent frame must be finite and nonzero")?;
        let tangent = bitangent.cross(&normal);
        Ok(Self {
            center,
            normal,
            tangent,
            bitangent,
            radius,
            total_flux,
            inv_area,
            radiance,
            flux_over_pi,
        })
    }
    pub fn center(&self) -> Vector3<f64> {
        self.center
    }
    pub fn normal(&self) -> Vector3<f64> {
        self.normal
    }
    pub fn total_flux(&self) -> f64 {
        self.total_flux
    }
    /// Le=Φ/(πA), without spectral density. Cached at construction.
    pub fn radiance(&self) -> f64 {
        self.radiance
    }

    /// 在发光圆盘上均匀抽取一点，供直接光和光子发射使用。
    ///
    /// 输入：两个 [0,1) 样本 u；输出：位置、单位法线、面积密度 p_A=1/(πr²)，无效时 None。
    ///
    /// 公式：局部半径 s=r√u₀，方位角 φ=2πu₁，点为 center+to_world((s cosφ,s sinφ,0))。
    pub fn sample_position(&self, u: [f64; 2]) -> Option<DiskPositionSample> {
        if !valid_uv(u) {
            return None;
        }
        let r = self.radius * u[0].sqrt();
        let (sin, cos) = (TAU * u[1]).sin_cos();
        let position = self.center + (self.tangent * (r * cos) + self.bitangent * (r * sin));
        position
            .iter()
            .all(|v| v.is_finite())
            .then_some(DiskPositionSample {
                position,
                normal: self.normal,
                pdf_area: self.inv_area,
            })
    }

    /// 同时采样圆盘上的起点和发光半球方向，生成光子发射记录。
    ///
    /// 输入：各含两个 [0,1) 数的 position_u/direction_u；输出：Ray、法线、p_A 与 p_ω，无效时 None。
    ///
    /// 公式：p_A=1/A，p_ω=cosθ/π；方向覆盖整个发光半球，不限定为指向玻璃。
    /// 匹配的每条光子通量 L_e cosθ/(N p_A p_ω)=Φ/N，由调用方施加，不能再除一次 PDF。
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
        let (sin, cos) = (TAU * direction_u[1]).sin_cos();
        let direction = self.tangent * (r * cos)
            + self.bitangent * (r * sin)
            + self.normal * (1.0 - direction_u[0]).sqrt();
        let ray = Ray::new(p.position, direction)?;
        let pdf_direction = p.normal.dot(&ray.direction).max(0.0) / PI;
        (pdf_direction > 0.0 && pdf_direction.is_finite()).then_some(DiskEmissionSample {
            ray,
            normal: p.normal,
            pdf_area: p.pdf_area,
            pdf_direction,
        })
    }

    /// 从着色点抽取一个圆盘发光位置，准备面积直接光采样的几何权重。
    ///
    /// 输入：世界着色点 position（米）和两个 [0,1) 样本 u。
    ///
    /// 输出：Some(位置、指向光源的 wi、距离 r、PDF、入射权重)；光源背面为 Ok(None)，非法几何/溢出为 Err。
    ///
    /// 公式：incident_weight=L_e·cosθ_light/(r² p_A)=Φ·cosθ_light/(πr²)。
    /// 这里 r 是采样点间距离；调用方还须乘 BSDF、接收余弦、D65、可见性并按样本数平均，不补采背面样本。
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
        let incident_weight = self.flux_over_pi * cosine_light / distance_squared;
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

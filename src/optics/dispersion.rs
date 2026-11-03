//! 介质的 Cauchy 色散；波长输入 nm，B 的单位为 μm²。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cauchy {
    pub a: f64,
    pub b_um2: f64,
}

impl Default for Cauchy {
    /// 演示参数，不代表特定实测玻璃。
    fn default() -> Self {
        Self {
            a: 1.5046,
            b_um2: 0.0042,
        }
    }
}

impl Cauchy {
    pub fn validate(&self) -> Result<(), String> {
        if !self.a.is_finite() || self.a <= 0.0 || !self.b_um2.is_finite() || self.b_um2 < 0.0 {
            return Err("material.dispersion 要求有限的 a > 0、b_um2 >= 0".into());
        }
        // 配置光谱范围内的端点不能溢出；中间值在两者之间。
        if [380.0, 780.0].into_iter().any(|nm| self.ior(nm).is_none()) {
            return Err("material.dispersion 在可见波段计算溢出".into());
        }
        Ok(())
    }

    pub fn ior(&self, wavelength_nm: f64) -> Option<f64> {
        if !wavelength_nm.is_finite()
            || wavelength_nm <= 0.0
            || !self.a.is_finite()
            || self.a <= 0.0
            || !self.b_um2.is_finite()
            || self.b_um2 < 0.0
        {
            return None;
        }
        let wavelength_um = wavelength_nm / 1000.0;
        let index = self.a + self.b_um2 / wavelength_um.powi(2);
        (index.is_finite() && index > 0.0).then_some(index)
    }
}

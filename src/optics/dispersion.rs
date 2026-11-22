//! 介质的 Cauchy 色散；波长输入 nm，B 的单位为 μm²。

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cauchy {
    pub a: f64,
    pub b_um2: f64,
}

impl Default for Cauchy {
    /// 返回演示用 Cauchy 系数 A=1.5046、B=0.0042 μm²；不代表某种实测玻璃。
    fn default() -> Self {
        Self {
            a: 1.5046,
            b_um2: 0.0042,
        }
    }
}

impl Cauchy {
    /// 校验当前 Cauchy 模型能否用于可见光色散。
    ///
    /// 输入：self 中的 A/B；输出：有效时 Ok(())，A≤0、B<0、非有限值或可见光端点溢出时 Err。
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

    /// 按波长求玻璃折射率，让不同颜色的光产生不同折射方向。
    ///
    /// 输入：正有限波长 wavelength_nm（nm）及 self 中的 A/B。
    ///
    /// 输出：无量纲折射率 n；波长、系数或结果无效时返回 None。
    ///
    /// 公式：λ_μm=λ_nm/1000，n(λ)=A+B/λ_μm²；B 的单位为 μm²。
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nanometer_input_uses_micrometers_and_normal_dispersion() {
        let model = Cauchy::default();
        for nm in [380.0, 450.0, 550.0, 650.0, 780.0] {
            assert_eq!(
                model.ior(nm),
                Some(model.a + model.b_um2 / (nm / 1000.0_f64).powi(2))
            );
        }
        assert!(model.ior(450.0) > model.ior(650.0));
    }
    #[test]
    fn invalid_wavelength_and_overflow_fail_explicitly() {
        for nm in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e-200] {
            assert!(Cauchy::default().ior(nm).is_none());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IorMode {
    /// 所有波长均使用材质的常数折射率。
    Constant,
    /// 按输入波长（nm）查询材质的 Cauchy 模型；未配置色散的介质仍使用常数。
    Dispersion,
}

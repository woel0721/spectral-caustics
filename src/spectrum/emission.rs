//! 相机光谱功率密度（每 nm）；D65 白点校准的 PBRT Illuminant 重建。
//! 发光基谱独立于反射率基谱，不对 HDR 强度施加 `[0,1]` 上限。

use super::cie_xyz;
use super::d65_spd;
use nalgebra::Vector3;
use std::sync::LazyLock;

pub const WAVELENGTH_MIN_NM: f64 = 380.0;
pub const WAVELENGTH_MAX_NM: f64 = 780.0;
/// 各等宽波长层等配额混合的边缘 PDF；分层估计的每项权重为 1/(PDF*spp)。
pub const WAVELENGTH_PDF: f64 = 1.0 / (WAVELENGTH_MAX_NM - WAVELENGTH_MIN_NM);

static ILLUMINANT: LazyLock<Vec<[f64; 8]>> = LazyLock::new(|| {
    let csv = include_str!("../../assets/color/pbrt_rgb_illuminant.csv");
    super::parse_csv(csv.split_once('\n').expect("发光基谱缺少表头").1)
});

/// 原数据在每个整数 nm 间均线性插值；SPD×CMF 为二次式，用 Simpson 精确积分。
static D65_SCALE: LazyLock<f64> = LazyLock::new(|| {
    let f = |nm| d65_spd(nm).unwrap() * cie_xyz(nm).unwrap().y;
    let integral: f64 = (380..780)
        .map(|nm| {
            let nm = f64::from(nm);
            (f(nm) + 4.0 * f(nm + 0.5) + f(nm + 1.0)) / 6.0
        })
        .sum();
    1.0 / integral
});

/// 查询归一化 D65 白光的光谱密度，供太阳和圆盘照明使用。
///
/// 输入：380–780 nm 内的波长；输出：每 nm 的相对密度 S(λ)，非法或范围外返回 None。
///
/// 公式：S(λ)=k·D65(λ)，k=1/∫D65(λ)ȳ(λ)dλ，使白光积分亮度 Y=1。
/// 此返回值未包含 Δλ 或相机的 1/p(λ)，调用方在积分时只乘一次。
pub fn d65_spectrum(wavelength_nm: f64) -> Option<f64> {
    if !(WAVELENGTH_MIN_NM..=WAVELENGTH_MAX_NM).contains(&wavelength_nm) {
        return None;
    }
    Some(d65_spd(wavelength_nm)? * *D65_SCALE)
}

/// 返回连续 D65 光谱在 380–780 nm 上的归一化系数 k=1/∫D65(λ)ȳ(λ)dλ。
///
/// 输入：无；输出：缓存的 k，供圆盘光子发射与光谱相机共用同一能量标尺。
pub fn d65_normalization_continuous() -> f64 {
    *D65_SCALE
}

/// 返回连续 D65 白光积分后的 XYZ，供 RGB 圆盘照明与光谱照明共用白点。
///
/// 输入：无；输出：缓存的 ∫S(λ)·[x̄(λ),ȳ(λ),z̄(λ)]dλ，其中 Y=1。
/// 逐个 1 nm 区间用 Simpson 公式积分，保留相对 HDR 标尺。
pub fn d65_white_xyz() -> Vector3<f64> {
    static WHITE: LazyLock<Vector3<f64>> = LazyLock::new(|| {
        let f = |nm| cie_xyz(nm).unwrap() * d65_spectrum(nm).unwrap();
        (380..780)
            .map(|nm| {
                let nm = f64::from(nm);
                (f(nm) + f(nm + 0.5) * 4.0 + f(nm + 1.0)) / 6.0
            })
            .sum()
    });
    *WHITE
}

/// 把线性 RGB 发光颜色近似重建为指定波长的发光光谱，供 HDRI/背景的光谱相机使用。
///
/// 输入：非负有限线性 RGB（可大于 1）和 380–780 nm 波长。
///
/// 输出：每 nm 的标量光谱密度；负 RGB、非法波长或溢出返回 None。
/// 先令 m=max(RGB)，用 PBRT 独立 Illuminant 七基谱重建 RGB/m，再相对白基谱 W 校准：
/// Lλ=m·max(Bλ,0)/Wλ·S_D65(λ)，其中 Bλ 是组合基谱；m=0 时返回零。
/// 基谱在 380–720 nm 线性插值，720–780 nm 延用端点；只截负值，保留 HDR 强度。
/// 这是 RGB 到光谱的近似，不代表实测 HDRI 光谱，也不保证任意 RGB 色度精确还原。
pub fn rgb_emission(rgb: Vector3<f64>, wavelength_nm: f64) -> Option<f64> {
    if !(WAVELENGTH_MIN_NM..=WAVELENGTH_MAX_NM).contains(&wavelength_nm)
        || rgb.iter().any(|v| !v.is_finite() || *v < 0.0)
    {
        return None;
    }
    let scale = rgb.max();
    if scale == 0.0 {
        return Some(0.0);
    }
    let white = d65_spectrum(wavelength_nm)?;
    let result = if rgb.x == rgb.y && rgb.y == rgb.z {
        white * scale
    } else {
        let (spectrum, w) = super::smits_basis(&ILLUMINANT, rgb / scale, wavelength_nm);
        (spectrum.max(0.0) / w * white) * scale
    };
    (result.is_finite() && result >= 0.0).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_d65_normalizes_y_and_reconstructs_neutral_hdr_without_clipping() {
        let mut xyz = Vector3::zeros();
        for i in 0..4000 {
            let nm = 380.0 + (f64::from(i) + 0.5) * 0.1;
            xyz += cie_xyz(nm).unwrap() * d65_spectrum(nm).unwrap() * 0.1;
            assert!(
                (rgb_emission(Vector3::repeat(30.0), nm).unwrap()
                    - 30.0 * d65_spectrum(nm).unwrap())
                .abs()
                    < 1e-13
            );
        }
        assert!((xyz.y - 1.0).abs() < 1e-7);
        let sum = xyz.sum();
        assert!((xyz.x / sum - 0.3127).abs() < 3e-4);
        assert!((xyz.y / sum - 0.3290).abs() < 3e-4);
    }

    #[test]
    fn illuminant_reconstruction_preserves_hdr_gain_and_rejects_invalid_inputs() {
        let color = Vector3::new(0.3, 0.8, 0.1);
        for nm in [380.0, 450.0, 550.0, 650.0, 780.0] {
            let unit = rgb_emission(color, nm).unwrap();
            assert!((rgb_emission(color * 10000.0, nm).unwrap() - unit * 10000.0).abs() < 1e-10);
            assert_eq!(rgb_emission(Vector3::zeros(), nm), Some(0.0));
        }
        for value in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(rgb_emission(Vector3::repeat(value), 550.0).is_none());
        }
        for nm in [379.0, 781.0, f64::NAN] {
            assert!(rgb_emission(color, nm).is_none());
            assert!(d65_spectrum(nm).is_none());
        }
        assert!(ILLUMINANT.iter().all(|row| row[1] > 0.0));
    }
}

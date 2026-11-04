//! 相机光谱功率密度（每 nm）；D65 白点校准的 PBRT Illuminant 重建。
//! 发光基谱独立于反射率基谱，不对 HDR 强度施加 [0,1] 上限。

use super::spectrum::{cie_xyz, d65_spd};
use nalgebra::Vector3;
use std::sync::LazyLock;

pub const WAVELENGTH_MIN_NM: f64 = 380.0;
pub const WAVELENGTH_MAX_NM: f64 = 780.0;
/// 各等宽波长层等配额混合的边缘 PDF；分层估计的每项权重为 1/(PDF*spp)。
pub const WAVELENGTH_PDF: f64 = 1.0 / (WAVELENGTH_MAX_NM - WAVELENGTH_MIN_NM);

static ILLUMINANT: LazyLock<Vec<[f64; 8]>> = LazyLock::new(|| {
    include_str!("../../assets/color/pbrt_rgb_illuminant.csv")
        .lines()
        .skip(1)
        .map(|line| {
            let values: Vec<f64> = line
                .split(',')
                .map(|v| v.parse().expect("发光基谱数值必须有效"))
                .collect();
            values.try_into().expect("发光基谱必须有八列")
        })
        .collect()
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

/// 380..780 nm 积分后白光 Y=1；这里不含 Δλ 或相机的 1/p(λ)。
pub fn d65_spectrum(wavelength_nm: f64) -> Option<f64> {
    if !(WAVELENGTH_MIN_NM..=WAVELENGTH_MAX_NM).contains(&wavelength_nm) {
        return None;
    }
    Some(d65_spd(wavelength_nm)? * *D65_SCALE)
}

/// 380..780 nm 连续 D65 的归一化 k；与 Hero 直射使用同一固定发射光谱。
pub fn d65_normalization_continuous() -> f64 {
    *D65_SCALE
}

/// 与 d65_spectrum 相同连续归一化的白光 XYZ；RGB 圆盘 NEE 共用此标尺。
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

/// PBRT 的独立 Illuminant 七基谱，按 RGB 通道排序组合，再相对其白基谱校准为 D65。
/// RGB→光谱不唯一：这是发光近似，不保证重建 XYZ 精确还原任意 RGB 色度。
/// 先分离 max(RGB) 作为 HDR 强度，再重建色度；负 RGB、非有限输入及溢出返回 None。
/// 380..720 nm 线性插值，720..780 nm 延用端点；只去掉负光谱，不限制上界。
pub fn rgb_emission(rgb: Vector3<f64>, wavelength_nm: f64) -> Option<f64> {
    let white = d65_spectrum(wavelength_nm)?;
    if rgb.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return None;
    }
    let scale = rgb.max();
    if scale == 0.0 {
        return Some(0.0);
    }
    let rgb = rgb / scale;
    let table = &*ILLUMINANT;
    let nm = wavelength_nm.clamp(table[0][0], table[table.len() - 1][0]);
    let right = table.partition_point(|row| row[0] < nm).max(1);
    let left = right - 1;
    let t = (nm - table[left][0]) / (table[right][0] - table[left][0]);
    let [w, c, m, y, r, g, b] =
        std::array::from_fn(|i| table[left][i + 1] * (1.0 - t) + table[right][i + 1] * t);
    let (red, green, blue) = (rgb.x, rgb.y, rgb.z);
    let spectrum = if red <= green && red <= blue {
        red * w
            + if green <= blue {
                (green - red) * c + (blue - green) * b
            } else {
                (blue - red) * c + (green - blue) * g
            }
    } else if green <= red && green <= blue {
        green * w
            + if red <= blue {
                (red - green) * m + (blue - red) * b
            } else {
                (blue - green) * m + (red - blue) * r
            }
    } else {
        blue * w
            + if red <= green {
                (red - blue) * y + (green - red) * g
            } else {
                (green - blue) * y + (red - green) * r
            }
    };
    let result = (spectrum.max(0.0) / w * white) * scale;
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

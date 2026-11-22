//! RGB 反射光谱近似与波长（nm）到线性 CIE XYZ 的单个积分贡献。
//!
//! 使用 CIE 官方 1 nm 数据表，非整数波长做线性插值；来源与许可见
//! `assets/color/README.md`。数据通过 include_str! 随程序编译，运行时无需读文件。

pub mod emission;
use std::sync::LazyLock;

use nalgebra::Vector3;

static CIE_CMF: LazyLock<Vec<[f64; 4]>> =
    LazyLock::new(|| parse_csv(include_str!("../../assets/color/CIE_xyz_1931_2deg.csv")));
static D65_SPD: LazyLock<Vec<[f64; 2]>> =
    LazyLock::new(|| parse_csv(include_str!("../../assets/color/CIE_std_illum_D65.csv")));

// PBRT/Smits 的七条 RGB 反射基谱，第一列保存原始节点波长（nm）。
// CSV 首行为列名；来源与 BSD 许可见 assets/color/README.md。
static RGB_REFLECTANCE_BASIS: LazyLock<Vec<[f64; 8]>> = LazyLock::new(|| {
    let csv = include_str!("../../assets/color/pbrt_rgb_reflectance.csv");
    parse_csv(csv.split_once('\n').expect("RGB 基谱缺少表头").1)
});

/// 把线性 RGB 反射率近似重建为单个波长的反射率，供光谱材质求值。
///
/// 输入：各分量在 `[0,1]` 的有限 linear_rgb，以及正有限波长 wavelength_nm（nm）。
///
/// 输出：ρ(λ)∈`[0,1]`；非法输入返回 None。
/// 按 RGB 大小组合 PBRT Smits 七基谱 B，公式为 ρ(λ)=clamp(0.94·B(λ),0,1)。
/// 380–720 nm 节点间线性插值，表外延用端点；这是近似反射谱，不保证 D65 下精确还原 RGB。
/// 数据来源：<https://github.com/mmp/pbrt-v3/blob/master/src/core/spectrum.cpp>
pub fn rgb_reflectance(linear_rgb: Vector3<f64>, wavelength_nm: f64) -> Option<f64> {
    if !wavelength_nm.is_finite()
        || wavelength_nm <= 0.0
        || linear_rgb
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return None;
    }
    if linear_rgb == Vector3::zeros() {
        return Some(0.0);
    }

    let (reflectance, _) = smits_basis(&RGB_REFLECTANCE_BASIS, linear_rgb, wavelength_nm);
    Some((0.94 * reflectance).clamp(0.0, 1.0))
}

/// 查询一个波长对人眼三刺激值的响应，用于把光谱积分成颜色。
///
/// 输入：正有限波长（nm）；输出：CIE 1931 2° 匹配函数 [x̄(λ),ȳ(λ),z̄(λ)]，尚未积分。
/// 360–830 nm 之间按官方 1 nm 表线性插值，表外返回零；非正或非有限波长返回 None。
/// 数据来源与许可见 assets/color/README.md。
pub fn cie_xyz(wavelength_nm: f64) -> Option<Vector3<f64>> {
    let Some((left, right, t)) = table_bracket(&CIE_CMF, wavelength_nm)? else {
        return Some(Vector3::zeros());
    };
    let one_minus_t = 1.0 - t;
    let l = &CIE_CMF[left];
    let r = &CIE_CMF[right];
    Some(Vector3::new(
        l[1] * one_minus_t + r[1] * t,
        l[2] * one_minus_t + r[2] * t,
        l[3] * one_minus_t + r[3] * t,
    ))
}

/// 查询 D65 日光在某波长处的相对光谱功率，供光谱积分和白光归一化使用。
///
/// 输入：正有限波长（nm）；输出：D65(λ)，560 nm 处为 100。
/// 300–830 nm 内线性插值，表外返回零；非法波长返回 None。它不是每条光子的总能量。
pub fn d65_spd(wavelength_nm: f64) -> Option<f64> {
    let Some((left, right, t)) = table_bracket(&D65_SPD, wavelength_nm)? else {
        return Some(0.0);
    };
    Some(D65_SPD[left][1] * (1.0 - t) + D65_SPD[right][1] * t)
}

/// 把一个波长的功率密度转换成一份线性 XYZ 积分贡献。
///
/// 输入：波长 λ（nm）、功率密度 S(λ)、积分宽度 Δλ（nm）及所有波长共用的归一化系数 k。
///
/// 输出：ΔXYZ=k·S(λ)·[x̄(λ),ȳ(λ),z̄(λ)]·Δλ；非法输入或溢出返回 None。
/// S/k 须非负有限，Δλ 须正有限；梯形积分端点使用半宽度，不再除波长数。
/// 空间光子通量、透射率及像素面积权重由调用方另外处理。
pub fn wavelength_to_xyz(
    wavelength_nm: f64,
    spectral_power: f64,
    delta_lambda_nm: f64,
    k: f64,
) -> Option<Vector3<f64>> {
    if !spectral_power.is_finite()
        || spectral_power < 0.0
        || !delta_lambda_nm.is_finite()
        || delta_lambda_nm <= 0.0
        || !k.is_finite()
        || k < 0.0
    {
        return None;
    }

    let cmf = cie_xyz(wavelength_nm)?;
    let weight = k * spectral_power * delta_lambda_nm;
    let xyz = cmf * weight;
    xyz.iter().all(|value| value.is_finite()).then_some(xyz)
}

/// 为一组离散波长求 D65 白光的亮度归一化系数，避免改变步长时改变总亮度。
///
/// 输入：samples 中的 (波长 nm,梯形积分宽度 nm)；输出：k，使这组白光积分后 Y=1。
///
/// 公式：k=1/Σ[D65(λ_i)·ȳ(λ_i)·Δλ_i]。
/// 例如 380–780 nm、步长 10 nm 时，端点宽度为 5 nm，其余为 10 nm。
/// 空数组、非法样本、全零亮度或溢出返回 None；追踪前计算，丢失光子后不能重新归一化。
pub fn d65_normalization(samples: &[(f64, f64)]) -> Option<f64> {
    let mut total_y = 0.0;
    for &(wavelength_nm, delta_lambda_nm) in samples {
        total_y +=
            wavelength_to_xyz(wavelength_nm, d65_spd(wavelength_nm)?, delta_lambda_nm, 1.0)?.y;
    }
    if !total_y.is_finite() || total_y <= 0.0 {
        return None;
    }
    let k = total_y.recip();
    (k.is_finite() && k > 0.0).then_some(k)
}

/// 把编译进程序的颜色 CSV 表转换为定长数值行，供后续光谱查询使用。
///
/// 输入：每行恰有 N 列、无表头的 CSV 文本；输出：`Vec<[f64;N]>`。
/// 列数或数字格式错误会 panic，表示仓库内置数据损坏。
pub(super) fn parse_csv<const N: usize>(csv: &str) -> Vec<[f64; N]> {
    csv.lines()
        .map(|line| {
            let mut fields = line.split(',');
            let row = std::array::from_fn(|_| {
                fields
                    .next()
                    .expect("CIE 数据缺少列")
                    .parse::<f64>()
                    .expect("CIE 数据不是有效浮点数")
            });
            assert!(fields.next().is_none(), "CIE 数据包含多余列");
            row
        })
        .collect()
}

/// 按 PBRT Smits 七基谱组合线性 RGB，返回 `(组合值, 白基谱值)`；仅插值当前通道顺序实际使用的基谱列。
pub(super) fn smits_basis(table: &[[f64; 8]], rgb: Vector3<f64>, wavelength_nm: f64) -> (f64, f64) {
    let nm = wavelength_nm.clamp(table[0][0], table[table.len() - 1][0]);
    let right = table.partition_point(|row| row[0] < nm).max(1);
    let left = right - 1;
    let t = (nm - table[left][0]) / (table[right][0] - table[left][0]);
    let one_minus_t = 1.0 - t;
    let interp = |column: usize| table[left][column] * one_minus_t + table[right][column] * t;
    let white = interp(1);
    let (r, g, b) = (rgb.x, rgb.y, rgb.z);
    let combined = if r <= g && r <= b {
        r * white
            + if g <= b {
                let c = if g > r { (g - r) * interp(2) } else { 0.0 };
                let b_term = if b > g { (b - g) * interp(7) } else { 0.0 };
                c + b_term
            } else {
                let c = if b > r { (b - r) * interp(2) } else { 0.0 };
                let g_term = if g > b { (g - b) * interp(6) } else { 0.0 };
                c + g_term
            }
    } else if g <= r && g <= b {
        g * white
            + if r <= b {
                let m = if r > g { (r - g) * interp(3) } else { 0.0 };
                let b_term = if b > r { (b - r) * interp(7) } else { 0.0 };
                m + b_term
            } else {
                let m = if b > g { (b - g) * interp(3) } else { 0.0 };
                let r_term = if r > b { (r - b) * interp(5) } else { 0.0 };
                m + r_term
            }
    } else {
        b * white
            + if r <= g {
                let y = if r > b { (r - b) * interp(4) } else { 0.0 };
                let g_term = if g > r { (g - r) * interp(6) } else { 0.0 };
                y + g_term
            } else {
                let y = if g > b { (g - b) * interp(4) } else { 0.0 };
                let r_term = if r > g { (r - g) * interp(5) } else { 0.0 };
                y + r_term
            }
    };
    (combined, white)
}

/// 在波长间距为 1 nm 的内置数据表中定位插值区间，供 CIE/D65 共同使用。
///
/// 返回：`None` 表示非法波长，`Some(None)` 表示表外（应返回零），`Some(Some((left, right, t)))` 表示插值权重。
fn table_bracket<const N: usize>(
    table: &[[f64; N]],
    wavelength_nm: f64,
) -> Option<Option<(usize, usize, f64)>> {
    if !wavelength_nm.is_finite() || wavelength_nm <= 0.0 {
        return None;
    }
    let first_nm = table[0][0];
    let last_nm = table[table.len() - 1][0];
    if wavelength_nm < first_nm || wavelength_nm > last_nm {
        return Some(None);
    }

    let position = wavelength_nm - first_nm;
    let left = position.floor() as usize;
    let right = (left + 1).min(table.len() - 1);
    let t = position - left as f64;
    Some(Some((left, right, t)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_reflectance_matches_pbrt_samples_and_interpolates_with_endpoint_extension() {
        assert_eq!(RGB_REFLECTANCE_BASIS.len(), 32);
        // 独立锚点来自 PBRT 的 RGBRefl2SpectRed/Blue，而非 CIE 颜色表。
        let red_at_380 = 0.94 * 1.657_560_486_708_618e-1;
        let red_at_next = 0.94 * 1.1846442802747797e-1;
        assert!((rgb_reflectance(Vector3::x(), 380.0).unwrap() - red_at_380).abs() < 1e-14);
        assert!(
            (rgb_reflectance(Vector3::z(), 380.0).unwrap() - 0.94 * 9.920_977_146_972_068e-1).abs()
                < 1e-14
        );
        let midpoint = (380.0 + 390.967743) / 2.0;
        assert!(
            (rgb_reflectance(Vector3::x(), midpoint).unwrap() - (red_at_380 + red_at_next) / 2.0)
                .abs()
                < 1e-14
        );
        assert_eq!(
            rgb_reflectance(Vector3::x(), 360.0),
            rgb_reflectance(Vector3::x(), 380.0)
        );
        assert_eq!(
            rgb_reflectance(Vector3::x(), 780.0),
            rgb_reflectance(Vector3::x(), 720.0)
        );
        assert!(rgb_reflectance(Vector3::x(), 650.0).unwrap() > 0.8);
        assert!(rgb_reflectance(Vector3::x(), 450.0).unwrap() < 0.05);
        assert!(rgb_reflectance(Vector3::z(), 450.0).unwrap() > 0.8);
        assert!(rgb_reflectance(Vector3::z(), 650.0).unwrap() < 0.1);
        // PBRT 七基谱组合的六种 RGB 排序，固定参考值检查二次色和通道差值。
        for (rgb, expected) in [
            (Vector3::new(0.2, 0.5, 0.8), 0.7731004868887764),
            (Vector3::new(0.2, 0.8, 0.5), 0.4940760664753345),
            (Vector3::new(0.5, 0.2, 0.8), 0.7597784062710301),
            (Vector3::new(0.8, 0.2, 0.5), 0.5267500564516008),
            (Vector3::new(0.5, 0.8, 0.2), 0.20195544183758296),
            (Vector3::new(0.8, 0.5, 0.2), 0.24795151243159563),
        ] {
            assert!((rgb_reflectance(rgb, 380.0).unwrap() - expected).abs() < 1e-14);
        }
    }

    #[test]
    fn rgb_reflectance_is_bounded_for_all_channel_orderings_and_rejects_invalid_input() {
        for r in [0.0, 0.2, 0.5, 0.8, 1.0] {
            for g in [0.0, 0.2, 0.5, 0.8, 1.0] {
                for b in [0.0, 0.2, 0.5, 0.8, 1.0] {
                    for nm in 360..=830 {
                        let reflectance =
                            rgb_reflectance(Vector3::new(r, g, b), f64::from(nm)).unwrap();
                        assert!((0.0..=1.0).contains(&reflectance));
                        if r == 0.0 && g == 0.0 && b == 0.0 {
                            assert_eq!(reflectance, 0.0);
                        }
                    }
                }
            }
        }
        for value in [-0.1, 1.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for channel in 0..3 {
                let mut rgb = Vector3::repeat(0.5);
                rgb[channel] = value;
                assert_eq!(rgb_reflectance(rgb, 550.0), None);
            }
        }
        for nm in [0.0, -550.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(rgb_reflectance(Vector3::repeat(0.5), nm), None);
        }
    }

    fn assert_xyz(actual: Vector3<f64>, expected: Vector3<f64>, tolerance: f64) {
        assert!(
            (actual - expected).norm() < tolerance,
            "{actual:?} != {expected:?}"
        );
    }

    fn visible_samples(step_nm: usize) -> Vec<(f64, f64)> {
        (380..=780)
            .step_by(step_nm)
            .map(|nm| {
                let width = if nm == 380 || nm == 780 {
                    step_nm as f64 / 2.0
                } else {
                    step_nm as f64
                };
                (nm as f64, width)
            })
            .collect()
    }

    #[test]
    fn official_tables_have_complete_one_nm_grids_and_metadata_column_sums() {
        fn check<const N: usize>(table: &[[f64; N]], first_nm: f64, count: usize) {
            assert_eq!(table.len(), count);
            for (index, row) in table.iter().enumerate() {
                assert_eq!(row[0], first_nm + index as f64);
                assert!(row.iter().all(|value| value.is_finite() && *value >= 0.0));
            }
        }
        check(&CIE_CMF, 360.0, 471);
        check(&D65_SPD, 300.0, 531);
        // 独立参考值来自随表保存的官方 metadata，而不是本实现的积分结果。
        for (column, expected) in [
            280245.0,
            106.865469489595,
            106.8569171011719,
            106.892251278636,
        ]
        .into_iter()
        .enumerate()
        {
            let sum: f64 = CIE_CMF.iter().map(|row| row[column]).sum();
            assert!((sum - expected).abs() < 1e-9);
        }
        let sum: f64 = D65_SPD.iter().map(|row| row[1]).sum();
        assert!((sum - 40852.204).abs() < 1e-8);
    }

    #[test]
    fn known_cmf_and_d65_samples_keep_xyz_channel_order() {
        for (nm, expected) in [
            (450.0, Vector3::new(0.3362, 0.038, 1.77211)),
            (550.0, Vector3::new(0.4334499, 0.9949501, 0.008749999)),
            (555.0, Vector3::new(0.5120501, 1.0, 0.005749999)),
            (650.0, Vector3::new(0.2835, 0.107, 0.0)),
        ] {
            assert_xyz(cie_xyz(nm).unwrap(), expected, 1e-12);
        }
        assert_eq!(d65_spd(450.0), Some(117.008));
        assert_eq!(d65_spd(560.0), Some(100.0));
    }

    #[test]
    fn fractional_wavelengths_linearly_interpolate_both_tables() {
        let expected = Vector3::new(0.3362, 0.038, 1.77211) * 0.75
            + Vector3::new(0.3331977, 0.03984667, 1.7682589) * 0.25;
        assert_xyz(cie_xyz(450.25).unwrap(), expected, 1e-12);
        assert!((d65_spd(560.25).unwrap() - (100.0 * 0.75 + 99.6334 * 0.25)).abs() < 1e-12);
    }

    #[test]
    fn endpoints_are_included_and_outside_tables_is_zero() {
        assert_xyz(
            cie_xyz(360.0).unwrap(),
            Vector3::new(0.0001299, 0.000003917, 0.0006061),
            1e-15,
        );
        assert_xyz(
            cie_xyz(830.0).unwrap(),
            Vector3::new(0.000001251141, 0.00000045181, 0.0),
            1e-15,
        );
        assert_eq!(d65_spd(300.0), Some(0.0341));
        assert_eq!(d65_spd(830.0), Some(60.3125));
        for nm in [1.0, 359.99, 830.01, f64::MAX] {
            assert_eq!(cie_xyz(nm), Some(Vector3::zeros()));
        }
        for nm in [1.0, 299.99, 830.01, f64::MAX] {
            assert_eq!(d65_spd(nm), Some(0.0));
        }
    }

    #[test]
    fn xyz_contribution_includes_power_bin_width_and_shared_k() {
        // 555 nm：S = 2，Δλ = 10 nm，k = 0.01，故整条 CMF 乘 0.2。
        let expected = Vector3::new(0.10241002, 0.2, 0.0011499998);
        assert_xyz(
            wavelength_to_xyz(555.0, 2.0, 10.0, 0.01).unwrap(),
            expected,
            1e-12,
        );
        for (power, width, k) in [(0.0, 10.0, 1.0), (2.0, 10.0, 0.0)] {
            assert_eq!(
                wavelength_to_xyz(555.0, power, width, k),
                Some(Vector3::zeros())
            );
        }
    }

    #[test]
    fn d65_integral_matches_white_point_and_is_stable_across_sample_counts() {
        let mut whites = Vec::new();
        for step_nm in [1, 5, 10] {
            let samples = visible_samples(step_nm);
            let k = d65_normalization(&samples).unwrap();
            // 相对 SPD 以 100 为量级；归一化尺度应约为 1 / 10567。
            assert!((k - 0.00009463).abs() < 1e-7);
            let xyz = samples.iter().fold(Vector3::zeros(), |sum, &(nm, width)| {
                sum + wavelength_to_xyz(nm, d65_spd(nm).unwrap(), width, k).unwrap()
            });
            assert!((xyz.y - 1.0).abs() < 1e-12);
            let total = xyz.x + xyz.y + xyz.z;
            assert!((xyz.x / total - 0.3127).abs() < 3e-4);
            assert!((xyz.y / total - 0.3290).abs() < 3e-4);
            whites.push(xyz);
        }
        assert_xyz(whites[0], whites[1], 0.002);
        assert_xyz(whites[0], whites[2], 0.002);
    }

    #[test]
    fn invalid_inputs_and_overflow_are_rejected() {
        for nm in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(cie_xyz(nm).is_none());
            assert!(d65_spd(nm).is_none());
            assert!(wavelength_to_xyz(nm, 1.0, 10.0, 1.0).is_none());
            assert!(d65_normalization(&[(nm, 10.0)]).is_none());
        }
        for invalid in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(wavelength_to_xyz(555.0, invalid, 10.0, 1.0).is_none());
            assert!(wavelength_to_xyz(555.0, 1.0, invalid, 1.0).is_none());
            assert!(wavelength_to_xyz(555.0, 1.0, 10.0, invalid).is_none());
        }
        assert!(wavelength_to_xyz(555.0, 1.0, 0.0, 1.0).is_none());
        assert!(wavelength_to_xyz(555.0, f64::MAX, 10.0, 1.0).is_none());
        assert!(wavelength_to_xyz(450.0, f64::MAX, 1.0, 1.0).is_none());
        assert!(d65_normalization(&[]).is_none());
        assert!(d65_normalization(&[(900.0, 10.0)]).is_none());
        assert!(d65_normalization(&[(555.0, 0.0)]).is_none());
        assert!(d65_normalization(&[(555.0, f64::MAX)]).is_none());
        assert!(d65_normalization(&[(555.0, f64::from_bits(1))]).is_none());
        // 每项均有限，但两个贡献求和溢出时也必须拒绝。
        assert!(d65_normalization(&[(555.0, 1e306), (555.0, 1e306)]).is_none());
    }
}

//! RGB 反射光谱近似与波长（nm）到线性 CIE XYZ 的单个积分贡献。
//!
//! 使用 CIE 官方 1 nm 数据表，非整数波长做线性插值；来源与许可见
//! `assets/color/README.md`。数据通过 include_str! 随程序编译，运行时无需读文件。

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

/// 用 PBRT 的 Smits 七基谱近似，从线性 RGB 反射率重建 rho(λ)。
///
/// RGB 各分量必须有限且位于 [0, 1]，波长必须为正有限的 nm。
/// 在原始 380–720 nm 节点间线性插值，表外延用最近端点值。
/// 按 PBRT 乘 0.94，并将最终反射率限制到 [0, 1] 以保持能量约束。
/// 这是 RGB 到光谱的近似，不是实测材质数据，也不保证 D65 下精确还原 RGB。
/// 来源：https://github.com/mmp/pbrt-v3/blob/master/src/core/spectrum.cpp
pub fn rgb_reflectance(linear_rgb: Vector3<f64>, wavelength_nm: f64) -> Option<f64> {
    if !wavelength_nm.is_finite()
        || wavelength_nm <= 0.0
        || linear_rgb
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return None;
    }

    let table = &*RGB_REFLECTANCE_BASIS;
    let wavelength_nm = wavelength_nm.clamp(table[0][0], table[table.len() - 1][0]);
    let right = table.partition_point(|row| row[0] < wavelength_nm).max(1);
    let left = right - 1;
    let t = (wavelength_nm - table[left][0]) / (table[right][0] - table[left][0]);
    let [white, cyan, magenta, yellow, red, green, blue] = std::array::from_fn(|index| {
        table[left][index + 1] * (1.0 - t) + table[right][index + 1] * t
    });

    let (r, g, b) = (linear_rgb.x, linear_rgb.y, linear_rgb.z);
    let reflectance = if r <= g && r <= b {
        r * white
            + if g <= b {
                (g - r) * cyan + (b - g) * blue
            } else {
                (b - r) * cyan + (g - b) * green
            }
    } else if g <= r && g <= b {
        g * white
            + if r <= b {
                (r - g) * magenta + (b - r) * blue
            } else {
                (b - g) * magenta + (r - b) * red
            }
    } else {
        b * white
            + if r <= g {
                (r - b) * yellow + (g - r) * green
            } else {
                (g - b) * yellow + (r - g) * red
            }
    };
    Some((0.94 * reflectance).clamp(0.0, 1.0))
}

/// CIE 1931 2° 颜色匹配函数 [x̄(λ), ȳ(λ), z̄(λ)]，不是已经积分的 XYZ。
///
/// 表格覆盖 360–830 nm。按照官方元数据，表外的有效波长返回零；
/// 非正、NaN 或无穷波长返回 None。第一周渲染使用其中的 380–780 nm。
pub fn cie_xyz(wavelength_nm: f64) -> Option<Vector3<f64>> {
    let row = interpolate(&CIE_CMF, wavelength_nm)?;
    Some(Vector3::new(row[1], row[2], row[3]))
}

/// D65 的相对光谱功率 S(λ)，560 nm 处为 100，不是光子的总能量。
///
/// 表格覆盖 300–830 nm，插值与非法输入规则同 cie_xyz。
pub fn d65_spd(wavelength_nm: f64) -> Option<f64> {
    Some(interpolate(&D65_SPD, wavelength_nm)?[1])
}

/// 计算单个波长样本的线性 XYZ 贡献：k × S(λ) × CMF(λ) × Δλ。
///
/// spectral_power 是该波长处的功率密度 S(λ)，delta_lambda_nm 是积分宽度，
/// k 是所有波长共用的归一化因子。使用 D65 时可由 d65_normalization 计算 k。
/// 梯形积分的两个端点应传半个宽度；本函数不额外除以波长样本数。
/// 空间光子通量、透射率等权重由调用方另外相乘，再累积到 XYZ Framebuffer。
///
/// 功率和 k 必须有限且非负，宽度必须有限且为正；非法输入或溢出返回 None。
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

/// 用同一组波长和积分宽度计算 D65 的 k = 1 / Σ[S(λ) ȳ(λ) Δλ]。
///
/// samples 中每项为 (波长 nm, 积分宽度 nm)，例如 380–780 nm 的 10 nm
/// 梯形积分使用 41 点：两个端点宽度为 5，其余为 10。累加这些样本的 XYZ 后，
/// 未经过滤的 D65 白光 Y = 1。这个 k 必须在追踪前计算，丢失光子后不要重新计算。
/// 空数组、非法样本、全零 Y、求和或倒数溢出均返回 None。
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

// 只解析编译进程序的官方表格；格式错误表示仓库内的数据已被损坏。
fn parse_csv<const N: usize>(csv: &str) -> Vec<[f64; N]> {
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

// 两张表都是 1 nm 等间距数据，首列是波长，其余列按同一比例插值。
fn interpolate<const N: usize>(table: &[[f64; N]], wavelength_nm: f64) -> Option<[f64; N]> {
    if !wavelength_nm.is_finite() || wavelength_nm <= 0.0 {
        return None;
    }
    let first_nm = table[0][0];
    let last_nm = table[table.len() - 1][0];
    if wavelength_nm < first_nm || wavelength_nm > last_nm {
        return Some([0.0; N]);
    }

    let position = wavelength_nm - first_nm;
    let left = position.floor() as usize;
    let right = (left + 1).min(table.len() - 1);
    let t = position - left as f64;
    Some(std::array::from_fn(|column| {
        table[left][column] * (1.0 - t) + table[right][column] * t
    }))
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

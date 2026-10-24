//! 玻璃折射率的计算模式。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IorMode {
    /// 所有波长均使用折射率 1.52。
    Constant,
    /// 按输入波长（nm）计算 Cauchy 色散。
    Dispersion,
}

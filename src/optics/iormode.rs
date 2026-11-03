//! 玻璃折射率的计算模式。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IorMode {
    /// 所有波长均使用材质的常数折射率（无材质实验入口默认 1.52）。
    Constant,
    /// 按输入波长（nm）查询材质的 Cauchy 模型；未配置色散的介质仍使用常数。
    Dispersion,
}

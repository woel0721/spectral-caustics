# Codex Instructions — Spectral Caustics Renderer

本项目是一个 Rust CPU 离线光谱焦散渲染器的分阶段学习项目。**先阅读 `ROADMAP.md`，并以它作为任务范围和验收标准。**

## 当前目标

第一周仅实现：平行白光 -> 三棱镜入射折射 -> 玻璃内部传播 -> 出射折射 -> 地面光子累积 -> CIE XYZ -> sRGB PNG。第一版输出地面焦散俯视图，不要求渲染完整玻璃摄影画面。

## 开发规则

- 一次只实现 `ROADMAP.md` 中一个明确的 Day/里程碑，不要一次生成完整项目。
- 优先可运行、可测试、可解释的最小实现；不额外引入 GUI、GPU、BVH、KD-Tree、Path Tracer、完整 PBR。
- 使用 `nalgebra` 做数学、`image` 输出 PNG；其他库仅按当前任务必要性添加。
- 光学/几何优先 `f64`；光谱单位显式标注 nm，Cauchy 公式用 μm。方向归一化、求交 epsilon、全反射、NaN 都要处理。
- 能量在内部用线性值累积；颜色转换按 CIE XYZ -> linear sRGB -> display sRGB，不能直接把线性 RGB 乘 255 当 sRGB。
- 对积分/Monte Carlo 波长采样做合理的 `Δλ` 或 `1 / p(λ)` 加权，空间发射数量增加时保持总能量不变。
- 标准颜色数据需有可追溯来源；不要虚构 CIE/D65 数据，不要把 RGB 插值冒充真实光谱模型。
- 默认使用固定随机种子，保证可复现；物理简化在 README 中明确写出。
- 每阶段结束运行 `cargo fmt`、`cargo test`，并在合适时运行 `cargo clippy -- -D warnings`。
- 最终回复报告：做了什么、改了什么文件、真实运行了哪些检查、尚有哪些限制、下一阶段建议。

## 默认工作方式

若用户没有指定阶段，先询问是否从 Day 1 开始，**不要擅自完成整份路线图**。

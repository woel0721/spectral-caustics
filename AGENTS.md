# Codex Instructions — Spectral Caustics Renderer

本项目是分阶段开发的 Rust CPU 离线光谱焦散渲染器。先阅读 `ROADMAP.md` 和当前源码；路线图中的第一周限制及历史阶段说明不是当前实现状态。以用户本次指定的范围和验收为准，不自动推进后续阶段。

## 当前基线：E2

- 光源端：平行光经三棱镜双界面折射，包含 Fresnel 透射通量权重；地面 XYZ 累积、CIE/D65 光谱积分及 sRGB PNG。
- 相机端：最多八次玻璃 Fresnel 反射/折射；解析太阳、HDRI/渐变环境照明、固定配额 Balance/Power MIS、有限深度 Lambertian 路径与 Russian Roulette。
- E2 允许 `max_diffuse_bounces=0..1024`，Rust 缺省为 0，展示 JSON 保持 1。0/1 须保留 E1 的结果、浮点分组与 RNG 顺序。后续 Lambertian 顶点只接收太阳/环境直接光，归入 Direct；miss/玻璃不继续，不能重复计入逃逸 HDRI、焦散图或完整地面图。
- 路径迭代更新 RGB 吞吐量、当前 Ray、radiance 与独立玻璃/漫反射计数。第三次漫反射后且有剩余预算时执行 RR，p=clamp(max(RGB throughput),0.05,0.95)，存活除 p 补偿；最后顶点和 0/1 不抽 RR。
- 相机像素使用独立确定性 RNG 流与 Rayon；Direct/Caustics/Combined 共用路径。
- `src/render/integrator.rs` 是入口，具体职责位于其子模块；`src/scene.rs` 保留 Scene 主类型，构建、物体、光照、棋盘格和光子追踪分开实现。
- Rust 内置/旧 JSON 缺省值为环境样本 1、Balance、漫反射反弹 0；`configs/default.json` 显式使用 8、Power、1。相机 SPP 的 Rust 缺省值为 64，展示 JSON 为 256；两者不可混同。

这是有界的 Lambertian 路径估计器，不是通用无偏 Path Tracer。未经当前任务明确授权，不增加漫反射后的玻璃续追、新材质、透明 shadow ray、环境焦散、GUI/GPU、BVH/KD-Tree 或通用 Photon Mapping。

## 开发与重构规则

- 一次完成用户指定的里程碑；未指定时询问目标，不默认重做 Day 1。
- 保持公开 API 和旧兼容语义；审计引用后才删除确实无用的私有代码。公开接口、回归测试和对照估计器需明确授权后才能删除。
- 重构不得改变运算/采样顺序、RNG 消费、MIS/PDF、Fresnel、光谱/颜色、焦散或 E2 计光规则。主要改动独立验证和提交。
- 优先可运行、可测试、可解释的最小实现；减少实际重复，不为减少行数增加间接层。
- 使用 `nalgebra` 做数学、`image` 输出 PNG；其他库仅按当前任务必要性添加。
- 光学/几何优先 `f64`；光谱单位显式标注 nm，Cauchy 公式用 μm。处理归一化、epsilon、全反射、NaN 和溢出；重构时保留现有错误语义。
- 能量在线性空间累积；CIE XYZ → linear sRGB → display sRGB。波长积分保留 `Δλ` 或 `1/p(λ)` 权重，增加空间发射数量时总能量不变。
- 标准颜色数据须可追溯；不虚构 CIE/D65 数据，不把 RGB 插值冒充真实光谱模型。
- 固定 seed 可复现；保持独立随机流及 Rayon 1/2/4 线程结果一致，Direct + Caustics ≈ Combined。物理简化、兼容行为写入 README。
- 阅读并保留工作区已有改动，不覆盖用户未提交的配置或其他工作。

## 验收与报告

运行 `cargo fmt`、`cargo fmt --check`、`cargo test --locked`、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check`。重构先建立基线；重点核对固定 seed、线程数、Balance/Power、E1 0/1、三个输出分量、玻璃和光源焦散。

最终报告修改文件/函数、实际消除的重复、保留的兼容与冗余、真实检查结果及限制；不把未执行的检查或后续工作写成已完成。

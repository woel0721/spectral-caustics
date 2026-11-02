# Codex Instructions — Spectral Caustics Renderer

本项目是分阶段开发的 Rust CPU 离线光谱焦散渲染器。先阅读 `ROADMAP.md` 和当前源码；路线图描述当前实现与边界，Git 历史中的阶段说明不构成当前约束。以用户本次指定的范围和验收为准，不自动推进后续阶段。

## 当前功能与边界

- 光源端：平行光经三棱镜双界面折射，包含 Fresnel 透射通量权重；地面 XYZ 累积、CIE/D65 光谱积分及 sRGB PNG。
- 相机端：最多八次玻璃 Fresnel 反射/折射；解析太阳、HDRI/渐变环境照明、固定配额 Balance/Power MIS、有限深度 Lambertian 路径与 Russian Roulette。
- `max_diffuse_bounces=0..1024`，Rust 缺省为 0，展示 JSON 使用 4。后续 Lambertian 顶点只接收太阳/环境直接光，归入 Direct；玻璃事件可在漫反射后继续，但漫反射后的 miss 暂不计逃逸环境光（下一阶段处理），且不能重复计入焦散图。
- 路径迭代更新 RGB 吞吐量、当前 Ray、radiance 与独立玻璃/漫反射计数。第三次漫反射后且有剩余预算时执行 RR，p=clamp(max(RGB throughput),0.05,0.95)，存活除 p 补偿；最后顶点和 0/1 不抽 RR。
- 相机像素使用独立确定性 RNG 流与 Rayon；Direct/Caustics/Combined 共用路径。
- `src/render/integrator.rs` 是入口，具体职责位于其子模块；`src/scene.rs` 保留 Scene 主类型，构建、物体、光照、棋盘格和光子追踪分开实现。
- Rust 内置缺省值为环境样本 1、Balance、漫反射反弹 0；`configs/default.json` 显式使用 8、Power、4。相机 SPP 的 Rust 缺省值为 64，展示 JSON 为 256；两者不可混同。

这是有界的 Lambertian 路径估计器，不是通用无偏 Path Tracer。F2b 第一阶段已授权漫反射后的玻璃续追；不要未经明确授权继续添加透明 shadow ray、环境焦散、新材质、GUI/GPU、BVH/KD-Tree 或通用 Photon Mapping，也不要在下一阶段 MIS 设计前直接放开漫反射后的环境逃逸累加。

## 开发与重构规则

- 一次完成用户指定的里程碑；未指定时询问目标，不默认重做 Day 1。
- 本次 Breaking Cleanup 已授权删除历史兼容 API、冻结算法、历史测试适配层和无意义的重复测试。先审计全仓库引用、迁移有效调用方，再删除；当前核心算法与数值正确性测试必须保留。首次命中直接着色，后续不得恢复延迟着色或历史分量选择分支。
- 不要求与旧版本逐位相同，也不冻结旧浮点分组和 RNG 消耗顺序。当前实现自身必须保持固定 seed 可复现及 Rayon 1/2/4 线程结果一致，并验证 MIS/PDF、Fresnel、光谱/颜色、焦散与分量计光正确性。主要改动分阶段验证和提交。
- 优先可运行、可测试、可解释的最小实现；减少实际重复，不为减少行数增加间接层。
- 使用 `nalgebra` 做数学、`image` 输出 PNG；其他库仅按当前任务必要性添加。
- 光学/几何优先 `f64`；光谱单位显式标注 nm，Cauchy 公式用 μm。处理归一化、epsilon、全反射、NaN 和溢出；错误应明确且可测试。
- 能量在线性空间累积；CIE XYZ → linear sRGB → display sRGB。波长积分保留 `Δλ` 或 `1/p(λ)` 权重，增加空间发射数量时总能量不变。
- 标准颜色数据须可追溯；不虚构 CIE/D65 数据，不把 RGB 插值冒充真实光谱模型。
- 固定 seed 可复现；保持独立随机流及 Rayon 1/2/4 线程结果一致，Direct + Caustics ≈ Combined。物理简化和当前 API 写入 README。
- 阅读并保留工作区已有改动，不覆盖用户未提交的配置或其他工作。

## 验收与报告

运行 `cargo fmt`、`cargo fmt --check`、`cargo build --locked --all-targets`、`cargo test --locked`、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check`，并实际运行当前主要相机/焦散示例。清理前建立测试基线；重点核对固定 seed、线程数、Balance/Power、路径深度、RR、三个输出分量、玻璃和光源焦散，不保留冻结旧算法作为验收基准。

最终报告删除的文件/函数/测试数量、实际消除的重复、保留的重要测试与冗余、API/渲染语义变化、真实检查结果及限制；不把未执行的检查或后续工作写成已完成。

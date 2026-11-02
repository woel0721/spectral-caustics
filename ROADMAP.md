# Spectral Caustics Renderer — 当前状态与边界

本路线图描述当前实现，不作为自动推进后续算法的指令。下一项里程碑由用户指定；历史 Day/E/F 阶段实现细节与旧验收数字已由 Git 历史保存。

## 已实现

- [x] 米制 Z-up 几何、单三棱镜/有限地面配置、材质绑定与最近交点。
- [x] 双界面 Snell 折射、Cauchy 色散、Fresnel 透射通量与地面双线性 XYZ 累积。
- [x] 可追溯 CIE/D65 梯形积分、线性颜色与固定曝光 sRGB PNG。
- [x] 通用 BsdfSample RGB 相机路径，支持 Lambertian、Dielectric、Mirror、GgxMetal；共享最多 32 次 Delta 事件预算。
- [x] HDRI/渐变环境、重要性采样与固定配额 Balance/Power MIS。
- [x] 0..1024 次额外连续 BSDF 连接（JSON 字段名保留）、RGB 吞吐量与补偿 RR。
- [x] Direct/Caustics/Combined 共用相机路径，解析太阳避免重复计光。
- [x] JSON 配置校验、固定种子、Rayon 独立流与线程一致性。
- [x] Breaking Cleanup：统一当前算法，移除冻结算法、历史适配层和调试流程。

## Breaking Cleanup 引用审计

基于最新 main `1b14775`，清理前工作区干净。全仓库检索定义、实际调用、重导出、配置、示例与文档，不把仅测试使用等同于无价值。

| 候选 | 清理前实际用途 | 处理 |
| --- | --- | --- |
| 三份冻结路径实现及逐位/hash 对照 | 仅历史测试 | 删除算法副本及对照测试 |
| `trace_diffuse_path`、`sample_one_diffuse_indirect`、`trace_camera_ray` | 仅测试适配；生产路径不调用 | 删除生产模块中的入口，测试使用当前状态/`test_support` |
| `render_camera_glass_with_lighting` | 仅单图测试链 | 测试直接检查三图结果，删除分量选择分支和枚举 |
| `render_camera_glass_with_ground` | 一份完整地面图示例及测试 | 用现有解析太阳+焦散流程替代，删除整图替换模式及示例 |
| `take_photon` | 仅投影落点的测试 | 改用 `take_photon_landing`，删除丢弃权重的公开 wrapper |
| `render_camera_lit`、`render_camera_debug`、灰度地面/波长入口 | main、早期调试测试 | main 改为当前三分量输出，有效数值测试迁移，删除历史流程 |
| `save_png`、`Render::settings`、法线颜色工具 | 无当前生产调用 | 显式曝光保存，删除无用 accessor/调试工具 |
| `render_camera_glass` | 不绑定接收地面图的 RGB 场景与测试 | 保留；支持彩色地面和任意代码场景，不等价于光谱地面绑定 |
| `render_spectral_caustics`、`Render::render/render_mode` | 主图/折射率模式对照、能量测试 | 保留门面，内部统一为分量累积后相加 |
| `take_photon_landing/from_hit`、材质/几何/光谱工具 | 加权单光子、批量生产与物理测试 | 保留当前算法和数据结构 |
| `SpectralLighting::direct/combined` | 光源能量检查/总辐照度预览 | 保留；相机只读取焦散，不叠加 sampled direct |
| JSON 可省略字段及默认值 | 当前配置功能 | 保留规则与测试，不承诺冻结历史图像 |

## 删除与替代

删除的废弃源码/示例文件：

- `src/render/integrator/diffuse_tests/legacy.rs`
- `src/render/integrator/e2_tests/legacy_e1.rs`
- `src/render/integrator/f1_tests/legacy_e2.rs`
- `src/render/camera_debug.rs`
- `examples/camera_glass.rs`
- `examples/camera_glass_with_ground.rs`

`e2_tests.rs` 的有效测试整理为 `bounce_tests.rs`；`f1_tests.rs` 的事件状态测试整理为 `path_tests.rs`，删除原文件及冻结比较。当前共享状态 helper 位于 `test_support.rs`，不含冻结传输实现。

移除 30 个非 `#[test]` 函数名（按名称去重；包含被现代实现替代的入口，不等于 30 份独立算法）：

- 相机/地面/输出：`render_camera_glass_with_ground`、`render_camera_glass_with_lighting`、`render_camera_lit`、`render_camera_debug`、`render_camera_glass_impl`、`bind_ground_lighting`、`take_photon`、`save_png`、`normal_to_rgb8`、`Render::ground_hits`、`Render::refracted_hits`、`Render::settings`、`CameraPathComponents::select`。
- 灰度光源链：`render_ground_hits`、`render_refracted_ground_hits`、`render_landing_points`、`render_landing_point_sets`、`render_photon_landing_sets`。
- 历史算法及测试工具：`trace_camera_path_2521356`、`trace_camera_path_e1`、`trace_camera_path_e2`、`cosine_environment_lighting`、`random_mixture_environment_lighting`、`render_diffuse`、`partial_reference_image`、`fingerprint`、`trace_diffuse_path`、`sample_one_diffuse_indirect`、`trace_camera_ray`。
- 延迟着色：`accumulate_first_surface` 改为当前 `first_surface_lighting`，不再保存 `FirstSurface`；同时删除 `e1_diffuse_weights`、可选 `SunGroundLighting` 及 `GroundLightingComponent`。

历史生成 PNG 共 18 张退出版本控制；淘汰的调试图删除，当前命令仍可在忽略的 `output/` 中生成结果。未删除 HDRI、CIE/D65、PBRT 数据或其许可证。

## 行为变化与保留的结构

首次表面立即着色，不再等待漫反射链结束。深度 1 与更深路径共用 RGB 吞吐量乘法，错误也按当前顶点求值顺序返回；不保留历史加法分组或 RNG 消耗契约。光源总图统一为 `direct + caustics`，可能出现末位舍入差异。

Direct 包含解析太阳、环境和有界漫反射贡献；F2b 第二阶段也将漫反射后经玻璃逃逸的环境计入 Direct。Caustics 仍只来自首次地面顶点，不能在续追中重复读取焦散图。RR、Fresnel、MIS/PDF 和光谱能量的物理含义不变。

仍保留的有用冗余：独立 RGB 单缓冲相机与光谱三图相机共用一份路径/像素循环，但有两种缓冲写入分支，避免 RGB 场景分配三张图；光源保留 sampled direct 用于守恒验证。单光子及独立逐波长发射测试参考用于数值验证，不是冻结生产算法。Render 门面与自由函数分别提供配置/线程池集成和直接调用。

F2b 第一阶段已允许漫反射后的玻璃传播；暂不增加透明 shadow ray、新材质、环境焦散、GUI/GPU、BVH/KD-Tree 或通用 Photon Mapping。当前物理简化及运行方法见 [README.md](README.md)。

## 验收记录

清理前：309 项单元测试、38 项集成测试通过，3 项历史性能比较忽略，共 350 项测试定义。

清理后：删除 39 项测试（含 3 项 ignored 比较），补充 3 项当前行为测试，另外 8 项重命名/改写；净减少 36 项。现为 277 项单元测试 + 37 项集成测试，全部运行，无 ignored 测试。新增测试检查首次即时着色，以及当前光谱 splat 的 XYZ 守恒/边界/非法能量。

保留的重要验证包括：两平面独立视因子级数（2/3/5 深度）、RR 存活概率边界与均值补偿、Balance/Power 配额与 PDF、HDRI 数值求积、Fresnel 两界面能量、玻璃八事件上限、焦散不受太阳阴影重复衰减、光谱积分与 PNG、异常输入/溢出、配置往返、命令行真实输出，以及当前 0/1/2/3/5 深度在 1/2/4 线程的确定性和分量可加性。

最终实际通过：`cargo fmt --check`、`cargo build --locked --all-targets`、`cargo test --locked`、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check`（含暂存区检查）。另完成 release 全目标构建。

| 实跑 | 参数 | 结果 |
| --- | --- | --- |
| `camera_direct_caustics` | 默认场景 512×512，8 SPP | 三张 RGB8 PNG 成功 |
| `diffuse_indirect` | 320×240，16 SPP，深度 0/1/4 | 三张 RGB8 PNG 成功，输出互不相同 |
| `camera_parallel_benchmark` | 64×64，16 SPP，环境样本 8/Power/深度 4 | 1/2/4/6 线程的光源和三个相机 XYZ 缓冲一致 |
| `refract_ray` | 固定折射率 1.52 | 双界面方向与地面落点成功输出 |
| JSON 主程序 | 展示配置缩至 96×64，16 SPP、光源每轴 2、4 线程 | 主图、固定折射率对照与三个相机分量均输出成功 |

这些是当前算法的实际执行和数值验收。未执行默认 512×512、256 SPP 的完整主程序渲染，也不以历史快照逐位对照作为验收。


## F2a — 通用路径事件状态重构

基于 main `7dfbe7b`，只整理内部状态和控制流，不开放混合玻璃传播。

- `CameraPathState` 新增私有 `last_scatter: LastScatter`，取值为 `Camera`、`Diffuse`、`Delta`；初始状态为 Camera。两个事件计数器和漫反射预算仍各自独立。
- `trace_camera_path` 先按当前材质的 `is_delta()` 分派。Delta 分支集中检查 Diffuse → Glass 禁止规则和八次玻璃上限；非 delta 分支按是否已发生漫反射选择首次表面/后续 Lambertian 计光。
- `apply_scatter` 共用 RGB 吞吐量和事件计数更新；Ray 在循环末尾统一写回。玻璃采样事件即使为零权重也计一次，漫反射只统计继续追踪的非零连接；原有零吞吐量终止时机保留。
- `last_scatter` 表示最近事件，不代替路径历史。miss 是否加入背景、地面是否可读取焦散，仍依据 `diffuse_bounces`，为未来中间插入 Delta 后保留正确的历史判断。
- 保留现有测试，补充 Camera/Delta/Diffuse 状态与 0/1/2/4 预算，以及玻璃八事件与漫反射预算独立性的测试。测试共用现有状态 helper，没有增加冻结实现或生产测试适配入口。

F2b 再处理 Camera → Diffuse → Glass → Diffuse 的传播和计光；当前遇到 Diffuse → Glass 仍在采样前终止，后续 miss 不添加逃逸 HDRI，后续 Lambertian 不查焦散图。Fresnel、MIS、RR、独立 RNG 流、公开 API 和 JSON 均不扩展。

F2a 验收：原有 314 项测试全部保留，新增 2 项事件状态测试；279 项单元测试 + 37 项集成测试通过，无 ignored。`cargo fmt --check`、`cargo build --locked --all-targets`、`cargo test --locked`、`cargo clippy --locked --all-targets -- -D warnings` 及 `git diff --check` 通过。

临时公共 API 探针比较重构前后 1,746 份 XYZ 缓冲，覆盖无环境/渐变/HDRI、Balance/Power、环境样本 1/3/8、漫反射预算 0/1/2/4、seed 42/123，以及 1/2/4 线程，结果一致；探针未纳入仓库，没有保留冻结算法或 hash 测试。受控 RNG 测试继续核对玻璃、环境、漫反射和 RR 的消费顺序。

实际运行 `camera_direct_caustics -- 8`（512²）、`diffuse_indirect -- 16`（0/1/4 预算）和 `camera_parallel_benchmark -- 16 64`；三项均成功，后者确认当前光源和相机 XYZ 在 1/2/4/6 线程一致。没有执行 F2b 或默认 256 SPP 的全尺寸渲染。


## F2b 第一阶段 — 漫反射后的玻璃传播

基于 `dee297d` 的 F2a 状态模型，开放 `Camera → Diffuse → Glass → Diffuse` 和多玻璃事件穿插的路径，不改动 BSDF/环境 MIS、RR、JSON 或公开接口。

- `path.rs` 的 delta 分支不再因为最近事件是 `Diffuse` 而终止。每次玻璃反射/折射仍使用现有 `MaterialSample::Glass` 及 `apply_scatter(Delta, ...)`；不消耗漫反射预算，整条路径玻璃上限仍为 8。
- 后续 Lambertian 顶点沿用 `secondary_direct_lighting`，只计太阳与环境 NEE 到 Direct；首次地面仍是唯一可使用光谱焦散图的顶点。漫反射深度达到上限后可以经过玻璃到终点，但不能再追加新的漫反射连接。
- 将旧的“Diffuse → Glass 终止”测试更新为透射事件计数与 RNG 检查；增加双界面 Fresnel 透射后的独立 RGB 数值参考，以及漫反射后第九次玻璃事件终止检查。原有的未绑定、非 Lambertian 和背面终止检查仍保留。

**尚未完成：** 当前 `diffuse_bounces > 0` 的 miss 仍不添加环境辐亮度，包括 Diffuse → Glass → Environment。为了不与局部环境 NEE/MIS 重复计光，本阶段刻意没有直接放开这个条件；下一阶段需为 delta 链后的逃逸环境建立正确估计规则，并验证玻璃遮挡、MIS、分量与采样统计。

PR #1 审查以 `e9542ca` 为起点。原始分支的全目标构建和严格 Clippy 通过，但格式检查有两处差异，单元测试有一项真实回归：`deep_render_repeats_across_threads_mis_and_components_and_preserves_photons` 的焦散不变性断言失败。

根因是漫反射后的玻璃仍使用 camera RNG，消费了后续 SPP 抖动和玻璃前缀所需的随机数。现将这些玻璃事件移到已有 diffuse 续追流；首次漫反射前的玻璃仍使用 camera 流，environment 流不变。保留原有断言并扩展到深度 4；没有删除有效测试或放宽容差。

受控路径测试同步验证新的流归属，且补充双界面的实际 Snell 出射坐标/方向、前缀与后缀合计八个玻璃事件，以及一次镜面反射的单位权重和漫反射后不计逃逸背景的范围限制。双界面透射的 RGB 吞吐量及 Direct 数值参考保持原标准。

本轮实际通过 `cargo fmt --check`、`cargo build --locked --all-targets`、`cargo test --locked`、`cargo clippy --locked --all-targets -- -D warnings` 和 `git diff --check`。282 项单元测试 + 37 项集成测试全部通过，无 ignored。

实际运行 `camera_direct_caustics -- 8`（512×512）、`diffuse_indirect -- 16`（320×240，0/1/4 预算）和 `camera_parallel_benchmark -- 16 64`。PNG 尺寸/格式正确，检查相机合成图和四反弹示例未见明显异常；线程示例的光源与三个相机 XYZ 在 1/2/4/6 线程一致。低采样预览仍有蒙特卡洛噪声，本轮没有执行默认 256 SPP 的全尺寸渲染。

此次修复不开放漫反射后的环境逃逸，不更改环境 MIS、Fresnel、RR、公开 API 或 JSON；透明 shadow ray、环境焦散、嵌套介质及通用无偏路径追踪仍未实现。

## F2b 第二阶段 — 漫反射后经玻璃逃逸环境

基于最新 main `3e6fdad`，清理前工作区干净，测试基线为 282 项单元测试 + 37 项集成测试全部通过。本节取代上面历史阶段中“经玻璃逃逸环境尚未计入”的限制。

`trace_camera_path` 的 miss 保留两类判断：无漫反射历史仍查询相机背景；已有漫反射历史时，只有 `last_scatter == Delta` 且存在 `scene.environment` 才查询最终方向的环境辐亮度，并将 `throughput ⊙ radiance` 加入 Direct。最近散射为 Diffuse 的 miss 仍截断，无环境时不将 `background_color` 当作续追照明。

局部 `environment_lighting` 的 BSDF/HDRI 两种策略都只查询直达环境，shadow ray 遇到玻璃即遮挡，因此无法生成 Diffuse → Delta+ → Environment；新增项与其路径支持不重叠，无需额外 MIS 权重。完整吞吐量已包含 Lambertian BSDF、余弦/PDF、Fresnel 离散事件权重和 RR 存活补偿，不再重复相乘或除概率。环境采样器、Balance/Power 配额与 PDF、状态结构、公开 API、JSON 及 RNG 流均不修改。

新增 `path_tests/environment_escape.rs` 的 10 项测试：单界面透射和最终渐变方向/强度、反射单位权重、双界面折射率平方抵消、无环境时背景不参与续追、纯漫反射及玻璃前缀后的漫反射逃逸不重复计光、玻璃后再次漫反射的 NEE/逃逸分工、完整 RGB 与 RR 补偿、旋转 HDRI 和 1/3/8 样本的 Balance/Power 分工、八次玻璃上限、非法环境/贡献溢出。原有 319 项测试全部保留，包括首次地面焦散、分量相加、独立 RNG、深度/RR 和线程确定性；没有删除文件、函数或测试，也没有引入冻结算法或重复生产实现。

最终实际通过 `cargo fmt`、`cargo fmt --check`、`cargo build --locked --all-targets`、`cargo test --locked`、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check`。现为 292 项单元测试 + 37 项集成测试，无失败或 ignored。

修改前后实跑与对比：

- `camera_direct_caustics -- 8`：512×512 三张 RGB8 PNG；Caustics 文件完全一致，Direct/Combined 随新增环境项变化，已检查合成预览。
- `diffuse_indirect -- 16`：320×240、0/1/4 漫反射预算；三张 PNG 前后完全一致，已检查四反弹预览。
- 临时公共 API 探针：48×32、16 SPP、seed 42，覆盖无环境/渐变/HDRI、0/1/4 预算、Balance/Power、环境样本 3。90 份原始 XYZ 缓冲中 74 份逐位一致；16 份变化均为有环境且预算大于零的 Direct/Combined。全部光源和相机 Caustics 缓冲逐位一致，无环境及深度 0 的全部缓冲逐位一致。探针和快照未纳入版本控制。
- `camera_parallel_benchmark -- 16 64`：光源和三份相机 XYZ 在 1/2/4/6 线程逐位一致；`refract_ray` 成功输出双界面方向和地面落点。

仍是有界估计器，未实现透明 shadow ray、太阳穿玻璃的直接照明、环境焦散图、嵌套介质或新材质。狭窄玻璃路径可能有较高方差；深度和八次玻璃上限仍会截断路径。本轮未执行默认 256 SPP 的全尺寸主程序渲染。

## 通用 BSDF 第一阶段

从 main `687d76f` 审计并迁移所有材质采样调用方，删除材质专用采样 enum/玻璃采样记录及 `is_lambertian()`/`is_delta()` 材质判断。统一采样返回方向、RGB 权重、可选立体角 PDF 和具体散射事件。Dielectric 的 Fresnel 离散选择抵消与 radiance eta² 只在材质内部处理一次。

路径以总深度、连续连接数、最近采样事件和已着色连续顶点历史推进；Delta 数量由总深度减连续深度得到。保留 `max_diffuse_bounces` 配置名，将其语义推广到连续 BSDF 连接。直接光不再按 Lambertian 类型筛选，辐照度图通过显式反射率能力绑定。新增 Mirror、JSON 校验、RGB 镜面后逃逸与光谱焦散隔离测试；已有 Fresnel、RR、MIS/光谱/颜色/分量与线程验证迁移到新接口。

局部环境 NEE 保持二值遮挡；非 Delta miss 不再计光，Delta 链逃逸以完整 RGB throughput 进入 Direct。混合材质的局部 BSDF 样本若为 Delta，保留空配额，不能补采。光源端双界面算法只用于理想介质，不把 Mirror 的反射当作玻璃折射焦散。

第一阶段验证：295 项单元测试 + 39 项集成测试（原 37 项保留，新增 2 项）通过；fmt、全目标 build、严格 Clippy 与 diff 检查通过。工作区原有 Delta 上限 32，旧八次上限测试在初始基线中有 4 项失败；现改为按上限测试边界，工作区 32 与提交版 8 均完整通过。HDRI 更换及上限修改保留为用户未提交改动。

实跑 release `camera_direct_caustics -- 4`（512²）、`diffuse_indirect -- 4`（320×240，深度 0/1/4）、`camera_parallel_benchmark -- 4 32` 成功；后者在 1/2/4/6 线程的光源和三个相机 XYZ 缓冲逐位一致。未运行默认 256 SPP 全尺寸主渲染。

## 通用 BSDF 第二阶段 — GGX Metal

新增独立的 `material/ggx.rs`（D/G、精确导体 Fresnel、半向量采样和反射 Jacobian/PDF）与 `ggx_metal.rs`。仅支持 RGB 各向同性单次散射金属，`alpha=roughness²`；roughness≤0.001 采用理想镜面。完整 NDF 采样的无效反射保留为零样本，不能重采样或除以有效接受率。JSON enum 新增 eta/k/roughness 参数及校验。Scene 的 trait object 存储、RGB 相机和三分量入口保留。

补充半球 NDF 归一化、逆 CDF、D/G/F 独立数值参考、Fresnel 退化/极值、sample/pdf/evaluate 一致性、互易性、PDF 质量与接受率、吞吐量均值和半球积分、粗糙度零极限与近切线数值测试。实际路径包含 GGX → Mirror → Dielectric 双界面 → Lambertian、重复 GGX 与 RR；测试混合 Diffuse/Delta 的局部 NEE 空配额和总体均值，非 Delta 逃逸不会重复计光。

Mirror、粗糙/光滑 GGX 的 JSON 场景在 Balance/Power、深度 0/1/4、seed 42/123、Rayon 1/2/4（含重复 1）线程下逐位一致，三个输出可加。旧玻璃场景生成的非零焦散图不会被套到 Mirror/GGX 棱镜；新反射材质场景的焦散图为零。光谱图地面仍显式要求中性 Lambertian，灰 GGX 也会被拒绝；独立 RGB 相机可渲染 GGX 地面。

### 本次最终验收与改动数量

- 审查起点为远端和本地一致的 main `687d76f`，先阅读 AGENTS/路线图并检索材质采样和路径的全仓库调用方。初始工作区测试为 288 通过、4 失败：已有上限 32 的本地改动与旧八次上限断言冲突；此时集成测试未继续执行。没有覆盖或提交用户的 HDRI/上限修改。
- 删除源码/示例文件 0 个；删除 4 种旧函数/API 入口及其对应定义：`MaterialSample::wi` 1 处、`Material::is_lambertian` 及实现 7 处、`Material::is_delta` 及实现 5 处、`sample_diffuse_ray` 1 处，合计移除/替换 14 处旧定义（包含测试材质实现）。新的 `ScatterType::is_delta` 判断事件，语义不同。另移除 3 个旧采样类型 MaterialSample/GlassSample/GlassEvent，不提供兼容适配层。
- 完整测试删除 0 项，5 项重命名/按当前事件语义改写；新增 25 项。329 → 354 项：311 单元 + 43 集成（19 配置/渲染、18 环境、6 材质传输），无 ignored。旧离散 probability 字段校验改为当前 weight/PDF/event 契约检查，Fresnel 频率、两界面 eta²、光谱能量、RR 补偿及现有 MIS/线程测试保留。
- 实际消除的重复是路径中的玻璃专用吞吐量更新与连续材质的再次 evaluate/pdf 权重重算；所有续追统一使用 sample.weight 和一次事件更新。`path.rs` 仅做通用事件推进，地面辐照度转换移至直接着色模块。保留独立 D/G/F 数值参考、概率质量/接受率和能量积分测试，RGB/XYZ 两种输出以及光源 flux/相机 radiance 两种权重，不为了行数而混合它们。
- `cargo fmt` 已执行。`cargo fmt --check`、`cargo build --locked --all-targets`、`cargo test --locked`、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check` 全部通过。最终代码分别在工作区 32 次 Delta + 用户 HDRI，以及提交配置 8 次 Delta + 仓库 HDRI 下完整通过这五项检查；临时验证后恢复用户文件。
- 第二阶段 release 再次实跑 `camera_direct_caustics -- 4`（512²）、`diffuse_indirect -- 4`（320×240、0/1/4 深度）、`camera_parallel_benchmark -- 4 32`、`refract_ray`。基准 1/2/4/6 线程光源/三个相机 XYZ 逐位一致。另使用临时 JSON 实跑 Mirror、roughness=0.45 GGX、roughness=0 GGX 的 64×48、16 SPP、4 线程主程序；RGB8 输出解码确认 Direct 非零、Caustics 全黑、Combined=Direct。

仍未实现 RoughDielectric、Plastic、各向异性、微表面多次散射能量补偿、光谱金属 BSDF、金属反射焦散、透明 shadow ray、嵌套介质与通用光子映射。解析太阳不通过 Delta 方向直接求值；只采样支持的表面/环境路径。有限深度截断、NDF 无效样本与玻璃二值遮挡的边界保留，当前系统不能宣称通用无偏 Path Tracer。没有运行默认 512²/256 SPP 全尺寸主渲染。

## 默认展示配置与 Delta 上限更新

用户随后要求将此前保留的改动一并提交：展示 JSON 改用随仓库提供的 `assets/HDRI/herkulessaulen_4k.hdr`，相应配置测试与 README 同步；`MAX_DELTA_EVENTS` 从 8 提高到 32，现有预算边界测试按此常量验证。上文关于保留未提交修改及提交版上限 8 的内容是两个 BSDF 重构提交时的验收记录。

提交前再次通过 fmt、全目标 build、354 项测试（311 单元 + 43 集成）、严格 Clippy 和 diff 检查。

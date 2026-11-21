# 渲染器重构记录（2026-11-06）

以开始实施时的工作区快照为基线，包含当时已有的 59 个修改文件。已有配置的数值、相机和材质参数已保留；配置只移除旧开关并补齐光源 type。没有新增依赖。

## 已完成的职责调整

1. Scene 只持有物理场景和求交；RenderJob 组合 Camera、采样、接收区和绑定编号。GeometryHit 与绑定后的 Hit 分开，材质编号不再是 Option。
2. Renderer 只有 render 一个高层入口，持有设置与线程池；BSDF、路径和光谱积分都不在 Renderer 中。
3. 光子统一 Forward。XYZ / Eλ 共用发射首交点、介质事件与标量沉积；Constant 复用几何。普通相机不分配光子 Direct 图。
4. RGB / Hero 共用 CameraPathState、路径循环、环境/圆盘 NEE、MIS、预算和 RR。仅一个 CameraTransport 数值/传输抽象；没有加入 Scene trait、Integrator trait、动态 Observer 或共享锁。
5. Fresnel / Snell / 界面事件共用；Radiance 的 eta² 与光子 Flux 的系数/PDF 分开。BSDF 改为明确的 RGB / scalar 返回值，删除运行时数值变体和对应的不可能状态。
6. Film、颜色转换和显示输出分离。Diagnostics 通过 feature 编译，RGB / Spectral 均观察真实路径；普通构建没有诊断字段、缓冲和事件记录调用。

## 删除和合并统计

这里按 `.rs` 路径和函数名多重集统计；重命名计为旧定义消失/新定义出现，同名搬迁不计为删函数。因此这些数字不是“删除了多少物理功能”。

| 项目 | 开始快照 | 当前 | 说明 |
| --- | ---: | ---: | --- |
| Rust 文件 | 97 | 102 | 45 个旧路径移除、50 个新路径，包含搬迁/拆分 |
| 函数定义 | 1062 | 1059 | 125 个旧名定义删除/替换，122 个新名定义 |
| 测试定义 | 477 | 451 | 45 个旧定义删除/替换，19 个新定义，净减少 26 |

### 有意删除

- 删除 `two_transmission` 算法及其选择枚举 `PhotonTraceMode`、配置、调用入口和适配测试。
- 删除旧 `Render` 的 `render_mode`、`render_lighting`、`save` 以及重叠的 Renderer / Camera 单图和分量包装。光子追踪职责从 Scene 移至 Photon Integrator。`take_photon_landing`、`take_photon_from_hit`、`trace_caustics_photon` 随旧入口移除，`trace_forward_photon` 迁为 Photon Integrator 的 `trace_photon`。
- 删除重复光谱路径状态、采样类型和动态数值变体 `SpectralPathState`、`SpectralBsdfSample`、`BsdfValue` 和 `EvaluateMode`。
- 移除 Ray 中未使用的保留距离及逆方向缓存。绑定后的运行时 Hit 使用必填物体和材质编号。
- 移除由 Debug / Release profile 隐式决定 AOV 的规则、旧 JSON 字段和未标记类型的光源格式。

测试清理针对旧双透射/模式选择、兼容入口、可缺失材质、已删除 Ray 字段和无调用方的 Framebuffer 累积器。Cauchy、解析 Snell 落点、Forward 玻璃板 Fresnel 级数、TIR、棱边续追、RGB/Scalar RR 补偿、BSDF/PDF、MIS 数值积分、CIE/D65、发射通量、光谱重积分、CRN、分量相加与线程确定性保留或迁到活跃实现。没有把旧积分器放进测试目录继续维护。保留单元和集成两个层次的部分覆盖，分别验证局部数学与真实 Pipeline，不为减少数量而删除它们。

## 真实消除的重复

- 两套相机路径状态和主循环 → 一套；RGB/标量的 RR、后续玻璃、首次焦散归属和环境逃逸不再分开维护。
- RGB/光谱环境 NEE 与 MIS、圆盘 NEE → 共用估计器；旧首/后顶点着色函数删除。
- 两套 XYZ / Eλ 发射与 Forward 算法 → 同一标量几何内核；首交点只查一次，Direct/逃逸/遮挡不在每个波长重新求交。
- 相机/光子 Fresnel 事件选择 → optics/interface；用不同权重方法保留两种传输物理。
- RGB 与 Photon 的源白点 → Light::white_xyz；没有焦散目标时仍保留同一 D65 标尺。
- BSDF 采样记录 → BsdfSample<V>，删除重复的标量采样类型和一对一字段转抄包装。

## 检查留在哪里

- JSON 结构解析、实际对象构造、Renderer 设置、光子计划/目标绑定各自承担入口验证。主流程不先构建一套校验几何再重复构建。
- Framebuffer 布局由构造函数保证，外部只有切片；删除依赖外部可破坏布局的重复检查。图之间尺寸不匹配和实际颜色溢出仍处理。
- 环境参数通过构造/显式修改验证，查询不再重复 is_valid。圆盘面积/辐亮度缓存、归一化法线只在构造时准备；采样不重复验证静态几何。
- 太阳在像素循环外准备。光子目标/IOR/上限在 PhotonTracer 创建时验证；相机初态在 trace 入口验证，不再每次反弹全量验证。
- PDF=0、样本拒绝、全反射、无交点、零吞吐、实际退化方向/距离/溢出、深度和 RR 仍有明确处理。公开自定义 Material 的事件/PDF/权重契约仍检查。
- 只保留可恢复错误/合法缺失使用 Option/Result；没有用 Release 的 unwrap 或 debug_assert 代替真实数值失败处理。

## API 与数值语义

- Forward 统一了光子焦散计算，多次反射、透射和全反射均可计入结果。原双透射算法的 PNG 只作历史对照。
- 原无焦散图 RGB 入口的太阳法向辐照度从 `Φ/A` 修正为 `Φ/(A·|d_z|)`，使水平接收面的辐照度与光子发射归一化一致。统一入口保留源 D65 白点。
- Sphere、Prism 和 VerticalPrism 在 JSON 中统一以 `caustics=false` 为缺省值，显式选择非 Dielectric 作为焦散目标会报错。光源需要 `type` 字段，旧调试字段和旧算法字段会被拒绝。
- 配置解析与实际构建验证分离。CLI 统一保存三个分量，AOV 通过 `diagnostics` feature 显式开启，Debug 和 Release 均支持。旧入口随迁移移除。
- 标量几何层先合并，再乘光谱权重与 CIE 匹配函数。该顺序和圆盘几何缓存可能影响末位舍入。固定设置、seed 和采样块时，线程间结果仍逐位一致。改变采样块分组的对照使用容差。
- 没有扩大物理范围：二值玻璃阴影、单理想介质、首次连续地面焦散、环境逃逸限制及有限预算不变。

## 旧文件迁移清单

| 旧文件 | 当前归属 / 清理 |
| --- | --- |
| `src/config/cameraconfig.rs` | src/config/camera.rs |
| `src/config/lightconfig.rs` | src/config/light.rs |
| `src/config/materialconfig.rs` | src/config/material.rs |
| `src/config/rendersetting.rs` | src/config/render.rs |
| `src/config/sceneconfig.rs` | src/config/scene.rs |
| `src/optics/emission.rs` | src/spectrum/emission.rs |
| `src/optics/iormode.rs` | 合入 src/optics/dispersion.rs |
| `src/optics/spectrum.rs` | src/spectrum/mod.rs |
| `src/render/caustics.rs` | src/integrator/photon/generation.rs + src/caustics/mod.rs |
| `src/render/caustics/wavelength.rs` | 共用 photon/generation.rs；只读 Eλ 数据迁入 caustics/map.rs |
| `src/render/framebuffer.rs` | src/film.rs |
| `src/render/integrator.rs` | src/integrator/camera/mod.rs |
| `src/render/integrator/area.rs` | src/integrator/camera/lighting/area.rs |
| `src/render/integrator/bounce_tests.rs` | src/integrator/camera/tests/bounce.rs |
| `src/render/integrator/camera.rs` | src/integrator/camera/rgb.rs；删除单图/分量包装层 |
| `src/render/integrator/debug.rs` | src/diagnostics/mod.rs（feature） |
| `src/render/integrator/debug/tests.rs` | src/diagnostics/tests.rs（feature） |
| `src/render/integrator/diffuse.rs` | camera/path.rs；删除重复后续顶点着色/续追 |
| `src/render/integrator/diffuse_tests.rs` | src/integrator/camera/tests/diffuse.rs |
| `src/render/integrator/direct.rs` | camera/lighting/mod.rs + 共享 camera/path.rs；删除旧首顶点着色 |
| `src/render/integrator/environment.rs` | src/integrator/camera/lighting/environment.rs |
| `src/render/integrator/environment_tests.rs` | src/integrator/camera/tests/environment.rs |
| `src/render/integrator/environment_tests/power.rs` | src/integrator/camera/tests/environment.rs（power 子模块） |
| `src/render/integrator/environment_tests/stratified.rs` | src/integrator/camera/tests/environment.rs（stratified 子模块） |
| `src/render/integrator/ground.rs` | src/integrator/camera/ground.rs；只保留图查询/绑定 |
| `src/render/integrator/path.rs` | src/integrator/camera/path.rs |
| `src/render/integrator/path_tests.rs` | src/integrator/camera/tests/path.rs |
| `src/render/integrator/path_tests/environment_escape.rs` | src/integrator/camera/tests/path.rs（environment_escape 子模块） |
| `src/render/integrator/path_tests/materials.rs` | src/integrator/camera/tests/path.rs（materials 子模块） |
| `src/render/integrator/rng.rs` | src/sampling/rng.rs |
| `src/render/integrator/spectral.rs` | src/integrator/camera/spectral.rs |
| `src/render/integrator/spectral/diagnostic.rs` | src/diagnostics/wavelength.rs（feature） |
| `src/render/integrator/spectral/lighting.rs` | 合入共享 camera/lighting/，删除独立估计器 |
| `src/render/integrator/spectral/path.rs` | 合入 src/integrator/camera/path.rs，删除独立循环与 RR |
| `src/render/integrator/spectral/state.rs` | 合入 src/integrator/camera/types.rs |
| `src/render/integrator/test_support.rs` | src/integrator/camera/tests/support.rs |
| `src/render/integrator/tests.rs` | src/integrator/camera/tests/mod.rs |
| `src/render/integrator/types.rs` | src/integrator/camera/types.rs |
| `src/render/mod.rs` | src/renderer.rs；输出编码迁入 src/output.rs |
| `src/render/utils.rs` | src/color.rs + src/output.rs；空间插值迁入 src/caustics/map.rs |
| `src/scene.rs` | src/scene/mod.rs；Camera/接收区/积分器选项移入 RenderJob |
| `src/scene/builder.rs` | src/config/build.rs |
| `src/scene/checkerboard.rs` | src/material/checkerboard.rs |
| `src/scene/photon.rs` | src/integrator/photon/mod.rs + config/job.rs；删除双透射算法 |
| `src/scene/photon/path.rs` | src/integrator/photon/path.rs |

## 验收记录

- `cargo fmt`、`cargo fmt --check`：通过。
- `cargo build --locked --all-targets`：通过；`cargo build --release --locked --all-targets` 也通过。
- `cargo test --locked`：420 项通过（318 单元、102 集成）。
- `cargo test --locked --all-targets --features diagnostics`：450 项通过（329 单元、118 集成、3 示例）。源文件中 451 个测试定义包含两个互斥 feature 测试，单次构建不会同时运行。
- 默认 / diagnostics 两种 `cargo clippy --locked --all-targets ... -- -D warnings`：通过。
- Release diagnostics 的路径 AOV、CLI 和光谱观察器 4 项额外测试：通过。
- `git diff --check`：通过。
- 13 次实际 Release 渲染：64×48、16 SPP、光子每轴分层 2，覆盖 RGB+HDRI、RGB 三棱镜、Hero Constant / Dispersion、Packet=1/4、平行光/圆盘。三组场景在 1/2/4 线程的三个 PNG 分量逐字节一致；已目视检查 Combined / Caustics。
- 实际运行 camera_parallel_benchmark（32²、8 SPP、1/2/4/6 线程），三个 XYZ 分量逐位一致；spectral_camera_compare（48²、16 SPP、Constant/Dispersion）和 sphere_wavelengths（32²、8 SPP、450/550/650 nm）均成功。

渲染与检查日志位于实施时的 `/private/tmp/spectral-refactor-i_zppv0q/`；没有覆盖仓库现有输出或提交新的大图资源。渲染验证使用缩小配置，不是所有展示配置的原尺寸/原 SPP 重渲染，也不是长期性能基准。测试期间曾并发运行两种 feature 构建，导致共用 CLI 路径被覆盖；最终验收已串行重跑，通过结果均来自这些重跑。

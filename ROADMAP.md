# Spectral Caustics Renderer — 当前状态与边界

本路线图描述当前实现，不作为自动推进后续算法的指令。下一项里程碑由用户指定；历史 Day/E/F 阶段实现细节与旧验收数字已由 Git 历史保存。

## 已实现

- [x] 米制 Z-up 几何、单三棱镜/有限地面配置、材质绑定与最近交点。
- [x] 双界面 Snell 折射、Cauchy 色散、Fresnel 透射通量与地面双线性 XYZ 累积。
- [x] 可追溯 CIE/D65 梯形积分、线性颜色与固定曝光 sRGB PNG。
- [x] 理想玻璃 Fresnel 相机路径，最多 8 个玻璃事件。
- [x] HDRI/渐变环境、重要性采样与固定配额 Balance/Power MIS。
- [x] 0..1024 次额外 Lambertian 连接、RGB 吞吐量与补偿 RR。
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

Direct 仍包含解析太阳、环境和有界漫反射贡献；Caustics 仍只来自首次地面顶点，不能在续追中读取焦散图或添加逃逸 HDRI。RR、Fresnel、MIS/PDF 和光谱能量的物理含义不变。

仍保留的有用冗余：独立 RGB 单缓冲相机与光谱三图相机共用一份路径/像素循环，但有两种缓冲写入分支，避免 RGB 场景分配三张图；光源保留 sampled direct 用于守恒验证。单光子及独立逐波长发射测试参考用于数值验证，不是冻结生产算法。Render 门面与自由函数分别提供配置/线程池集成和直接调用。

不增加漫反射后的玻璃续追、透明 shadow ray、新材质、环境焦散、GUI/GPU、BVH/KD-Tree 或通用 Photon Mapping。当前物理简化及运行方法见 [README.md](README.md)。

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

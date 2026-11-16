# 验证范围与证据

按任务和实际改动选择验证，执行结果不得由历史报告代替。物理契约见 [transport.md](transport.md)，现有命令及参数说明复用 [README 的示例与验证](../README.md#示例与验证)。

## 选择检查

- 纯文档改动：核对本次涉及的实现声明、相对链接/锚点、术语、文档职责及根 AGENTS 的加载路由，运行 `git diff --check`；无需运行 Cargo、创建渲染输出或重做数值基线。
- Rust 改动：迭代时先跑相关数学/行为回归，交付时执行下列验收命令。重构或清理先建立当前基线，再迁移调用方和测试；不要冻结旧积分器作为验收标准。
- 改动实际渲染时，除测试外运行相关相机/焦散示例并检查图像；缩小分辨率/SPP、使用独立输出目录、记录配置/seed/模式，保留仓库已有输出。只改局部配置/输出策略时选择对应 Pipeline 检查，不默认重渲染全部展示图。

```sh
cargo fmt
cargo fmt --check
cargo build --locked --all-targets
cargo test --locked
cargo test --locked --all-targets --features diagnostics
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features diagnostics -- -D warnings
git diff --check
```

普通与 diagnostics 构建分别验证；仅跑启用全部 feature 的构建会遗漏 `cfg(not(feature="diagnostics"))` 分支。CLI 集成测试会执行构建产物，两种 feature/profile 的 Cargo 验收串行运行，避免共享可执行文件互相覆盖。Diagnostics/profile 相关改动还需针对受影响的 CLI/AOV 测试运行 Release + diagnostics；不要将 Debug 测试结果外推到 Release。

## 改动到回归的映射

表中列出入口，不是每次都要执行的额外全量清单；先阅读受影响测试，再按触及的契约选择。

| 改动 | 应验证的内容与现有入口 |
| --- | --- |
| 共享 Camera Path、throughput、RNG、并行 | [path](../src/integrator/camera/tests/path.rs)、[bounce](../src/integrator/camera/tests/bounce.rs)、[config_render](../tests/config_render.rs)：即时着色、深度 0/1/多次、RR 补偿、Delta/连续预算、首次焦散、分量相加和独立流；覆盖 RGB/Hero/Packet 与 1/2/4 线程 |
| BSDF、环境、NEE/MIS、光源 | 材质本地数学测试、[environment](../src/integrator/camera/tests/environment.rs)、[environment_scene](../tests/environment_scene.rs)、[disk_area_light](../tests/disk_area_light.rs)：求值/采样/PDF 一致，Balance/Power、N=1/偶数/奇数、零/拒绝样本、遮挡、有限阴影段、独立积分与环境逃逸不重计 |
| 折射界面、光子、焦散沉积 | [photon/path](../src/integrator/photon/path.rs) 本地测试、[forward_photons](../tests/forward_photons.rs)、[sphere_caustics](../tests/sphere_caustics.rs)、[vertical_prism](../tests/vertical_prism.rs)：Snell/Fresnel、TIR、eta 权重、棱边续追、事件截断、空间通量、XYZ/Eλ 重积分及确定性 |
| 波长、色散、光谱/颜色数据 | [spectral_camera](../tests/spectral_camera.rs)、[spectral_caustics](../tests/spectral_caustics.rs) 和 spectrum 本地测试：波长层覆盖、400/SPP、Packet/CRN、lane 独立、D65/重建、Constant/Dispersion、光谱 GGX 拒绝；对照独立细波长积分而非强求 RGB 完全一致 |
| 几何或材质绑定 | 相应 geometry 本地测试及 [sphere_transport](../tests/sphere_transport.rs)、[beveled_prism](../tests/beveled_prism.rs)、[material_transport](../tests/material_transport.rs)：命中/法线、退化/极值、有效编号、背面、材质能力及焦散绑定限制 |
| 均匀介质与体照明 | [medium/tests](../src/medium/tests.rs)、[volume](../tests/volume.rs)、[spectral 本地测试](../src/integrator/camera/spectral.rs)：盒段裁剪、Beer–Lambert、自由程概率/权重、HG 归一化/符号/抽样、独立单次散射积分、0/1/多次预算、纯吸收、均匀环境能量、RGB/Hero/Packet CRN、MIS/线程和不改变光子图 |
| 配置、输出、Diagnostics | config/output 本地测试、[config_render](../tests/config_render.rs)、[diagnostics_feature](../tests/diagnostics_feature.rs)、[path_debug](../tests/path_debug.rs)：解析/构造分界、旧字段拒绝、文件名冲突、编码、feature 开关、观察器不改变物理分量/RNG、lane 计数与 Release 行为 |

示例按目的选择：[camera_direct_caustics](../examples/camera_direct_caustics.rs) 看三个分量；[camera_parallel_benchmark](../examples/camera_parallel_benchmark.rs) 看线程结果；[spectral_camera_compare](../examples/spectral_camera_compare.rs) 看 Constant/Dispersion；[spectral_packet_compare](../examples/spectral_packet_compare.rs) 看 Packet；[homogeneous_medium_compare](../examples/homogeneous_medium_compare.rs) 看无介质/吸收/散射。参数先读示例或 README，不把一次缩小渲染当作长期性能基准。

## 如何解释结果

- 固定配置、seed、采样块的当前实现，应在重复执行和 Rayon 1/2/4 线程下逐位一致，逐个检查 Direct/Caustics/Combined；Diagnostics 开关的纯观察路径也应保持物理结果。
- 浮点重分组、源端积分步长变化、Monte Carlo 与独立数值积分的比较使用有理由的容差；不冻结旧版本 RNG 消耗或 PNG。统计比较说明样本数与误差范围。
- 通量守恒测试须区分完整接收与有限窗口、遮挡、逃逸、截断。对丢失通量不能重新归一化；有雾的源端完整能量不在当前焦散模型的保证范围。
- 保留解析 Snell/Fresnel、MIS/PDF、RR、颜色数据、波长重积分、光子通量等核心数学测试，以及真实 Pipeline 的集成覆盖。局部数学与端到端回归虽有重叠，验证目的不同；不为降低测试数量删除它们。
- 新测试优先证明物理不变量或可观察行为，避免复制实现算法、只断言内部结构或为可逆低影响文档改动增加实现镜像测试。

## 当前验证限制

- [config_render](../tests/config_render.rs) 的 `default_executable_renders_all_three_spheres_and_repeats_across_threads` 和 [sphere_transport](../tests/sphere_transport.rs) 的 `debug_target_can_switch_between_sphere_ground_and_none_without_changing_radiance` 仍有 `debug_assertions` 相关断言，与 feature 控制的 Release 语义需要核对。涉及该区域时报告实际结果，不为迁就旧断言恢复 profile 门控。
- [Diagnostics::internal_tir](../src/diagnostics/mod.rs) 用 `material.ior()` 常数做 TIR 标签判定，未接收当前波长 IOR；不应将光谱 TIR 标签当作波长精确的物理真值。观察器不扰动路径的测试不能证明此标签准确。
- 上述为源码可见的验证风险，不是本文件声称已执行并失败的检查；相关修复依具体任务范围处理。已有文档中的过期路径/注释按 [project.md](project.md#信息来源与范围) 判断。

## 交付证据

报告实际修改、接口/渲染语义变化、已执行检查与渲染参数、失败或未执行项及原因。清理任务再报告删除/迁移与实际消除的重复、保留的重要测试；纯文档任务无需编造删除统计或渲染验收结果。

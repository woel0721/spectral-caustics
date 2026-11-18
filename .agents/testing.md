# 验证范围与证据

按任务和实际改动选择验证，执行结果不得由历史报告代替。物理契约见 [transport.md](transport.md)，现有命令及参数说明复用 [技术指南的示例与验证](../docs/rendering_guide.md#示例与验证)。

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
| 网格密度与确定性消光 | [medium/tests](../src/medium/tests.rs) 覆盖密度索引、全局 Majorant、裁剪、边角与反向射线、均匀退化和双层解析解。生产 `VoxelDda` 的最终 Tr 与逐体素独立盒积分对照。纯吸收、全零密度和预算耗尽有精确 Tr 与 RNG 回归。 |
| 局部 Majorant 与随机碰撞 | [grid 内部测试](../src/medium/grid/tests.rs) 核对局部上界、非整除尾块、跨块剩余光学深度、零上界块 RNG 和分层碰撞 CDF。逐段 slab 对照调用 `MajorantDda`，步长为 1 时仍走粗块特化。[medium/tests](../src/medium/tests.rs) 另测生存概率、隐式吸收权重和候选距离无法推进时的错误。 |
| 网格体散射与相机接入 | [grid_scattering](../tests/grid_scattering.rs) 对照单次散射独立积分、多次散射均匀环境能量和独立预算。[grid_medium](../tests/grid_medium.rs) 覆盖自定义介质、RGB/Hero/Packet 的 1/2/4 线程分量比较；[spectral 本地测试](../src/integrator/camera/spectral.rs) 对照均匀及网格介质的 Packet CRN。 |
| 配置、输出、Diagnostics | config/output 本地测试、[config_render](../tests/config_render.rs)、[diagnostics_feature](../tests/diagnostics_feature.rs)、[path_debug](../tests/path_debug.rs)：解析/构造分界、旧字段拒绝、文件名冲突、编码、feature 开关、观察器不改变物理分量/RNG、lane 计数与 Release 行为 |

示例按目的选择：[camera_direct_caustics](../examples/camera_direct_caustics.rs) 看三个分量；[camera_parallel_benchmark](../examples/camera_parallel_benchmark.rs) 看线程结果；[spectral_camera_compare](../examples/spectral_camera_compare.rs) 看 Constant/Dispersion；[spectral_packet_compare](../examples/spectral_packet_compare.rs) 看 Packet；[homogeneous_medium_compare](../examples/homogeneous_medium_compare.rs) 看无介质/吸收/散射；[grid_medium_compare](../examples/grid_medium_compare.rs) 看均匀/分层/团块密度与 0/1/8 体预算。参数先读示例或 README，不把一次缩小渲染当作长期性能基准。局部上界性能使用 [grid_majorant_benchmark](../examples/grid_majorant_benchmark.rs) 和 grid 内部 ignored Release 分块基准；时间不作 CI 断言，须分别报告采样/整图收益、均匀场景开销、缓存内存与构造成本。测量条件见 [基准记录](../docs/medium_majorant_benchmark.md)。

## 如何解释结果

- 固定配置、seed 和采样块时，重复运行和 Rayon 1/2/4 线程的 Direct、Caustics、Combined 应分别逐位一致。[均匀介质](../tests/volume.rs)和[网格介质](../tests/grid_medium.rs)的线程回归使用浮点 `assert_eq!`，它将 `+0.0` 和 `-0.0` 判为相等。要验证包括零符号在内的位模式，可逐分量比较 `f64::to_bits()`。Diagnostics 的纯观察路径也应保持物理结果。
- 浮点重分组、源端积分步长变化、Monte Carlo 与独立数值积分的比较使用有理由的容差；不冻结旧版本 RNG 消耗或 PNG。统计比较说明样本数与误差范围。
- 通量守恒测试须区分完整接收与有限窗口、遮挡、逃逸、截断。对丢失通量不能重新归一化；有雾的源端完整能量不在当前焦散模型的保证范围。
- 保留解析 Snell/Fresnel、MIS/PDF、RR、颜色数据、波长重积分、光子通量等核心数学测试，以及真实 Pipeline 的集成覆盖。局部数学与端到端回归虽有重叠，验证目的不同；不为降低测试数量删除它们。
- 新测试优先证明物理不变量或可观察行为，避免复制实现算法、只断言内部结构或为可逆低影响文档改动增加实现镜像测试。

## 当前验证限制

- [Diagnostics::internal_tir](../src/diagnostics/mod.rs) 用 `material.ior()` 常数做 TIR 标签判定，未接收当前波长 IOR；不应将光谱 TIR 标签当作波长精确的物理真值。观察器不扰动路径的测试不能证明此标签准确。

## 交付证据

报告实际修改、接口/渲染语义变化、已执行检查与渲染参数、失败或未执行项及原因。清理任务再报告删除/迁移与实际消除的重复、保留的重要测试；纯文档任务无需编造删除统计或渲染验收结果。

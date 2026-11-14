# 项目约定与知识维护

本文件负责工程决策与验证边界。模块职责见 [architecture.md](architecture.md)，物理契约见 [transport.md](transport.md)，验收方法见 [testing.md](testing.md)。

## 信息来源与范围

- 首次进入项目或涉及行为变更时，阅读 [README](../README.md) 和 [当前实现与开发边界](../ROADMAP.md)，再核对相关源码。路线图描述当前范围，不授权自动实现后续功能。
- README 保留运行方式、配置、公共 API 和用户可见行为；ROADMAP 保留当前能力及限制。这些参考文档解释如何在该架构内工作，不复制完整使用手册或目录清单。
- [重构记录](../docs/refactor.md) 是带日期的迁移与验收证据，其中的测试数量、日志路径和当时授权不代表当前状态或持续权限。
- [颜色数据说明](../assets/color/README.md) 负责来源、许可证与校验和。该文档仍有旧源码路径及已删除 API 的描述；当前接口以 [spectrum](../src/spectrum/mod.rs)、[emission](../src/spectrum/emission.rs) 和 [Material](../src/material/mod.rs) 为准。
- 文档与实现冲突时先定位差异并报告，不为迎合陈旧文字恢复旧行为，也不借机修复任务外问题。改变接口或物理语义时同步其所属文档及相关链接。

## Rust 与抽象选择

- 使用现有 Rust 2024 工具链约定，遵循附近代码及 rustfmt；依赖以 [Cargo.toml](../Cargo.toml) 为准。数学使用 `nalgebra`，图像使用 `image`，新增依赖须有当前任务的实际需要。
- 光学和几何优先 `f64`，变量或接口明确物理量与单位；说明非显然公式、前提和限制，避免转述语法或逐行复述实现。
- 优先具体类型、局部函数和清晰模块。`Material` trait 支持场景材质，`CameraTransport` 支持 RGB/标量共用路径；沿用它们的实际职责，不默认引入通用 Scene/Integrator trait、动态数值变体、包装层或共享锁。
- 共享几何与相同估计器，保留不同物理权重及采样器的边界；相似字段或函数形状不足以证明可以合并。
- 删除过时实现前全仓库查引用，迁移有效调用方和测试，再删除旧入口。不要默认保留转发包装、冻结积分器或测试内的旧算法；解析解与独立数值参考应保留。

## 错误与验证边界

[README 的错误边界说明](../README.md#输入和错误边界) 对应以下实际入口：

| 边界 | 当前职责 |
| --- | --- |
| [Config::load/from_json](../src/config/mod.rs) | 文件读取、Serde 结构解析；不等同于完整数值或场景验证 |
| [Config::build / RenderJob::from_config](../src/config/build.rs) | 实际构造相机、几何、材质、光源、环境、介质，验证参数及对象绑定；HDRI 缺失返回错误 |
| [Renderer::new](../src/renderer.rs) | 验证 RenderSetting 并创建可选线程池 |
| [光子计划与追踪器](../src/integrator/photon/generation.rs)、[PhotonTracer](../src/integrator/photon/path.rs) | 发射布局、接收区、目标、IOR、事件预算及归一化 |
| [Camera Path 入口](../src/integrator/camera/path.rs) | 验证可由低层调用方提供的路径初态，并检查自定义材质的事件/PDF/权重契约 |

`Config::validate` 用于显式配置检查和保存，包含部分几何试构建；正常渲染不先完整 validate 再重复 build。不要为了“统一验证”增加第二套完整场景构造。

- 私有构造不变量成立时，不在每次查询或每次反弹重复校验整对象；例如 [Framebuffer](../src/film.rs) 的尺寸与 Vec 长度由构造函数绑定。公开可修改的状态、独立调用入口和跨对象绑定仍需各自必要的检查。
- 仅在证明所有调用方已有保障后移除冗余检查。保留非有限值、实际溢出/下溢导致的无效量、退化方向、分配尺寸、零 PDF、TIR、无命中、零吞吐量和预算终止的处理。
- 合法无贡献或拒绝样本可以返回零/`None`；配置错误、违反契约与可恢复数值失败使用现有 `Result` 方式。不要把所有 `Option` 改成错误，也不要用零值或默认场景掩盖应报告的失败。
- 维持现有简单错误类型，不为统一外观增加错误框架。`expect`/断言只适合可证明的内部不变量或内置数据；不能用 panic 或 `debug_assert!` 替代 Release 中可能出现的正常输入/数值错误处理。

## 文档维护

每条规则只在一个参考文档展开，其他位置链接它。根 AGENTS 只保留通用规则和加载路由；API 细节跟随源码，运行说明跟随 README，实验结果留在带上下文的记录中。只添加会改变工程决策的知识，不积累通用 Rust 教程、历史阶段口号或一次性任务清单。

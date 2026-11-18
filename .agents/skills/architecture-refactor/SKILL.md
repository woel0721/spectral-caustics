---
name: architecture-refactor
description: Use when refactoring module ownership, APIs, shared abstractions or camera/photon responsibilities, removing obsolete implementations, or performing broad code cleanup. Prefer behavior-specific change skills for algorithm fixes and rendering-review for reviews.
---

# 架构重构与实现清理

读取 [project.md](../../project.md) 的抽象/删除/验证约定和 [architecture.md](../../architecture.md) 的职责边界；涉及传输代码时补读 [transport.md](../../transport.md)，基线和回归按 [testing.md](../../testing.md) 选择。

实施与讨论的边界遵循 [AGENTS.md 的通用规则](../../../AGENTS.md#始终遵守)。

## 工作流程

1. 查看当前工作区和 diff，记录本次范围内的行为与验证基线；区分原有失败和新变化，不覆盖未提交工作，也不把历史验收当作现状。
2. 全仓库查找调用方、导出、配置入口、示例和测试，梳理实际依赖与职责；确定重复的是算法、状态还是仅外观相似的不同物理量。
3. 定义最小目标职责及迁移顺序，说明抽象解决的实际问题；保留清楚的具体类型，不为减少行数或未来可能性加一层通用接口。
4. 按迁移顺序更新调用方和测试。删除旧入口后复查导出、示例与文档链接，确认都指向当前实现。
5. 对照契约核对物理权重、计光与验证归属、随机流和归约顺序；把允许的浮点分组变化与意外渲染语义变化分开记录，保留有效数值安全检查。
6. 用聚焦数学/行为测试和真实 Pipeline 集成回归验证职责迁移，执行适用验收及示例；最终复核 API/文档一致性，报告实际消除的重复、删除/迁移内容、数值变化和检查结果。

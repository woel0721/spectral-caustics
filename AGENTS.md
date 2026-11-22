# Codex Instructions — Spectral Caustics Renderer

这是 Rust CPU 离线渲染器。以本次任务范围和验收为准，维护当前实现及其物理边界，不自动推进后续功能。

## 始终遵守

- 开始前查看工作区状态及相关源码，保留用户未提交的修改。
- 所有 PR 都从 `dev` 向 `main` 提交。每次合并后，将最新的 `main` 同步到 `dev`，下一个 PR 继续复用 `dev`。
- 选择可运行、可测试、可解释的最小实现；只消除实际重复，不为减少行数或假想扩展增加间接层。
- 对明确、低风险、范围有限的修改，阅读相关代码后直接实现。只有存在重要架构选择、需求歧义或较大破坏性影响时，才先讨论方案。不要强制要求每个任务都生成计划或反复确认。
- 保持物理量、概率测度和计光归属明确。精简防御式代码不能删除有意义的数值检查与物理边界处理。
- 介质内部算法必须保持封装，不能让 Camera Integrator 随之复杂化；只通过关键接口使用介质，具体职责见 [architecture.md](.agents/architecture.md)。
- 保持固定设置和 seed 的可复现性、Rayon 线程间确定性，以及线性空间的分量相加关系。
- 不把历史任务授权、旧 API 或旧图像当作永久约束；不顺带恢复兼容层或扩大渲染能力。
- 按改动范围验证，报告实际行为变化、检查结果及限制；未执行的检查不得写成通过。

## 必需阅读与任务路由

| 文档 | 何时读取 |
| --- | --- |
| [project.md](.agents/project.md) | 所有仓库任务：工程约定、错误边界、文档职责 |
| [architecture.md](.agents/architecture.md) | 修改或审查 Rust 源码、API、模块职责，或解释执行流程 |
| [transport.md](.agents/transport.md) | 涉及相机、光子、BSDF、采样、光谱、光照、介质、数值计算的修改或审查 |
| [testing.md](.agents/testing.md) | 修改任何文件、编写测试或进行验收；纯文档任务使用其中的文档检查 |

这些参考文档按条件显式读取，不假设会自动注入。跨领域任务合并所需阅读；进入新区域时补读对应文档和实际实现。

按主要目标选择一个仓库 Skill。普通渲染改动用 [rendering-change](.agents/skills/rendering-change/SKILL.md)，光谱算法改动用 [spectral-change](.agents/skills/spectral-change/SKILL.md)，结构重构用 [architecture-refactor](.agents/skills/architecture-refactor/SKILL.md)。只读审查渲染实现或核实文档中的渲染行为声明时用 [rendering-review](.agents/skills/rendering-review/SKILL.md)。只检查文档结构、链接、Skill 元数据或 CI 命令时，按 [testing.md 的文档检查](.agents/testing.md#选择检查) 核对相应文件。

项目 [Hooks](.codex/hooks.json) 仅提供建议：`PostToolUse` 提示定向验证，`Stop` 检查 HEAD 到工作区的已跟踪净差异的空白问题（含暂存、不含未跟踪文件）；GitHub Actions CI 仍是强制验收层。

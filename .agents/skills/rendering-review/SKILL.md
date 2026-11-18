---
name: rendering-review
description: Review rendering physics, algorithms, architecture, numerical stability, code quality, or documentation claims about renderer behavior. Read-only review.
---

# 渲染实现审查

只做审查与报告，不修改代码、不自动修复或顺手格式化。审查请求即使涉及光谱或架构，也以本 Skill 为主。验证遵循任务允许的范围；严格只读审计不执行会生成文件的构建、测试或渲染。

读取 [project.md](../../project.md)。核对 Rust 实现、API 或职责时读 [architecture.md](../../architecture.md)，涉及物理或数值声明时查 [transport.md](../../transport.md) 对应章节，判断测试覆盖或 CI 声明时查 [testing.md](../../testing.md)。

## 工作流程

1. 明确用户指定的 diff、模块或问题范围，检查当前工作区；分清现有问题与本次改动引入的问题，避免扩展成无关的全仓库重写建议。
2. 按待核实的声明选择证据。CI 命令对照工作流及对应测试入口，API 职责查看定义与调用方。审查渲染行为时，沿受影响的路径从输入或采样点追到结果使用处。
3. 审查估计器时核对被估计量、概率测度、权重补偿及贡献归属，必要时独立推导或构造小型解析反例。
4. 涉及数值行为时检查物理边界、退化输入、数值失败及适用的构建配置。涉及抽象调整时核对它消除的实际重复及调用方依赖的行为。
5. 评价测试覆盖时，阅读相关断言和参考来源，核对受影响的 Pipeline、确定性及已知验证限制。报告中区分静态核对与实际运行结果。
6. 按影响优先级报告可操作发现，给出触发条件、后果、文件与当前行号和证据；将确认的缺陷与尚需复现的风险分开，后者说明缺少的证据及验证办法。没有确认问题时如实说明，并列出实际验证范围和剩余不确定性。

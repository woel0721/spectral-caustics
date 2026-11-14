---
name: rendering-review
description: Use when asked to review or audit rendering physics, algorithms, architecture, numerical stability, or code quality. This is review-only; do not modify code or automatically fix findings.
---

# 渲染实现审查

只做审查与报告，不修改代码、不自动修复或顺手格式化。审查请求即使涉及光谱或架构，也以本 Skill 为主。验证遵循任务允许的范围；严格只读审计不执行会生成文件的构建、测试或渲染。

先读 [project.md](../../project.md) 和 [architecture.md](../../architecture.md)。涉及物理/数值假设时读 [transport.md](../../transport.md) 对应契约；检查测试、选择验证或提出验证建议时读 [testing.md](../../testing.md)。

## 工作流程

1. 明确用户指定的 diff、模块或问题范围，检查当前工作区；分清现有问题与本次改动引入的问题，避免扩展成无关的全仓库重写建议。
2. 从实际输入沿执行路径追踪求交、采样、续追与输出，确认模块职责和数据使用位置；不能仅凭 README、函数名或历史记录判断行为。
3. 对照契约验证被估计量、概率测度、权重补偿及贡献归属；必要时独立推导或构造小型解析反例，检查重复计光、漏权重与错误耦合。
4. 检查物理边界、退化/极值、数值失败及 Release 行为，并区分必要保护与重复验证；评价抽象是否消除实际重复，不以代码长度作为正确性依据。
5. 阅读相关测试的断言和参考来源，判断它们是否独立验证结论、覆盖受影响的 Pipeline 与确定性；核实已知验证限制，实际未执行的检查不能作为证据。
6. 按影响优先级报告可操作发现，给出触发条件、后果、文件与当前行号和证据；将确认的缺陷与尚需复现的风险分开，后者说明缺少的证据及验证办法。没有确认问题时如实说明，并列出实际验证范围和剩余不确定性。

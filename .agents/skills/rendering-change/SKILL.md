---
name: rendering-change
description: Use when implementing or fixing camera/photon transport, BSDF sampling, NEE/MIS, continuation/RR, lighting/visibility, geometry affecting rendering, or existing medium transport. Prefer spectral-change for spectral algorithms, architecture-refactor for structural work, and rendering-review for reviews.
---

# 渲染行为修改

读取 [project.md](../../project.md) 的工程与验证边界、[architecture.md](../../architecture.md) 的职责，再按本次改动读取 [transport.md](../../transport.md) 的相关契约和 [testing.md](../../testing.md) 的验证映射。物理定义与验收命令由这些文档维护。

## 工作流程

1. 从用户描述的输入或渲染现象沿实际调用链定位实现与所属模块，确认会影响相机、光子还是两者，避免在调度层修补散射算法。
2. 明确估计的物理量、积分域和采样策略；沿样本从生成到累积的过程核对概率测度，以及已经包含的补偿、几何和可见性因子。
3. 对照传输契约列出会受影响的不变量、贡献归属和物理边界情形；先区分算法问题、无贡献样本与数值失败，再决定处理位置。
4. 在现有职责内选择最小实现，复用真正相同的计算；验证留在其所属边界，保留实际计算需要的数值检查，不新增假想扩展层或反复全量校验。
5. 实施修改，为修正的行为或物理不变量补充/调整聚焦回归；优先使用解析结果、独立积分或可观察行为，不把实现抄成测试。
6. 按 testing.md 选择受影响的 Pipeline、随机流/线程回归、分量检查和实际示例，再执行适用的交付检查；记录渲染设置与真实结果。
7. 复核最终 diff：确认改动范围、数值边界、文档同步及测试有效性，清除本次产生的临时实验改动；报告行为变化、验证证据和未执行项。

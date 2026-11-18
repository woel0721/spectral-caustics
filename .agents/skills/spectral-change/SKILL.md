---
name: spectral-change
description: Use when implementing or fixing wavelength sampling, Hero/Packet transport, wavelength CRN, dispersion or wavelength-dependent IOR, spectral reconstruction, CIE/D65, XYZ integration, or Eλ atlases. Prefer architecture-refactor for structure-only changes and rendering-review for reviews.
---

# 光谱行为修改

读取 [project.md](../../project.md) 和 [architecture.md](../../architecture.md)；积分、重建与传输边界以 [transport.md](../../transport.md) 为准，回归与验收按 [testing.md](../../testing.md) 选择。光谱任务优先使用本 Skill，不因同时经过相机/光子模块而叠加普通渲染流程。

## 工作流程

1. 判断改动属于源端离散积分、相机波长抽样、光谱重建还是两端之间的图集数据；沿数据生产、存储、查询到 XYZ 累积确认归属。
2. 明确波长、密度、辐照度/辐亮度的单位、支持波段、分层方式、采样配额及归一化来源；数据变更须核对现有来源、许可和插值约定。
3. 实现前做任务所需的简短权重推导：追踪波长 PDF 或节点宽度、CMF 与归一化分别在哪一步应用，检查遗漏或重复补偿；不强制另建设计文档。
4. 在正确归属处做最小修改，保留 RGB、标量 Spectral 和 Photon 的职责及物理区别；重建近似不转化为额外材质能力或实测数据声明。
5. 按改动对象选择回归。反射或发光重建对照相应基谱、插值端点及取值范围。相机波长采样检查分层与配额，涉及 Packet 路径时检查 lane 独立状态、CRN 副本及提前终止。改动随机流或并行归约时验证固定 seed 的线程确定性。
6. 源端 XYZ 积分或 Eλ 图集变更，用独立细波长积分核对归一化与能量。图集存储或查询变化时核对重积分，相机估计量变化时与解析或数值参考比较，说明容差。按 testing.md 选择受影响的 Pipeline 与隔离检查，报告验证结果。

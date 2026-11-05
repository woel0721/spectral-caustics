# 当前实现与开发边界

本文件描述当前架构，不冻结历史阶段的 API。实现细节及运行方式见 README。

## 架构

- Scene 只管理几何、材质、光源、环境、背景和求交。相机及积分器选项由 RenderJob 组合。
- Renderer 只有 `render(&RenderJob)` 一个高层入口，负责 RGB / Spectral Pipeline 与 Rayon 调度。
- Camera Integrator 的共享路径循环处理 RGB 和标量光谱数值；两种像素 / 波长采样器保留独立随机流。
- Photon Integrator 只使用 Forward；XYZ / Eλ 共用发射、首交点、界面抽样和沉积。Constant 复用几何，Dispersion 独立追踪各波长。
- optics/interface 显式区分 Radiance 的折射率平方权重和 Photon Flux 权重。
- Framebuffer、线性颜色转换、曝光 / ACES / PNG 分开。Diagnostics 独立 feature，正常构建没有诊断缓冲。

## 估计器边界

- 相机最多 32 个 Delta 事件，连续反弹预算 0..1024。Rust 缺省漫反射 0；展示 JSON 使用 4。
- 首次连续顶点立即着色；后续顶点接收直接照明；只有首次连续地面顶点读取焦散图。
- RR 从第三次连续散射后开始，仅在有剩余预算时抽样，存活补偿 1/p。
- 环境 NEE / MIS 使用二值遮挡。漫反射后最近事件为 Delta 时允许环境逃逸；最近为 Diffuse 时截断，避免重计。
- 光子限一个理想 Dielectric Sphere / Prism / VerticalPrism，事件上限 1..1024，默认 32。其他物体阻断，截断通量不再分配。
- 平行光总通量通过水平窗口归一化；圆盘均匀面积、余弦方向发射，有限距离阴影用于相机 NEE。
- Hero 使用 380..780 nm 分层采样与 400/SPP 权重；Packet=4 保持每像素总路径配额和 CRN。源端积分使用独立 Δλ。
- 固定 seed 的各分量在 1/2/4 线程逐位一致。Direct + Caustics 在线性空间等于 Combined。

## 有意删除

- two_transmission 算法及选择枚举、配置、入口、历史适配测试。
- 重叠的 Renderer / Camera 单图与分量包装 API，Scene 上的光子追踪职责。
- SpectralPathState / SpectralBsdfSample / BsdfValue / EvaluateMode 的重复状态和动态数值变体。
- Ray 未使用的缓存字段，运行时 Hit 的可缺失物体 / 材质编号。
- 由 Debug / Release profile 决定 AOV 的隐式规则，旧 JSON 字段及未标记的光源格式。

## 当前没有扩展

没有新增材质、透明阴影、嵌套介质、环境焦散、通用 Photon Mapping、BVH / KD-Tree、GUI 或 GPU。光谱 GGX 仍拒绝。接收区外通量丢弃；RGB XYZ 焦散图绑定仍要求中性 Lambertian 地面；光谱使用材质的波长反射率。

## 重构的数值变化

- 所有光子焦散统一 Forward。多次反射 / 透射和 TIR 进入结果，取代旧双透射限定光路，因此旧 PNG 不作为逐位基准。
- 原无图 RGB 入口的太阳强度 Φ/A 修正为 Φ/(A·|d_z|)，使水平接收面的能量和光子发射归一化一致。
- 标量几何层先合并再乘光谱 / CMF，可能改变末位舍入；采样块重新分组允许容差。固定设置和线程数的确定性仍要求逐位一致。
- 三种支持的几何统一以 caustics=false 为 JSON 缺省值；显式选择非 Dielectric 将报错，不再静默忽略。
- 配置解析与实际构建验证分离；CLI 统一输出三个分量；Diagnostics 改为显式 feature。以上均不保留兼容入口。

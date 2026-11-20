# 当前实现与开发边界

本文件描述当前架构，不冻结历史阶段的 API。实现细节及运行方式见 README。

## 架构

- Scene 只管理几何、材质、光源、环境、背景、可选有界介质（Box<dyn Medium>，内置 Homogeneous/Grid 实现）和求交。Medium 与 Material 同为 Send + Sync trait；可序列化的 MediumConfig 仍用枚举选择内置类型。相机及积分器选项由 RenderJob 组合。
- Renderer 只有 `render(&RenderJob)` 一个高层入口，负责 RGB / Spectral Pipeline 与 Rayon 调度。
- Camera Integrator 的共享路径循环处理 RGB 和标量光谱数值；两种像素 / 波长采样器保留独立随机流。
- Photon Integrator 使用 Forward。XYZ 和 Eλ 共用发射、首交点、界面抽样和沉积。Constant 复用落点与线段几何，Dispersion 从首交点逐波长续追。
- optics/interface 显式区分 Radiance 的折射率平方权重和 Photon Flux 权重。
- Framebuffer、线性颜色转换、曝光 / ACES / PNG 分开。Diagnostics 独立 feature，正常构建没有诊断缓冲。

## 估计器边界

- 相机最多 32 个 Delta 事件，连续反弹预算 0..1024。Rust 缺省漫反射 0；展示 JSON 使用 4。
- 首次连续顶点立即着色；后续顶点接收直接照明；只有首次连续地面顶点读取焦散图。
- RR 从第三次连续表面散射后开始，仅在有剩余表面预算时抽样，存活补偿 1/p。
- 环境 NEE / MIS 使用二值遮挡。漫反射后最近事件为 Delta 时允许环境逃逸；最近为 Diffuse 时截断，避免重计。
- 光子限一个理想 Dielectric Sphere / Prism / VerticalPrism，事件上限 1..1024，默认 32。其他物体阻断，截断通量不再分配。
- 平行光总通量通过水平窗口归一化；圆盘均匀面积、余弦方向发射，有限距离阴影用于相机 NEE。
- Hero 使用 380..780 nm 分层采样与 400/SPP 权重；Packet=4 保持每像素总路径配额和 CRN。源端积分使用独立 Δλ。
- 固定 seed 的各分量在 1/2/4 线程逐位一致。Direct + Caustics 在线性空间等于 Combined。
- Camera Path 通过 Medium 的路径段采样、透射率与相函数接口使用介质。边界裁剪、体素遍历、Majorant 和空碰撞循环封装在介质内部。RGB/Hero/Packet 复用同一相机路径循环和独立介质 RNG。
- 相机体散射预算 `max_volume_scatters=0..1024`，缺省 1，雾示例 8；达到预算或纯吸收时仅积累解析 Tr。预算独立于表面事件，体散射不新增 RR；体照明与其后续表面贡献归入 Direct。表面/介质阴影均含 Tr，几何仍为二值遮挡。
- 均匀介质使用指数自由程。GridMedium 按 8³ 细体素块缓存最大密度，并按路径波长计算局部 Majorant。Delta Tracking 携带剩余光学深度跨块，以局部 sigma_s/sigma_t 做隐式吸收。所有块的最大密度相同时合并为单段。网格相机和阴影段的确定性消光使用精确细体素 DDA。纯吸收、零有效散射、预算耗尽或仅经过零上界块时无需抽样。[Phase 3.3 基准记录](docs/medium_majorant_benchmark.md)
- 均匀介质、显式网格和程序化雾的吸收和散射系数可配置成覆盖 380–780 nm 的分段线性曲线。Hero/Packet 的自由程、表面阴影和体照明按路径波长查询。 [均匀介质配置](docs/rendering_guide.md#均匀介质与多次体散射) · [网格配置](docs/rendering_guide.md#非均匀密度网格)
- XYZ 焦散、Eλ 图集与独立光子 Direct 预览对有限命中段应用逐波长 Tr，记录未发生体散射的通量。JSON 和 Rust API 可为一个封闭玻璃绑定内部介质，相机与光子在起射点分类，透射穿过绑定界面时切换区域。光子的 Air/Glass 状态负责折射率侧别。有限相机预算丢弃更高阶体散射。Ratio Tracking、层次化空块跳跃和介质发光仍未实现。局部 Majorant 仍有块遍历与缓存开销，均匀场景不保证加速。

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

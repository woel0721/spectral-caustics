# 传输契约与物理不变量

修改或审查估计器前按本文件区分物理量与概率测度。推导及使用示例复用 [README 的传输约定](../README.md#传输约定) 和 [均匀介质说明](../README.md#均匀介质与多次体散射)；职责边界见 [architecture.md](architecture.md)。

## 方向、测度与吞吐量

[Material/BsdfSample](../src/material/mod.rs) 中 `wi`、`wo` 均从交点向外，`wo=-ray.direction`；几何法线朝外，不由调用方提前统一翻面。世界距离用 m，波长接口用 nm，[Cauchy](../src/optics/dispersion.rs) 内部换为 μm。Ray 构造时归一化方向，不能把距离与任意长度的方向参数混用。

| 概率量 | 含义与使用 |
| --- | --- |
| 方向 PDF `pω`，sr⁻¹ | 连续 BSDF、环境、相函数相对于立体角的密度；连续 BSDF 样本的 `pdf=Some(pω)` |
| 面积 PDF `pA`，m⁻² | 圆盘位置采样相对于面积的密度；不能直接当作方向 PDF；转换时 `pω=pA·r²/abs(cos_light)` |
| 离散概率 `p_event`，无量纲 | Fresnel 反射/透射事件的选择概率；Delta 样本 `pdf=None`，不伪造普通方向 PDF |

相机 throughput β 是路径散射/介质权重的乘积，局部贡献按 β·L 累加。连续 BSDF 的 `weight` 已包含 `f·abs(n·wi)/pω`，Delta 已含离散系数/概率补偿；续追只乘 weight，不再次乘余弦、除 PDF 或乘 Fresnel。局部 NEE 则使用 BSDF 求值及其自身采样测度。连续分量与 Delta 分量的能力声明、事件和 PDF 必须一致。

## 相机 radiance 与光子 flux

[InterfaceSample](../src/optics/interface.rs) 共用事件但提供两种权重：

- 实际选中的反射系数/概率为 `F/F`，透射为 `(1-F)/(1-F)`，选中事件的概率非零；按 Fresnel 抽样时光子 `flux_weight=C/p_event=1`。不要再乘一次 F 或 1-F。
- 相机 `radiance_weight` 仅在透射时额外乘 `(η_i/η_t)²`；进入/离开同一介质的平方因子相消。该 Jacobian 不能加入光子通量。
- TIR 为概率 1 的反射；TIR 与 F=0 的确定事件不消耗事件选择随机数。镜面反射和透射都占 Delta/光子事件预算。
- 光子按显式 Air/Glass 状态选择 IOR 和法线；透射切换状态，反射不切换。[光子续追](../src/integrator/photon/path.rs) 从真实交点出发并依靠求交 epsilon 排除自交；不要直接复用相机的法线偏移，否则可能跨过棱镜尖棱。

## NEE、MIS 与可见性

[表面环境估计器](../src/integrator/camera/lighting/environment.rs) 对 HDRI 多样本采用固定配额：`n_bsdf=floor(N/2)`，`n_env=N-n_bsdf`。令 `q_i=n_i·p_i`，Balance 的单项贡献是 `g/(q_bsdf+q_env)`，Power 是 `g·w_i/q_i`、`w_i=q_i²/Σq_j²`；g 包含 BSDF、Li、接收余弦及阴影透射率。固定配额的贡献求和后不再除 N，奇数配额使用实际数量。

表面 HDRI 的 N=1 随机 50/50 选择策略并使用混合密度；渐变只做 BSDF 方向采样并平均。[体环境估计器](../src/integrator/camera/lighting/volume.rs) 在整球组合 HG/环境采样，N=1 也按设置选择 Balance/Power，不能套用表面的特殊分支。拒绝、背面、遮挡、零贡献或局部连续 NEE 抽到 Delta 时保留原配额，不补采。

[圆盘 NEE](../src/light/disk.rs) 的 `incident_weight=Le·cos_light/(r²·pA)` 已含光源余弦、距离和面积 PDF；表面再乘 BSDF、接收余弦、光谱密度、可见性并按样本数平均。体散射使用相函数，不能乘接收面余弦。[阴影](../src/integrator/camera/lighting/mod.rs) 对所有几何（含玻璃）保持二值遮挡，未遮挡段乘介质 Tr；圆盘线段裁到采样发光位置。

## Direct、Caustics 与路径终止

以 [共享路径循环](../src/integrator/camera/path.rs) 和 [CameraPathState](../src/integrator/camera/types.rs) 为准：

- 首次连续表面立即着色，最后预算顶点也先着色再终止。后续表面的太阳、圆盘、环境和允许的环境逃逸都计入 Direct；Direct 不等于仅一次反弹的直射光。
- 只有尚未遇到连续顶点的接收地面读取 Caustics；已有体散射或更早的连续材质会取消资格，后续地面不得重复读取。`has_non_delta_vertex` 标记遇到连续分量/体散射，不能与已采样的 `non_delta_bounces` 次数互换。
- 焦散图已是辐照度：RGB 中性地面用 `ρE/π`，Spectral 地面用当前波长反射率。不要再乘太阳强度、入射余弦或几何阴影；棋盘格在各适用表面贡献/续追权重中各应用一次。
- 相机及纯 Delta 前缀 miss 可查询环境或常量背景；连续历史之后仅最近事件为 Delta 且存在环境时累加完整 throughput 的环境项。最近为连续事件或刚发生 HG 散射的 miss 已由当地环境 NEE 负责，不再次累加；常量背景不变成环境光源。
- 当前相机最多 32 个 Delta 事件；`max_diffuse_bounces=0..1024` 实际限制连续表面事件，包括 glossy，不只 Lambertian。RR 在已有至少三次连续表面散射且仍有表面预算时、当前局部着色之后执行；RGB 用 β 最大分量，光谱用标量，`p=clamp(importance,0.05,0.95)`，存活 β 除 p。浅层、最终顶点和零贡献不额外抽 RR。
- 光子事件预算为 1..1024，默认 32；恰好用完后仍允许落地/逃逸，到下一次界面才截断。遮挡、逃逸、接收区外及预算截断的通量不重分配。预算截断不做能量补偿。
- 三个输出在线性 XYZ 中满足 `Combined=Direct+Caustics`；经过曝光/ACES/sRGB 的 PNG 不满足可加性。保留 HDR 线性值，不为“安全”把所有颜色夹到 `[0,1]`。

## 光谱积分与发射归一化

[光子 generation](../src/integrator/photon/generation.rs)、[光源](../src/light/mod.rs) 与 [光谱相机](../src/integrator/camera/spectral.rs) 的权重分工：

| 环节 | 权重与边界 |
| --- | --- |
| 空间发射/沉积 | 每条光子 Φ/N，再除接收像素面积得到辐照度；四邻域沉积保持区域内通量，增加空间样本数不能增加总能量 |
| 源端离散 XYZ 积分 | 对 `(λ_nm,Δλ_nm)` 梯形节点乘 D65 归一化、CMF 和 Δλ；不能再除波长数 |
| Eλ 图集 | 保存每 nm 的辐照度密度，不提前乘 CMF 或 Δλ；相机查询后自行做波长积分 |
| Hero/Packet 相机 | 固定 380..780 nm 分层；每条标量 Lλ 贡献 `CMF(λ)·Lλ·400/camera_spp`，不额外乘源端 Δλ |
| Packet=4 | camera_spp 仍是每像素总光谱路径数，空间主射线数为 SPP/4；不再除一次 Packet 大小 |

平行光 Φ 定义在水平发射窗口 A，单位传播方向 d 对应光束法向辐照度 `Φ/(A·abs(d_z))`；无阻挡水平面为 Φ/A。窗口只限制光子发射位置，不裁切解析太阳。圆盘均匀面积、余弦方向发射，`Le=Φ/(πA)`，匹配的发射 PDF 已使每条光子为 Φ/N。

源端平行光按配置节点离散归一化 D65；圆盘和光谱相机使用连续 380..780 nm 的固定 D65 归一化。不要假设不同波段/步长下 RGB、Hero 与源端重积分逐位相等。Φ 使用项目 D65/Y 相对标尺，未标定为瓦特。

[反射谱](../src/spectrum/mod.rs) 与 [发光谱](../src/spectrum/emission.rs) 使用不同的 PBRT 基谱近似：反射率限制在 `[0,1]`，发光重建保留 HDR 强度并做 D65 白点校准。不能互换，不能冒充实测光谱或保证任意 RGB 精确还原；CIE/D65 来源和许可证链接 [颜色数据说明](../assets/color/README.md)。RGB 相机仍用常数 IOR，不能因源端启用 Dispersion 而悄悄改变它。

## 均匀介质与 Beer–Lambert

[HomogeneousMedium::sample_segment](../src/medium/mod.rs) 先把盒交区间裁到最近表面或盒出口。`σa/σs` 为 m⁻¹，`σt=σa+σs`，`Tr(L)=exp(-σt·L)`：

| 情况 | 已补偿的相机路径权重 |
| --- | --- |
| 尚有体散射预算，σs>0 且自由程 s<L | `s=-ln(1-u)/σt`，距离 PDF 为 `σt·Tr(s)`；每次隐式吸收给出 `σs·Tr(s)/pdf=σs/σt` |
| 尚有体散射预算，自由程越过 L | 生存概率已经是 Tr(L)，权重 `Tr(L)/Tr(L)=1`，不能再乘一次 Tr |
| 纯吸收，或已达到相机体散射预算 | 解析乘 Tr(L)，不再产生新的体散射事件，也不消费介质 RNG |

[HG](../src/medium/phase.rs) 的 wo/wi 都向外，正 g 的前向峰在 `wi=-wo`；采样返回 `p=pdf`，续追乘 `p/pdf=1`，没有表面余弦。每次体散射调用当地 NEE 后重新进入共享路径循环；`volume_scatters` 只计体事件，`max_volume_scatters=0..1024` 是独立相机积分预算，缺省 1，不占连续表面/Delta 预算，不新增体散射 RR。0 或预算耗尽保留总消光，不补偿被截断的更高阶能量；当地体照明及后续表面贡献归 Direct。每次 HG 后的环境 miss 均由当地环境 NEE 负责，后续表面 Delta 链逃逸仍可计光。阴影 Tr 使用实际盒内段长，不能以整段距离或光子侧消光替代。

## 随机流与数值处理

[RGB 采样](../src/integrator/camera/rgb.rs)、[Spectral 采样](../src/integrator/camera/spectral.rs) 与 [流种子](../src/sampling/rng.rs) 分离像素/前缀、首次环境、连续续追/RR 和介质随机流；连续历史后的玻璃使用续追流。表面圆盘 NEE 持有派生游标，配额变化不消费额外路径/环境随机数；体照明和 HG 使用介质流。

Packet 的路径流从同一状态副本开始；某 lane 提前结束不推进其他 lane。光子按发射索引派生事件流、按固定索引顺序归约，不依赖 worker 调度。固定设置/seed 的 1/2/4 线程结果须逐位一致；采样块重分组或算法改变的舍入差异不作为旧版本逐位兼容要求。

保留归一化、临界角/TIR、epsilon、自交、合法零 PDF/零贡献及有限性/溢出处理。具体入口检查归属见 [project.md](project.md#错误与验证边界)，物理正确性与确定性分别按 [testing.md](testing.md) 验证。

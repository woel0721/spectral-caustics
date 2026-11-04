# 当前状态与开发边界

本文件只保留当前能力、限制与最近一次实现验收。运行和配置见 [README.md](README.md)，历史阶段、API 迁移和逐次验收记录见 Git 历史。下一项里程碑由用户指定，不自动推进未实现功能。

## 已实现

| 范围 | 当前能力 |
| --- | --- |
| 主程序 | Glass、Mirror、GGX 三球同场景；共享路径输出 Direct/Caustics/Combined |
| 几何 | 米制 Z-up、有限地面、Sphere、Prism、三条长棱各 8 段圆角的 BeveledPrism；材质绑定与最近交点 |
| RGB 材质 | Lambertian、Dielectric、Mirror、各向同性 GgxMetal，统一 `BsdfSample` 接口 |
| 相机路径 | 独立 RGB / Hero Wavelength 标量路径，最多 32 次 Delta、0..1024 次额外连接与补偿 Russian Roulette |
| 光谱相机 | 单波长或四波长 Packet，共享主射线和公共路径随机数、独立标量路径；总 SPP 个完整细分波长层；Dielectric/Lambertian/Mirror；CIE 与 1/pλ 的 XYZ Monte Carlo 积分 |
| 照明 | 解析太阳、HDRI/渐变环境、重要性采样、固定配额 Balance/Power MIS |
| 光源焦散 | 显式选择理想 Dielectric Sphere/Prism；材质 Constant/Cauchy IOR、双界面 Snell/Fresnel、地面 XYZ 双线性累积、CIE/D65 积分 |
| 调试 | debug 构建可输出四张路径 AOV 与 JSON，目标物体独立可选 |
| 工程 | JSON 校验、固定 seed、独立 RNG 流、Rayon 线程一致性、XYZ → 线性 RGB → 曝光/简化 ACES → sRGB PNG |

默认展示使用 768×432、256 SPP、8 个环境样本、Power MIS、4 次连续连接及随附 HDRI。Rust 内置默认值与展示 JSON 的区别见 README。

`configs/sphere.json` 为主要光谱测试场景，显式启用玻璃球的 `caustics` 和 Cauchy 模型，输出 Sphere 的 Direct/Caustics/Combined。默认三球对照 JSON 仍关闭光源焦散。旧 Prism 配置、独立 IOR 实验入口与 Debug 目标选择保留；相机 RGB BSDF 采样不随光源色散设置变化。

`configs/sphere_spectral.json` 为独立光谱相机场景（256×256、512 总路径、Packet=4、128 个空间样本）。`render.camera_mode` 默认 `rgb`，设为 `spectral` 后独立输出主图；`render.mode` 选择 Constant/Dispersion。太阳使用连续归一化 D65；RGB HDRI/天空/背景使用独立 PBRT Illuminant 基谱与 D65 白点校准，保留 HDR 强度。源数据与近似边界见颜色数据说明。

## 当前限制

- 两种相机均为有界路径估计器。RGB 保留首次地面读取 XYZ 焦散图的三分量流程；Hero 不读取已积分的 XYZ 图，也暂不输出 RGB 专用的三分量或路径 AOV。非 Delta 环境逃逸由局部 NEE/MIS 计光，不能重复累加背景。
- Hero 相机的单波长 IOR 支持色散；GGX RGB eta/k 尚无光谱模型，会明确报错。RGB 发光重建仅是近似，不代表实测 HDRI 光谱，也不保证任意 RGB 色度精确还原。单波长 Monte Carlo 有颜色噪声，需要足够 SPP。
- 光源端只追踪一个显式选中的理想介质 Sphere/Prism 的直接双透射，不追踪内部多次反射。BeveledPrism、Mirror、GGX 不生成光源端光谱焦散；默认三球对照场景的 Caustics 为零。
- 玻璃对太阳/环境 shadow ray 仍为二值遮挡；尚无透明阴影、嵌套介质或体积。GGX 仅支持各向同性单次散射，尚无 RoughDielectric、Plastic、各向异性或多次散射补偿。
- 尚无环境/金属焦散、通用 Photon Mapping、BVH/KD-Tree、GUI 或 GPU。圆角仍由平面段近似，未使用平滑法线。

## 最近一次实现验收

Packet 公共随机数（CRN）续改，2026-11-04。

- Packet=4 在每个空间样本建立玻璃前缀、首次环境 NEE、续追三类初始 RNG 状态；各 Lane 从相同状态的独立副本开始。波长层内抖动继续按 Lane 分开，Packet=1 保留原随机序列，SPP、PDF、XYZ 权重、Fresnel 和 RR 补偿保持原样。
- 只复用随机序列，不共享可变 RNG 或标量路径状态；提前结束或额外消费不影响其他 Lane。路径分叉后不强制同步物理事件或方向，未加入按反弹/事件固定维度的采样器。
- 保留原有全部 429 项测试，新增 2 项集成测试，当前 **341 项单元 + 90 项集成 = 431 项全部通过**；fmt、fmt check、locked 全目标构建、完整测试、严格 Clippy、diff check 均通过。新增测试检查真实积分器的环境/续追 CRN 对齐、Constant IOR 的共同玻璃事件，以及 Dispersion 下不同折射方向；加强玻璃前缀公共随机数断言，原提前结束隔离、能量与 1/2/4 线程确定性测试继续通过。
- 按本次要求仅做正确性回归，没有运行新的效果或性能实验。没有新增依赖、删除文件/API/测试；保留工作区已有的场景配置调整。

以下保留此前独立 Lane 随机流版本的 Packet 初版验收记录，其实验数据不代表当前 CRN 的效果。

4-Wavelength Packet 光谱相机，2026-11-04；核心实现提交 `1cc29e9`。

- `spectral_packet_size` 仅支持 1/4，Rust/旧 JSON 缺省为 1；光谱球展示 JSON 显式为 4。`camera_spp` 始终为总光谱路径数；Packet=4 要求整除，空间样本数为 SPP/4。配置和直接 API 校验非法配额及路径/环境工作量溢出。
- 每个 Packet 只生成一次抖动和主射线，四个 Lane 在各自 100 nm 波段内进一步完整分层，独立新建标量状态和随机流；继续复用原 `trace_spectral_camera_path()`、材质、Fresnel/Snell、MIS、RR 和输出链。新增 `render_spectral_camera_with_packet()`；原公开函数保留签名并固定 Packet=1。每项积分权重仍是 400/总路径数。
- 改动前 **420** 项基线全部通过；保留全部原测试，新增 **3 项单元 + 6 项集成**，当前 **341 项单元 + 88 项集成 = 429 项测试全部通过**，无失败或 ignored。fmt、fmt check、locked 全目标构建、完整测试、严格全目标 Clippy、diff check 均通过；release 光谱相机的 22 项测试及 Packet 的 2 项边界/RNG 单元测试也通过。
- 测试覆盖 4/16/512 个细分层各访问一次、浮点右端边界、共享主射线、Lane 提前结束/额外随机消耗的隔离、独立标量贡献、D65 白点/能量、Constant/Dispersion 折射、非法配置和溢出。两种 Packet、两种 IOR、Balance/Power、深度 0/4、1/2/4 线程及重复结果逐位一致；RGB、光源焦散和三个分量继续兼容。
- 修改前后保存的 12 张线性 XYZ 小图（Constant/Dispersion、SPP=1/7/16、深度 0/4）在 Packet=1 下逐字节相同，SHA-256 为 `3a22a5b0c72e4f8cd8df6d805691d0987be2aec1a7a17e937935782e59a91c4c`。未保留冻结旧算法作为测试实现。
- 实跑新 `spectral_packet_compare`：256×256、512 总路径、三种子、10 线程，完成两种 IOR 的 Packet=1/4 以及独立 4096 SPP 参考。球内色度方差的三种子估计降低约 4.14% / 1.28%，耗时增加约 27.2% / 22.8%；轮廓 Y 方差上升，彩色噪点仍明显。指标、定义、有限参考与少量种子的限制见 README。
- 主程序实跑原光谱球配置：256×256、512 总路径、Packet=4、Dispersion，成功输出，约 23.8 秒（与完整回归并行，不用于性能对比）。原 `camera_direct_caustics -- 8` 与 `camera_parallel_benchmark -- 8 48` 成功；后者 1/2/4/6 线程 XYZ 逐位一致。另实跑 96×96、8 SPP 的 RGB Sphere 配置，输出非零光源焦散及 Direct/Caustics/Combined。
- 新增可重复的对比示例与说明；没有新增依赖，没有删除文件、函数或测试。没有改变 RGB 或光源传输算法，也未引入 SIMD、通用 Packet 状态、去噪、BDPT、Photon Mapping 或 GPU。

# 当前状态与开发边界

本文件只保留当前能力、限制与最近一次实现验收。运行和配置见 [README.md](README.md)，历史阶段、API 迁移和逐次验收记录见 Git 历史。下一项里程碑由用户指定，不自动推进未实现功能。

## 已实现

| 范围 | 当前能力 |
| --- | --- |
| 主程序 | Glass、Mirror、GGX 三球同场景；共享路径输出 Direct/Caustics/Combined |
| 几何 | 米制 Z-up、有限地面、Sphere、Prism、三条长棱各 8 段圆角的 BeveledPrism；材质绑定与最近交点 |
| RGB 材质 | Lambertian、Dielectric、Mirror、各向同性 GgxMetal，统一 `BsdfSample` 接口 |
| 相机路径 | 独立 RGB / Hero Wavelength 标量路径，最多 32 次 Delta、0..1024 次额外连接与补偿 Russian Roulette |
| 光谱相机 | 每样本均匀采一个 380–780 nm 波长；Dielectric/Lambertian/Mirror；CIE 与 1/pλ 的 XYZ Monte Carlo 积分 |
| 照明 | 解析太阳、HDRI/渐变环境、重要性采样、固定配额 Balance/Power MIS |
| 光源焦散 | 显式选择理想 Dielectric Sphere/Prism；材质 Constant/Cauchy IOR、双界面 Snell/Fresnel、地面 XYZ 双线性累积、CIE/D65 积分 |
| 调试 | debug 构建可输出四张路径 AOV 与 JSON，目标物体独立可选 |
| 工程 | JSON 校验、固定 seed、独立 RNG 流、Rayon 线程一致性、XYZ → 线性 RGB → 曝光/简化 ACES → sRGB PNG |

默认展示使用 768×432、256 SPP、8 个环境样本、Power MIS、4 次连续连接及随附 HDRI。Rust 内置默认值与展示 JSON 的区别见 README。

`configs/sphere.json` 为主要光谱测试场景，显式启用玻璃球的 `caustics` 和 Cauchy 模型，输出 Sphere 的 Direct/Caustics/Combined。默认三球对照 JSON 仍关闭光源焦散。旧 Prism 配置、独立 IOR 实验入口与 Debug 目标选择保留；相机 RGB BSDF 采样不随光源色散设置变化。

`configs/sphere_spectral.json` 为独立 Hero 相机场景（256×256、512 SPP）。`render.camera_mode` 默认 `rgb`，设为 `spectral` 后独立输出主图；`render.mode` 选择 Constant/Dispersion。太阳使用连续归一化 D65；RGB HDRI/天空/背景使用独立 PBRT Illuminant 基谱与 D65 白点校准，保留 HDR 强度。源数据与近似边界见颜色数据说明。

## 当前限制

- 两种相机均为有界路径估计器。RGB 保留首次地面读取 XYZ 焦散图的三分量流程；Hero 不读取已积分的 XYZ 图，也暂不输出 RGB 专用的三分量或路径 AOV。非 Delta 环境逃逸由局部 NEE/MIS 计光，不能重复累加背景。
- Hero 相机的单波长 IOR 支持色散；GGX RGB eta/k 尚无光谱模型，会明确报错。RGB 发光重建仅是近似，不代表实测 HDRI 光谱，也不保证任意 RGB 色度精确还原。单波长 Monte Carlo 有颜色噪声，需要足够 SPP。
- 光源端只追踪一个显式选中的理想介质 Sphere/Prism 的直接双透射，不追踪内部多次反射。BeveledPrism、Mirror、GGX 不生成光源端光谱焦散；默认三球对照场景的 Caustics 为零。
- 玻璃对太阳/环境 shadow ray 仍为二值遮挡；尚无透明阴影、嵌套介质或体积。GGX 仅支持各向同性单次散射，尚无 RoughDielectric、Plastic、各向异性或多次散射补偿。
- 尚无环境/金属焦散、通用 Photon Mapping、BVH/KD-Tree、GUI 或 GPU。圆角仍由平面段近似，未使用平滑法线。

## 最近一次实现验收

Hero Wavelength 相机，本地阶段提交 `2d40a2c`（材质/发光光谱）、`d6f2a8c`（相机/配置/测试）与 `8dc9bca`（随机流隔离），2026-11-03。以下计数对应当前工作区，保留已有 ACES 输出改动：

- 原有 401 项回归测试保留；新增 2 项发光光谱、1 项 RR 单元和 15 项光谱相机集成测试。当前 **338 项单元 + 81 项集成 = 419 项测试全部通过**，无失败或 ignored；另通过原单波长示例的 2 项验证和 release 光谱相机的 15 项测试。
- fmt、locked 全目标构建、完整测试、严格 Clippy、diff 检查通过。没有新增依赖，没有删除文件、接口或测试。
- 检查 D65 连续 Y 归一化与 XYZ 白点、恒定 IOR/色散方向、Fresnel 分支频率和 IOR 平方权重、标量反射率/PDF、MC 收敛、HDR 增益、背景/环境逃逸与 NEE 去重、RR 补偿、32 次 Delta 上限、非法输入与 GGX 拒绝。
- HDRI 验证包含单样本、2/3/8 配额的 Balance/Power，以及高对比彩色 HDR、旋转和玻璃遮挡，与独立半球数值积分比较。光谱两种 IOR 模式、两种 MIS、深度 0/4 下，固定 seed 在 1/2/4 线程及重复渲染中逐位一致；RGB 入口继续与原独立相机入口一致。增加续追深度不消耗玻璃前缀与首次环境随机流，后续玻璃、环境和 RR 只使用续追流。
- 主程序实跑 `configs/sphere_spectral.json`：256×256、512 SPP、Dispersion，约 21.7 秒（与回归测试并行运行）。`spectral_camera_compare` 输出同采样配置的 Constant/Dispersion 对比图，两次分别约 13.3/13.6 秒。单波长估计仍有颜色噪声。
- 原 `camera_direct_caustics -- 8` 与 `camera_parallel_benchmark -- 8 48` 实跑成功；原光源与 RGB 三分量在 1/2/4/6 线程逐位一致。
- 路径循环独立保留，未把 RGB 状态改为泛型；仅共用介质界面采样、Fresnel/Snell、余弦方向、MIS 基础函数、可见性、偏移和 Delta 预算。当前没有通用 Photon Mapping、体积、BDPT 或 GPU。

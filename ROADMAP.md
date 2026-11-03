# 当前状态与开发边界

本文件只保留当前能力、限制与最近一次实现验收。运行和配置见 [README.md](README.md)，历史阶段、API 迁移和逐次验收记录见 Git 历史。下一项里程碑由用户指定，不自动推进未实现功能。

## 已实现

| 范围 | 当前能力 |
| --- | --- |
| 主程序 | Glass、Mirror、GGX 三球同场景；共享路径输出 Direct/Caustics/Combined |
| 几何 | 米制 Z-up、有限地面、Sphere、Prism、三条长棱各 8 段圆角的 BeveledPrism；材质绑定与最近交点 |
| RGB 材质 | Lambertian、Dielectric、Mirror、各向同性 GgxMetal，统一 `BsdfSample` 接口 |
| 相机路径 | 最多 32 次 Delta、0..1024 次额外连续连接、RGB 吞吐量及补偿 Russian Roulette |
| 照明 | 解析太阳、HDRI/渐变环境、重要性采样、固定配额 Balance/Power MIS |
| 光源焦散 | 显式选择理想 Dielectric Sphere/Prism；材质 Constant/Cauchy IOR、双界面 Snell/Fresnel、地面 XYZ 双线性累积、CIE/D65 积分 |
| 调试 | debug 构建可输出四张路径 AOV 与 JSON，目标物体独立可选 |
| 工程 | JSON 校验、固定 seed、独立 RNG 流、Rayon 线程一致性、XYZ → sRGB PNG |

默认展示使用 768×432、256 SPP、8 个环境样本、Power MIS、4 次连续连接及随附 HDRI。Rust 内置默认值与展示 JSON 的区别见 README。

`configs/sphere.json` 为主要光谱测试场景，显式启用玻璃球的 `caustics` 和 Cauchy 模型，输出 Sphere 的 Direct/Caustics/Combined。默认三球对照 JSON 仍关闭光源焦散。旧 Prism 配置、独立 IOR 实验入口与 Debug 目标选择保留；相机 RGB BSDF 采样不随光源色散设置变化。

## 当前限制

- 相机为有界 RGB 路径估计器，不做逐波长色散。仅首次连续着色顶点为地面时读取焦散图，续追不再读取；非 Delta 的环境逃逸由局部 NEE/MIS 计光，不能重复累加背景。
- 光源端只追踪一个显式选中的理想介质 Sphere/Prism 的直接双透射，不追踪内部多次反射。BeveledPrism、Mirror、GGX 不生成光源端光谱焦散；默认三球对照场景的 Caustics 为零。
- 玻璃对太阳/环境 shadow ray 仍为二值遮挡；尚无透明阴影、嵌套介质或体积。GGX 仅支持各向同性单次散射，尚无 RoughDielectric、Plastic、各向异性或多次散射补偿。
- 尚无环境/金属焦散、通用 Photon Mapping、BVH/KD-Tree、GUI 或 GPU。圆角仍由平面段近似，未使用平滑法线。

## 最近一次实现验收

Sphere 光源端光谱焦散提交 `17d2f61`（2026-11-03）：

- 修改前建立 **391 项**测试基线；修改后 334 项单元 + 66 项集成，共 **400 项测试通过**，无失败或 ignored；新增的 9 项 Sphere 光谱测试另在 release 通过。
- `cargo fmt`、`cargo fmt --check`、locked 全目标构建、完整测试、严格 Clippy 和 diff 检查通过。
- 主程序以 `configs/sphere.json` 实跑 512×512、256 SPP（release，约 33.8 秒），检查三张 RGB8 PNG，确认 Caustics 非零且含彩色色边。高亮重叠处接近白色。
- 新测试覆盖球面入/出射法线、球透镜解析 Snell 方向、常数/色散落点、双界面 Fresnel 通量、逐波长独立渲染的 XYZ 积分对照、空间/波长采样能量归一化及遮挡终止。Constant/Dispersion、Balance/Power、深度 0/4 下，固定 seed 在 1/2/4 线程及重复渲染中逐位一致；相机分量在线性 XYZ 中可加，修改光源色散不改变 Direct 或独立 RGB 相机输出。
- `camera_direct_caustics -- 8`、`camera_parallel_benchmark -- 8 48`、`sphere_materials -- 8 64`、`refract_ray` 实跑成功；基准的光源与三个相机 XYZ 缓冲在 1/2/4/6 线程逐位一致。
- 保留全部已有测试与兼容光学入口；未删除文件、函数或测试。Sphere/Prism 共用原有双透射核心，Cauchy 公式集中到光学模块。

这些记录对应上述实现提交；复验命令见 README，不作为历史图像逐位兼容要求。

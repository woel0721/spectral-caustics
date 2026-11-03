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
| 光源焦散 | 理想介质三棱镜双界面 Snell/Cauchy/Fresnel、地面 XYZ 双线性累积、CIE/D65 积分 |
| 调试 | debug 构建可输出四张路径 AOV 与 JSON，目标物体独立可选 |
| 工程 | JSON 校验、固定 seed、独立 RNG 流、Rayon 线程一致性、XYZ → sRGB PNG |

默认展示使用 768×432、256 SPP、8 个环境样本、Power MIS、4 次连续连接及随附 HDRI。Rust 内置默认值与展示 JSON 的区别见 README。

## 当前限制

- 相机为有界 RGB 路径估计器，不做逐波长色散。仅首次连续着色顶点为地面时读取焦散图，续追不再读取；非 Delta 的环境逃逸由局部 NEE/MIS 计光，不能重复累加背景。
- 光源端只追踪一个理想介质三棱镜的两次透射。Sphere、BeveledPrism、Mirror、GGX 均不生成光源端光谱焦散；默认三球场景的 Caustics 为零。
- 玻璃对太阳/环境 shadow ray 仍为二值遮挡；尚无透明阴影、嵌套介质或体积。GGX 仅支持各向同性单次散射，尚无 RoughDielectric、Plastic、各向异性或多次散射补偿。
- 尚无环境/金属焦散、通用 Photon Mapping、BVH/KD-Tree、GUI 或 GPU。圆角仍由平面段近似，未使用平滑法线。

## 最近一次实现验收

三球主程序提交 `2a26de0`（2026-11-03）：

- 334 项单元 + 57 项集成，共 **391 项测试通过**，无失败或 ignored；另通过 release 配置/CLI 与 AOV 的 23 项集成测试。
- fmt、locked 全目标构建/测试、严格 Clippy、diff 检查及 release 全目标构建通过。
- 默认主程序实跑 768×432、256 SPP；检查三张 RGB8 PNG，确认 Direct=Combined、Caustics 全黑。
- `camera_direct_caustics -- 8`、`diffuse_indirect -- 8`、`camera_parallel_benchmark -- 8 48`、`refract_ray` 实跑成功；基准的光源与三个相机 XYZ 缓冲在 1/2/4/6 线程逐位一致。

这些记录对应上述实现提交；复验命令见 README，不作为历史图像逐位兼容要求。

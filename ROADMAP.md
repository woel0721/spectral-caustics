# 当前状态与开发边界

本文件只保留当前能力、限制与最近一次实现验收。运行和配置见 [README.md](README.md)，历史阶段、API 迁移和逐次验收记录见 Git 历史。下一项里程碑由用户指定，不自动推进未实现功能。

## 已实现

| 范围 | 当前能力 |
| --- | --- |
| 主程序 | Glass、Mirror、GGX 三球同场景；共享路径输出 Direct/Caustics/Combined |
| 几何 | 米制 Z-up、有限地面、Sphere、Prism、XY 截面沿 Z 挤出的无倒角 VerticalPrism、三条长棱各 8 段圆角的 BeveledPrism；材质绑定与最近交点 |
| RGB 材质 | Lambertian、Dielectric、Mirror、各向同性 GgxMetal，统一 `BsdfSample` 接口 |
| 相机路径 | 独立 RGB / Hero Wavelength 标量路径，最多 32 次 Delta、0..1024 次额外连接与补偿 Russian Roulette |
| 光谱相机 | 单波长或四波长 Packet，共享主射线和公共路径随机数、独立标量路径；总 SPP 个完整细分波长层；Dielectric/Lambertian/Mirror；CIE 与 1/pλ 的 XYZ Monte Carlo 积分 |
| 照明 | ParallelLight / 单面 Lambertian DiskAreaLight；圆盘面积 NEE 与全半球余弦光子发射；HDRI/渐变环境、重要性采样、固定配额 Balance/Power MIS |
| 光源焦散 | 显式选择理想 Dielectric Sphere/Prism/VerticalPrism；材质 Constant/Cauchy IOR、双界面 Snell/Fresnel、地面 XYZ 与标量 Eλ 图集、CIE/D65 积分；RGB/Hero 共享路径三分量 |
| 调试 | debug 构建可输出四张路径 AOV 与 JSON，目标物体独立可选；固定波长相机与灯带亚像素投影诊断；光谱相机可临时关闭地面太阳直射以观察焦散 |
| 工程 | JSON 校验、固定 seed、独立 RNG 流、Rayon 线程一致性、XYZ → 线性 RGB → 曝光/简化 ACES → sRGB PNG |

默认展示使用 768×432、256 SPP、8 个环境样本、Power MIS、4 次连续连接及随附 HDRI。Rust 内置默认值与展示 JSON 的区别见 README。

`configs/sphere.json` 为主要光谱测试场景，显式启用玻璃球的 `caustics` 和 Cauchy 模型，输出 Sphere 的 Direct/Caustics/Combined。默认三球对照 JSON 仍关闭光源焦散。旧 Prism 配置、独立 IOR 实验入口与 Debug 目标选择保留；相机 RGB BSDF 采样不随光源色散设置变化。

`configs/sphere_spectral.json` 为独立光谱相机场景，当前实验配置为 512×512、2048 总路径、Packet=4、512 个空间样本、曝光 0.4，使用单灯带 HDRI（旋转 127.0546°）、线性反射率 0.18 的中性灰地面，棋盘格关闭；临时使用 Cauchy A=1.4907、B=0.0084 μm²。`render.camera_mode` 默认 `rgb`，设为 `spectral` 后使用独立标量路径，选中焦散目标时接入 Eλ 图集并输出三分量；`render.mode` 选择 Constant/Dispersion。太阳使用连续归一化 D65；RGB HDRI/天空/背景使用独立 PBRT Illuminant 基谱与 D65 白点校准，保留 HDR 强度。源数据与近似边界见颜色数据说明。

`configs/vertical_prism_spectral.json` 与 `configs/vertical_prism_caustics.json` 使用 XY 等边三角形沿 Z 挤出的无倒角竖直三棱柱。前者是条纹 HDRI 的光谱相机折射对照，使用反射率 1.0 的白色地面；后者通过现有 RGB 三分量流程观察太阳双透射的地面光谱焦散特写，保留反射率 0.18 的中性灰地面。两者均关闭棋盘格，配置及运行命令见 README。

`configs/vertical_prism_spectral_caustics.json` 将相同地面特写接入 Hero 相机：256×256、256 总 SPP、Packet=4、开启 `caustics`，同一批路径累计 Direct/Caustics/Combined。图集使用原光源节点及 D65 归一化，保存每 nm 标量 Eλ，不含 CIE 或 Δλ；相机最终仅使用一次 400/SPP 的积分权重。没有焦散目标时跳过预计算并保留旧输出。

`configs/vertical_prism_area_light.json` 用半径 0.01 m 的圆盘隔离有限光源焦散，关闭环境和背景；256×256、256 总 SPP、Packet=4、8 个 Direct 面积样本、源端每轴分层 8。圆盘采用与 Hero 相同的连续 D65 发射光谱，步长仅控制图集和求积，不改变节点发射密度。旧光源 JSON 省略 type 继续解析为 parallel；新 JSON 使用 disk_area，Rust 场景/配置的 light 字段为枚举。物理约定、不可命中发光几何的边界和调用方式见 README。

## 当前限制

- 两种相机均为有界路径估计器。RGB 保留首次地面读取 XYZ 焦散图的流程；Hero 在第一个 non-delta 顶点为地面时读取独立 Eλ 图集并输出三分量，仍不输出路径 AOV。波长线性插值会平滑窄色边，有限图集不代替连续光子追踪；非 Delta 环境逃逸由局部 NEE/MIS 计光，不能重复累加背景。
- Hero 相机的单波长 IOR 支持色散；GGX RGB eta/k 尚无光谱模型，会明确报错。RGB 发光重建仅是近似，不代表实测 HDRI 光谱，也不保证任意 RGB 色度精确还原。单波长 Monte Carlo 有颜色噪声，需要足够 SPP。
- 光源端只追踪一个显式选中的理想介质 Sphere/Prism/VerticalPrism 的直接双透射，不追踪内部多次反射。BeveledPrism、Mirror、GGX 不生成光源端光谱焦散；默认三球对照场景的 Caustics 为零。
- 玻璃对太阳/环境 shadow ray 仍为二值遮挡；尚无透明阴影、嵌套介质或体积。GGX 仅支持各向同性单次散射，尚无 RoughDielectric、Plastic、各向异性或多次散射补偿。
- 尚无环境/金属焦散、通用 Photon Mapping、BVH/KD-Tree、GUI 或 GPU。圆角仍由平面段近似，未使用平滑法线。
- 圆盘当前不作为可命中或遮挡的发光几何；其直接照明仅由专用 NEE 估计，不与环境 MIS 或 BSDF 命中重复计光。尚不包含 Camera→Glass→Disk 等看到发光面本体的路径；光子仍只做理想介质双透射，有限接收范围外的通量不会重分配。

## 最近一次实现验收

圆盘面光源与光谱焦散，2026-11-04。

- 新增 `Light::{Parallel, DiskArea}`、`DiskAreaLight`、JSON `LightConfig` 与独立面积 NEE 模块，保留 ParallelLight 和旧 JSON。新增每顶点 `light_samples`，默认 1；原 Rust 调用方显式迁移为枚举访问，没有删除原数值断言。RGB/Spectral 共享面积采样、有限阴影线段、几何项与 PDF，未重构材质或路径状态。
- 圆盘单面 `Le=Phi/(pi*A)`，均匀位置 PDF `1/A`、完整余弦半球 PDF `cos/pi`；光子权重抵消为 `Phi*S_lambda/N`，再乘原双界面 Fresnel、除像素面积并 splat。Scalar 图层不含 Δλ，XYZ/Camera 各沿原积分入口施加一次权重。圆盘与 Hero Direct 共用固定连续 D65；平行光保留原离散归一化。光子块定位扩展为 2/4 维 RNG，面积 NEE 使用独立游标，Packet CRN、环境 MIS、RR 和首次地面焦散查询保留。
- 改动前 452 项基线全部通过；新增 1 项随机块单元与 9 项集成测试，当前 **347 项单元 + 115 项集成 = 462 项通过**，无失败/ignored，doc tests 通过。覆盖完整半球/面积分布与功率、距离平方和两余弦、有限阴影及软阴影独立求积、半径/数量/波长步长的能量标尺、保留节点密度在缩窄波段后不变、Constant/Dispersion 落点及颜色、发光点位置扩展的光学模糊、节点 XYZ 重积分、两个相机三分量与 1/2/4 线程、Packet=1/4、NEE 配额与环境/续追 CRN 隔离。扩大最终 receiver 后，9 项新场景测试再次通过。
- fmt、fmt check、locked 全目标构建、严格全目标 Clippy、完整测试、diff check 通过，另有 `sphere_wavelengths` 示例的 2 项测试通过。实际渲染 256×256、256 总 SPP、Packet=4 的 0.01 m Dispersion/Constant 与 0.1 m Dispersion 三组场景，输出在 `output/vertical_prism/area_light/`、`constant/`、`radius_0_1/`；大圆盘边缘更柔，Constant 没有折射率色散边缘。原 `camera_direct_caustics -- 8` 的三张 PNG 与改动前逐字节一致；`camera_parallel_benchmark -- 8 48` 的 1/2/4/6 线程 XYZ 逐位一致。
- 无新增依赖，无删除文件/函数/测试。复用统一发射和双透射代码，RGB/光谱不复制几何 NEE。保留已有未提交的地面太阳调试开关；该开关只影响平行太阳，圆盘 Direct 保留。当前仍有有限光子数量、空间/波长图集及求积误差，未添加方向裁剪/重要性锥、透明阴影、发光面命中 MIS 或通用 Photon Mapping。

以下保留此前调试开关验收。

光谱地面太阳 Direct 临时调试，2026-11-04。

- 新增 `render.spectral_debug_disable_ground_sun`，缺省 false，debug/release 均有效；只在光谱 Camera 的每次地面着色时跳过解析太阳 Direct，继续累计环境与 Caustics。通过独立路径状态传递，不改太阳通量或源端生成；无焦散目标的单图入口也传递完整设置，RGB 忽略此开关。
- 保留原 449 项测试，新增 1 项配置单元与 2 项数值/渲染集成测试，**346 项单元 + 106 项集成 = 452 项全部通过**，doc tests 通过。覆盖默认/序列化、首个及后续地面顶点、非地面太阳直射、环境与 RNG 保留、源端 XYZ/Eλ 与相机 Caustics 逐位不变、Packet=1/4、Rayon 1/2/4 线程、RGB 与未开启焦散时的单图入口；fmt、fmt check、locked 全目标构建、严格全目标 Clippy、diff check 均通过。
- 实跑 256×256、256 总 SPP、Packet=4 的竖直三棱柱暗地面对照，输出及可重载配置在 `output/vertical_prism/spectral_caustics_ground_sun_off/`。调试前后的 `camera_caustics.png` 逐字节相同；Combined 的地面太阳亮底消失，保留环境底色和彩虹。原 `camera_direct_caustics -- 8` 与 `camera_parallel_benchmark -- 8 48` 实跑通过，后者 1/2/4/6 线程 XYZ 逐位一致。无新增依赖，无删除文件/函数/测试。

以下保留此前光谱焦散接入验收。

Hero Wavelength 相机接入地面光谱焦散，2026-11-04。

- 新增只读 `SpectralCausticsMap`：各波长节点保存相对通量/m²/nm 的标量 Eλ，复用源端双透射、发射序列和双线性权重，保留原离散 D65 归一化与空间通量标尺；不含 CIE、Δλ 或地面反射率。世界坐标双线性与波长线性插值，范围外返回零。按波长并行，每层一个标量图；Constant 仅追踪一次几何通量，无采样块级图集副本。
- 首次 non-delta 顶点为地面时累计 `throughput * rhoλ/pi * Eλ` 到独立 Caustics；Delta 前缀有效，后续地面不重复读取，也不再施加太阳 shadow ray。Direct 与原无焦散入口逐位一致，三个分量由同一批 Hero/Packet 样本积分为 XYZ，Combined 逐像素直接相加。旧低层无焦散 API、RGB 相机和未开启焦散时的主程序输出保留。
- 原 440 项测试保留，新增 1 项单元与 8 项集成，当前 **345 项单元 + 104 项集成 = 449 项通过**，无失败/ignored，doc tests 通过。新增覆盖空间/波长插值及短末区间、节点重积分还原原 XYZ 图、光源通量/空间样本数标尺、Constant/Dispersion 落点、玻璃前缀、非地面首连续顶点、二次遮挡、Packet=1/4 与独立细波长求积、未曝光分量精确相加，以及固定 seed、Balance/Power HDRI、深度 0/4、Rayon 1/2/4 线程逐位一致；原 Fresnel、MIS/PDF、RR、RGB 和光源端测试继续通过。
- fmt、fmt check、locked 全目标构建、完整测试、严格全目标 Clippy、diff check 通过。新配置两种 IOR 均实跑 256×256、256 总 SPP、Packet=4，输出在 `output/vertical_prism/spectral_caustics/` 与其 `constant/`；色散焦散有分离色带，固定折射率投影重合。原 `camera_direct_caustics -- 8` 与 `camera_parallel_benchmark -- 8 48` 实跑通过，后者 1/2/4/6 线程 XYZ 逐位一致。
- 无新增依赖，无删除文件/函数/测试；保留工作区已有场景和文档调整。图集有限空间/波长分辨率会平滑色边，Hero 连续求积与原节点梯形求积有不同误差；尚未添加环境焦散、透明阴影或通用 Photon Mapping。

以下保留此前实现验收。

无倒角竖直三棱柱与两套条纹场景，2026-11-04。

- 新增 `VerticalPrism` / JSON `vertical_prism`，由 XY 三角形和 Z 范围直接构建三角形上下盖及矩形侧面，自动统一绕序与朝外法线。复用已有凸三棱柱的三角面求交、材质和光子双透射，不使用旧横向顶点；没有修改原有传输算法。
- 新增光谱相机与地面焦散两份配置，均为等边截面、条纹 HDRI、中性灰地面、关闭棋盘格。比较示例增加可选配置路径，保持旧调用方式。实跑 256×256、256 总 SPP 的 Constant/Dispersion 相机对照，以及 256×256、64 相机 SPP 的地面三分量输出。
- 同一条中心太阳射线在 Constant 下三波长落点相同；Dispersion 的 450/550/650 nm 地面落点约为 (-0.4497, 3.1640)、(-0.5140, 3.1443)、(-0.5502, 3.1324) m，蓝红相距约 0.1053 m。地面独立 Caustics 图显示彩色投影；合成图的太阳直射会降低对比度，不将其解释为条纹 HDRI 产生的环境焦散。
- 改动前 433 项基线全部通过；保留所有原测试，新增 3 项几何单元与 4 项集成测试，当前 **344 项单元 + 96 项集成 = 440 项全部通过**，另有投影示例 2 项测试通过。fmt、fmt check、locked 全目标构建、完整测试、严格 Clippy、diff check 均通过；新测试覆盖端盖/侧面、绕序/共享棱、非法输入、折射与 Fresnel、色散落点、焦散能量、分量相加和 1/2/4 线程逐位一致。原 MIS、深度/RR、光谱与颜色测试保留。
- 实跑原 `camera_direct_caustics -- 8` 与 `camera_parallel_benchmark -- 8 48`，后者 1/2/4/6 线程 XYZ 逐位一致。没有新增依赖、删除文件/函数/测试；通过复用现有求交与双透射实现避免复制几何遍历和光学算法。

此前固定波长投影诊断的验收如下。

固定波长相机投影诊断，2026-11-04。

- 新增 `render::integrator::spectral::render_fixed_wavelength_camera()`，多个固定波长共享每条初始相机射线和三类 CRN，独立进行标量路径追踪。每张图各自使用指定 SPP，返回原始 Lλ，不进行波长积分；原 Packet/RGB 路径保持原样，仅共用原有输入校验。
- 新示例 `spectral_wavelength_projection` 固定 450/550/650 nm，输出 D65 归一化灰度图、伪彩位置叠加、原始中央剖面及球内灯带投影 CSV/JSON。半高边界从色调映射前的剖面线性插值，原点在左上，像素中心为 x+0.5/y+0.5；选取球内中央区域排除直接背景灯带和侧面反射。
- 433 项回归测试及示例的 2 项投影测试通过；fmt、locked 全目标构建、完整测试、严格 Clippy、diff check 通过。验证共享射线/CRN、单波长辐亮度单位、重复波长逐位一致、1/2/4 线程一致、非法波长/采样/溢出，以及亚像素边界和无效剖面处理。原测试全部保留，没有新增依赖。
- 实跑当前 512×512 场景，每波长 2048 SPP、seed=42、Dispersion，三张图使用同一组 536,870,912 条主射线，分别独立追踪。中央扫描线 y=256.5 px 的灯带中心分别为 285.3533、285.8566、286.1116 px，450→650 nm 中心向右移动约 0.7582 px。该数值是本场景的半高边界插值估计；64×64 Constant IOR 对照的三波长投影位置一致。

以下保留此前 CRN 实现的验收记录。

Packet 公共随机数（CRN）续改，2026-11-04。

- Packet=4 在每个空间样本建立玻璃前缀、首次环境 NEE、续追三类初始 RNG 状态；各 Lane 从相同状态的独立副本开始。波长层内抖动继续按 Lane 分开，Packet=1 保留原随机序列，SPP、PDF、XYZ 权重、Fresnel 和 RR 补偿保持原样。
- 只复用随机序列，不共享可变 RNG 或标量路径状态；提前结束或额外消费不影响其他 Lane。路径分叉后不强制同步物理事件或方向，未加入按反弹/事件固定维度的采样器。
- 保留原有全部 429 项测试，新增 2 项集成测试，当前 **341 项单元 + 90 项集成 = 431 项全部通过**；fmt、fmt check、locked 全目标构建、完整测试、严格 Clippy、diff check 均通过。新增测试检查真实积分器的环境/续追 CRN 对齐、Constant IOR 的共同玻璃事件，以及 Dispersion 下不同折射方向；加强玻璃前缀公共随机数断言，原提前结束隔离、能量与 1/2/4 线程确定性测试继续通过。
- CRN 实现阶段按要求仅做正确性回归；后续场景单图试渲染尚未构成 CRN 效果或性能的定量对照。没有新增依赖、删除文件/API/测试；保留工作区已有的场景配置调整。

以下保留此前独立 Lane 随机流版本的 Packet 初版验收记录，其实验数据不代表当前 CRN 的效果。

4-Wavelength Packet 光谱相机，2026-11-04；核心实现提交 `1cc29e9`。

- `spectral_packet_size` 仅支持 1/4，Rust/旧 JSON 缺省为 1；光谱球展示 JSON 显式为 4。`camera_spp` 始终为总光谱路径数；Packet=4 要求整除，空间样本数为 SPP/4。配置和直接 API 校验非法配额及路径/环境工作量溢出。
- 每个 Packet 只生成一次抖动和主射线，四个 Lane 在各自 100 nm 波段内进一步完整分层，独立新建标量状态和随机流；继续复用原 `trace_spectral_camera_path()`、材质、Fresnel/Snell、MIS、RR 和输出链。新增 `render_spectral_camera_with_packet()`；原公开函数保留签名并固定 Packet=1。每项积分权重仍是 400/总路径数。
- 改动前 **420** 项基线全部通过；保留全部原测试，新增 **3 项单元 + 6 项集成**，当前 **341 项单元 + 88 项集成 = 429 项测试全部通过**，无失败或 ignored。fmt、fmt check、locked 全目标构建、完整测试、严格全目标 Clippy、diff check 均通过；release 光谱相机的 22 项测试及 Packet 的 2 项边界/RNG 单元测试也通过。
- 测试覆盖 4/16/512 个细分层各访问一次、浮点右端边界、共享主射线、Lane 提前结束/额外随机消耗的隔离、独立标量贡献、D65 白点/能量、Constant/Dispersion 折射、非法配置和溢出。两种 Packet、两种 IOR、Balance/Power、深度 0/4、1/2/4 线程及重复结果逐位一致；RGB、光源焦散和三个分量继续兼容。
- 修改前后保存的 12 张线性 XYZ 小图（Constant/Dispersion、SPP=1/7/16、深度 0/4）在 Packet=1 下逐字节相同，SHA-256 为 `3a22a5b0c72e4f8cd8df6d805691d0987be2aec1a7a17e937935782e59a91c4c`。未保留冻结旧算法作为测试实现。
- 调整配置前的 HDRI/棋盘格实验实跑 `spectral_packet_compare`：256×256、512 总路径、三种子、10 线程，完成两种 IOR 的 Packet=1/4 以及独立 4096 SPP 参考。球内色度方差的三种子估计降低约 4.14% / 1.28%，耗时增加约 27.2% / 22.8%；轮廓 Y 方差上升，彩色噪点仍明显。指标、定义、有限参考与少量种子的限制见 README。
- 主程序实跑原光谱球配置：256×256、512 总路径、Packet=4、Dispersion，成功输出，约 23.8 秒（与完整回归并行，不用于性能对比）。原 `camera_direct_caustics -- 8` 与 `camera_parallel_benchmark -- 8 48` 成功；后者 1/2/4/6 线程 XYZ 逐位一致。另实跑 96×96、8 SPP 的 RGB Sphere 配置，输出非零光源焦散及 Direct/Caustics/Combined。
- 新增可重复的对比示例与说明；没有新增依赖，没有删除文件、函数或测试。没有改变 RGB 或光源传输算法，也未引入 SIMD、通用 Packet 状态、去噪、BDPT、Photon Mapping 或 GPU。

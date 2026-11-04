# Spectral Caustics Renderer

Rust CPU 离线渲染器，支持理想 Dielectric Sphere/Prism 的光源端光谱焦散、独立 Hero Wavelength 光谱相机，以及 HDRI、MIS、有界 RGB BSDF 路径和 Rayon 并行。默认展示 Glass、Mirror、GGX 三个球体；光源焦散场景为 [configs/sphere.json](configs/sphere.json)，光谱相机场景为 [configs/sphere_spectral.json](configs/sphere_spectral.json)。当前能力与开发边界见 [ROADMAP.md](ROADMAP.md)。

## 运行与输出

从仓库根目录执行，需要支持 Rust 2024 edition 的工具链：

```sh
cargo run --release --locked
cargo run --release --locked -- configs/sphere.json
cargo run --release --locked -- configs/sphere_spectral.json
cargo run --release --locked -- /path/to/config.json
```

默认读取 [configs/default.json](configs/default.json)，使用随附的 `assets/HDRI/herkulessaulen_4k.hdr`。画面从左到右为玻璃（IOR=1.52）、Mirror（反射率 0.95）、GGX（roughness=0.4，RGB eta=0.2、k=3），地面为棋盘格。配置、HDR 和输出的相对路径均以启动目录为基准。

| 输出 | 内容 |
| --- | --- |
| `render.output_path`，默认 `output/spheres/camera_combined.png` | Combined = Direct + Caustics |
| 同目录的 `camera_direct.png` | 太阳、环境与相机续追贡献 |
| 同目录的 `camera_caustics.png` | 首次地面着色的光谱焦散贡献 |

三图共用一批相机路径。默认三球对照场景没有开启焦散；`sphere.json` 显式选择色散玻璃球，输出到 `output/sphere/`，其中 `camera_caustics.png` 包含球体的光谱焦散，`camera_combined.png` 为合成图。多波长重叠处接近白色，色散主要体现在焦散色边。自定义主图文件名时只保存指定名称；辅助图与主图重名时加 `aux_` 前缀。PNG 使用同一曝光，经 XYZ → linear sRGB → 曝光 → 简化 ACES → display sRGB 输出，生成图位于忽略的 `output/` 中。

ACES 对曝光后的每个线性 RGB 通道应用 `x=max(x,0)`、`clamp(x*(2.51*x+0.03)/(x*(2.43*x+0.59)+0.14),0,1)`；进入曲线前不把高光直接截断为 1。XYZ 缓冲和分量合成仍在线性空间中进行，仅 PNG 导出应用色调映射。Direct + Caustics = Combined 在导出前成立；经过非线性的 ACES/sRGB 后，PNG 像素不能直接相加。统计 AOV 保留原有直接量化方式。

## 配置

| 参数 | Rust 内置默认 | 展示 JSON |
| --- | --- | --- |
| `render.width/height` | 512×512 | 768×432 |
| `render.camera_spp` | 64 | 256 |
| `render.spectral_packet_size` | 1 | 默认展示省略；光谱球场景为 4 |
| `render.camera_mode` | `rgb` | 默认展示为 `rgb`，光谱相机场景为 `spectral` |
| `scene.environment_samples` | 1 | 8 |
| `scene.environment_mis_heuristic` | `balance` | `power` |
| `scene.max_diffuse_bounces` | 0 | 4 |
| `render.debug_path` | `false` | `false` |

`Config::default()` 保留单三棱镜实验场景和渐变天空；主程序加载上述 JSON。JSON 省略 `scene.environment` 或设为 `null` 会关闭环境照明，使用常量背景；环境中的 `map_path` 省略/null 时使用渐变，否则加载 HDR。`rotation_degrees` 绕 +Z 旋转，`intensity` 在线性空间缩放。

- `camera_spp` 始终是每像素总路径数；光谱 `spectral_packet_size` 只支持 1/4，取 4 时 SPP 必须被 4 整除。RGB 和光源端的采样不受 Packet 设置影响。`samples_per_pixel_axis` 独立控制光源每轴分层采样。
- `seed` 固定随机种子；`thread_count=null` 使用 Rayon 默认线程数，正整数创建专用池；`max_spectral_blocks` 控制光源采样块数。
- `max_diffuse_bounces` 表示额外连续 BSDF 连接数，包含 Diffuse/Glossy，范围 0..1024；4 最多访问 5 个连续着色顶点。
- 场景采用米制、Z-up；JSON 支持一个地面、至多一个三棱镜及多个球体/圆角三棱镜。Sphere/Prism 的 `caustics` 开关显式选择至多一个理想 Dielectric 焦散目标，不依赖物体下标或 `debug_object_id`。Sphere 缺省关闭；Prism 缺省开启以兼容旧配置，非介质 Prism 仍只遮挡光线。选择 Sphere 时应关闭其他 Prism 的开关；Sphere 开启时绑定非介质材质会报错。
- `receiver` 限制光子图范围，地面在其外扩 `ground_margin_m`，不限制解析太阳照明。
- `render.mode=constant` 使用材质的 `ior`；`dispersion` 使用其 Cauchy 模型，未配置模型则仍为常数。它同时控制光源端与独立光谱相机的 IOR 模式；RGB 相机始终使用常数 `ior`。光子积分的波长范围/步长以 nm 配置，光谱相机固定在 380–780 nm 采样。

材质使用 `type` 区分，所有参数必须有限，颜色为线性 RGB：

| 类型 | 参数与范围 |
| --- | --- |
| `lambertian` | `albedo`：三个 [0,1] 分量 |
| `dielectric` | `ior`：有限正数；可选 `dispersion`：Cauchy 参数 `a>0`、`b_um2≥0` |
| `mirror` | `reflectance`：三个 [0,1] 分量 |
| `ggx_metal` | RGB `eta>0`、`k≥0`，`roughness∈[0,1]` |

球体光谱配置的关键字段：

```json
{
  "type": "sphere",
  "center_m": [0.0, 0.0, 0.83],
  "radius_m": 0.8,
  "caustics": true,
  "material": {
    "type": "dielectric",
    "ior": 1.52,
    "dispersion": { "a": 1.5046, "b_um2": 0.0042 }
  }
}
```

`n(λ)=a+b_um2/λ_μm²`；输入波长为 nm，模型内部转换为 μm。上述参数用于演示，不代表特定实测玻璃。显式 Dielectric 的 `dispersion` 省略/null 表示常数；整个 `material` 省略时，保留旧演示玻璃默认值（1.52 和上述 Cauchy）。Rust 构造 `MaterialConfig::Dielectric` 需提供 `dispersion`，构造 `ObjectConfig::Sphere/Prism` 需提供 `caustics`；现有调用方已迁移。手工场景使用 `add_with_material` 后调用 `select_caustics_object(id)`，单条材质光谱路径可用 `trace_caustics_photon`；旧 `prism_id`、显式 IOR 的 `take_photon_landing` 和演示 `refractive_index` 保留。

完整球体配置见 [configs/sphere.json](configs/sphere.json)，圆角三棱镜见 [configs/beveled_prism.json](configs/beveled_prism.json)。圆角仅作用于三条长棱，每角固定 8 段；`bevel_size_m` 是沿原边的切点距离，每条边须长于它的两倍。实际求交使用分段几何法线，没有插值平滑法线。字段与校验见 [src/config](src/config)。

## Hero Wavelength 光谱相机

`render.camera_mode="spectral"` 将每个像素的 380–780 nm 范围按总 SPP 等宽细分，整条标量路径保持各自的波长。`spectral_packet_size` 缺省为 1，保留旧的波长、抖动及路径随机序列；设为 4 时，每个空间样本只生成一次子像素偏移和初始 Camera Ray，分别追踪四条独立路径：

| 总 SPP=512 | Packet=1 | Packet=4 |
| --- | --- | --- |
| 空间样本 / 初始射线 | 512 | 128 |
| 单波长路径数 | 512 | 512 |
| 波长分层 | 全范围 512 层 | 四个 100 nm 波段各 128 层 |

四个 Lane 分别在 380–480、480–580、580–680、680–780 nm 内再按空间样本数细分；每个细分层恰取一个均匀波长。共享初始射线和只读 Scene，各 Lane 新建 `SpectralPathState`，独立维护波长、throughput、radiance、散射事件、PDF 和深度，Dielectric 按各自 `n(λ)` 计算后续方向。Packet=4 的波长层内抖动仍由 seed、像素、空间样本、Lane 和 WAVELENGTH_STREAM 确定。玻璃前缀、首次环境 NEE、续追三类路径流采用公共随机数（CRN）：每个 Packet 为各类流建立一份初始状态，四个 Lane 各自复制同一初始状态后追踪，公共路径种子不依赖 Lane。相机流仍只生成一次主射线。各 Lane 的可变 RNG 游标独立，提前结束或额外抽样不会消耗其他 Lane 的序列；不同 Packet、像素及三类流之间仍隔离。后续 BSDF、玻璃、环境与 RR 只消费续追流。CRN 复用各流的第 k 个随机数，不按反弹/事件固定维度；路径分叉后，随机数对应的物理事件可能错位，不强制同步方向或分支。Rayon 不改变像素内累积顺序。

各层等配额混合的边缘 PDF 为 `1/400 nm⁻¹`。总路径数 `N=(SPP/packet_size)*packet_size=SPP`，最终直接累积 `Σ cie_xyz(λ)*Lλ/(pλ*N)`，每项积分权重均为 `400/SPP`；不额外乘 `Δλ`，也不再次除以 Packet 大小。所有贡献累积为同一个线性 XYZ 像素，最后才曝光、ACES 和 sRGB 编码。

- 支持 Dielectric、Lambertian、Mirror。Dielectric 在当前 λ 下进行 Fresnel/Snell 采样，radiance 透射权重为 `(eta_i/eta_t)²`，反射/透射的选择概率已补偿；连续两个相反界面的 IOR 平方因子相消。Lambertian/Mirror 使用已有 `rgb_reflectance()` 近似重建反射光谱。GGX 的 RGB eta/k 没有唯一物理光谱，配置和运行时入口会明确拒绝该材质。
- 太阳采用连续归一化 D65（380–780 nm 的 Y 积分为 1）。RGB HDRI、渐变和背景使用独立的 PBRT **Illuminant** 七基谱，并相对白基谱校准为 D65；保留 HDR 强度，不使用反射率基谱或 [0,1] 上限冒充发光光谱。这是近似重建，任意 RGB 色度不保证精确还原；来源、校准方法、许可见 [颜色数据说明](assets/color/README.md)。
- HDRI 方向采样沿用 RGB 亮度分布作为 proposal，实际 BSDF 与辐亮度均在当前 λ 计算。固定配额仍使用 `q_i=n_i*p_i` 的 Balance/Power MIS，单样本随机混合，渐变使用纯 BSDF 方向；被遮挡和背半球样本不补采。
- 保留最多 32 次 Delta、`max_diffuse_bounces` 连接预算和第三次连续连接之后的补偿 RR；光谱 RR 使用标量 `p=clamp(throughput,0.05,0.95)`。非 Delta 逃逸由局部 NEE/MIS 负责；漫反射后经 Delta 链逃逸环境才额外计光。玻璃对局部 shadow ray 仍是二值遮挡，有限深度仍会截断贡献。

该模式独立输出主图，不生成 RGB 专用的 Direct/Caustics 分量或调试 AOV，也不读取已经对波长积分的 XYZ 焦散贴图。当前光源端焦散仍通过原 RGB 三分量流程展示；不将 XYZ 贴图反解成单波长能量。主程序在光谱模式下直接调用相机，不执行光子图构建。

`Render::render_camera()` 按配置选择 RGB 或光谱；`Render::render_spectral_camera()` 遵循配置中的 Packet 大小。公开函数 `render_spectral_camera()` 保留原签名并固定 Packet=1；新增 `render_spectral_camera_with_packet(scene,width,height,spp,packet_size,seed,ior_mode)`。`trace_spectral_camera_path()` 保持单波长接口。原 RGB 入口继续保持 RGB 语义。示例场景为 256×256、512 总路径、Packet=4。

Packet=4 将独立空间样本数减为四分之一，可能降低颜色方差，也可能增加轮廓或纹理边缘的空间噪声；公共随机数只改变 Lane 间的相关性，各单波长估计的 PDF、Fresnel/RR 补偿、400/SPP 权重和总路径预算保持原样；不保证每个场景都优于 Packet=1。

```sh
cargo run --release --locked --example spectral_camera_compare
```

同一相机、seed 和波长采样配置下，输出 `output/sphere_spectral/camera_constant.png` 与 `camera_dispersion.png`，并保存两份可直接加载的配置。可传 `[SPP] [边长] [输出目录]`。

公平预算、多种子和高 SPP 参考对比：

```sh
cargo run --release --locked --example spectral_packet_compare -- 512 256 output/spectral_packet 3 4096
```

依次对比 Constant/Dispersion 的 Packet=1/4，种子从 42 递增；参考图为 Packet=1、独立种子 20261104。输出 PNG、可重载配置、线性 `.xyz64`（行优先 little-endian f64 XYZ）和 `metrics.json`。统计曝光前的 XYZ 方差/MSE、xy 色度方差、球内部/轮廓边缘，并单独计时相机渲染。参考图仍含有限 SPP 噪声，少量种子不能代表所有场景。

2026-11-04 实跑上述命令：256×256、每像素 512 条光谱路径（每帧 33,554,432 条）、10 线程、种子 42/43/44，分别使用两种 IOR 模式的 4096 SPP 参考。球内部为中心射线命中球体、排除四邻域轮廓的 11,880 像素；轮廓包含两侧的 708 像素。色度方差是跨种子的像素 `Var(X/(X+Y+Z))+Var(Y/(X+Y+Z))`，下表方差/MSE 再对球内部像素取均值。MSE 使用曝光前 XYZ，三个通道取均值；耗时只计相机渲染。

下表为 `31bb1b6` 的独立 Lane 随机流版本结果，不代表当前 CRN 的效果。本次 CRN 改动仅做正确性回归，未运行效果或性能实验。

| IOR | Packet | 球内 xy 方差 | 球内 XYZ MSE | 平均耗时 |
| --- | --- | --- | --- | --- |
| Constant | 1 | 1.34110e-3 | 4.22506e-3 | 10.717 s |
| Constant | 4 | 1.28563e-3 | 4.17284e-3 | 13.633 s |
| Dispersion | 1 | 1.32549e-3 | 4.25199e-3 | 12.000 s |
| Dispersion | 4 | 1.30858e-3 | 4.14856e-3 | 14.740 s |

本次三种子估计中，Packet=4 的球内色度方差分别降低 **4.14% / 1.28%**，XYZ MSE 降低 **1.24% / 2.43%**；耗时增加 **27.2% / 22.8%**。轮廓色度方差降低 16.2% / 8.4%，但轮廓亮度 Y 方差增加 **7.0% / 26.9%**，体现空间样本减少的代价。全图平均 Y 在两种 Packet 下相差均小于 0.002%，没有四倍能量变化。独立 Lane 的玻璃分支、环境照明和续追噪声仍明显，首版仅观察到有限改善；三种子与有限参考不足以保证所有场景或等耗时预算下获益。

## 路径调试

设置 `render.debug_path=true`，使用 debug 构建运行：

```sh
cargo run --locked -- /path/to/config.json
```

Release 忽略此开关。`scene.debug_object_id` 是物体的零基下标；默认三球配置中 0/1/2 分别为 Glass/Mirror/GGX，3 为地面。省略/null 时自动选择三棱镜，没有三棱镜则无目标。

| 文件 | 含义 |
| --- | --- |
| `path_target.png` | RGB 分别表示到达地面、逃逸到环境、发生内部全反射的路径比例；三项可重叠 |
| `reflection_count.png` | 镜面与 Glossy 反射的平均次数 |
| `transmission_count.png` | 镜面透射的平均次数 |
| `first_hit_normal.png` | 世界空间首次几何法线，按 `(n+1)/2` 编码 |
| `path_debug.json` | 物体命中、事件、TIR 和终止原因的原始计数 |

四张 AOV 只统计主光线首次命中目标的子样本，分母为像素总 SPP，保留边缘覆盖率。统计 PNG 不应用曝光或 sRGB 曲线；次数按 `clamp(ln(1+平均次数)/ln(33),0,1)` 显示，准确值保存在 JSON。记录不消费随机数，也不改变普通图像；几何逃逸不等于该路径一定增加环境辐亮度。

## 示例

```sh
cargo run --release --locked --example camera_direct_caustics -- 8
cargo run --release --locked --example sphere_materials -- 256 512
cargo run --release --locked --example sphere_wavelengths
cargo run --release --locked --example diffuse_indirect -- 16
cargo run --release --locked --example camera_parallel_benchmark -- 16 64
cargo run --release --locked --example refract_ray
```

这些示例包括默认场景三分量、单球四材质对照（Glass/Mirror/GGX 0/0.4）、单波长焦散、0/1/4 深度间接光、1/2/4/6 线程 XYZ 一致性，以及单光线双界面折射。`sphere_materials` 使用 debug 构建时还会输出各材质 AOV。

`sphere_wavelengths [SPP] [边长] [输出目录]` 读取当前 `configs/sphere.json`，分别输出 450/550/650 nm 的相机焦散及地面俯视焦散，默认目录为 `output/sphere/wavelengths/`。三个波长保留相同几何、发射网格、seed、采样块和曝光，因此逐条发射位置相同；每条路径使用材质的 `n(λ)` 和双界面 Fresnel 权重。图像为对应 D65 积分节点的真实 CIE XYZ 贡献（当前节点宽度 10 nm），使用完整光谱的共同归一化，不分别调整各波长亮度；相机仍使用原来的 RGB 材质。示例的两项独立验证可运行 `cargo test --locked --example sphere_wavelengths`。

## 实现与物理边界

- 入口流程见 [src/main.rs](src/main.rs)：`Config::load` → `Scene::from_config` → `Render::render_lighting` → `render_camera_glass_components` → `Render::save`。相机只读取光源图的 Caustics；sampled Direct 用于能量诊断，不能与解析太阳重复相加。
- 相机由统一 `BsdfSample` 推进 RGB 吞吐量，Dielectric/Mirror/光滑 GGX 合计最多 32 次 Delta 事件。第三次连续连接后的顶点已着色且仍有预算时执行 RR，`p=clamp(max(throughput),0.05,0.95)`，存活除以 p 补偿。
- 首次表面立即着色；仅首次连续着色顶点为地面时读取焦散图，续追不再读取。已有连续散射历史时，仅最近事件为 Delta 且有环境的 miss 累加环境到 Direct；非 Delta miss 由局部 NEE/MIS 负责。太阳/环境 shadow ray 对玻璃仍采用二值遮挡。
- GGX 为各向同性、单次散射金属，`alpha=roughness²`，使用完整 NDF 采样；无效反射保留零贡献、不补采。`roughness≤0.001` 切换为理想镜面。默认 eta/k 为演示参数，RGB 参数不代表真实光谱，未做微表面多次散射补偿。
- 光源焦散只追踪选中理想介质 Sphere/Prism 的两次透射：首次求交进入后，通过 `Scene::intersect()` 查找同一物体出射面，再求地面交点。折射率由绑定材质查询，两个界面使用同一波长的 `n(λ)`；Snell/Fresnel 函数保持原有数值行为。光子通量乘 `(1-F_enter)(1-F_exit)`，不乘相机 radiance 的 IOR 平方因子；反射、TIR、其他物体遮挡和区域外通量不再分配。D65/CIE XYZ 积分只计一次 `Δλ`，保留原有空间发射归一化与地面双线性 splat。相机三分量 API 要求接收地面为中性 Lambertian；彩色地面可使用独立 RGB 入口 `render_camera_glass`。
- RGB 与 Hero 光谱相机均为有界路径估计器。尚无透明阴影、嵌套介质、环境/金属焦散、通用 Photon Mapping、BVH/KD-Tree、体积、GUI 或 GPU；有限深度仍会截断路径。

几何/光学与 XYZ 使用 `f64`；固定配置和 seed 在当前实现的 1/2/4 线程下结果一致。模块入口为 [src/render/integrator.rs](src/render/integrator.rs) 和 [src/scene.rs](src/scene.rs)。CIE/D65、PBRT 反射/发光基谱的来源、许可证与校验和见 [颜色数据说明](assets/color/README.md)；RGB 到光谱是近似模型。

## 验证

```sh
cargo fmt
cargo fmt --check
cargo build --locked --all-targets
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
git diff --check
```

测试覆盖几何、光谱/颜色、Fresnel、MIS/PDF、路径深度/RR、分量可加性、非法输入及固定种子/线程一致性。最近一次实现验收见 [ROADMAP.md](ROADMAP.md)。

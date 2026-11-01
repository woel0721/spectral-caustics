# Spectral Caustics Renderer — Rust 开发路线图

> 项目目标：从一个能输出 PNG 的小型 CPU 离线程序开始，逐步扩展为支持玻璃折射、光谱色散与焦散的渲染器。先做出**能解释、能验证、能重复生成**的结果，再追求参考图那样复杂的视觉效果。
>
> 使用方式：将本文件放到仓库根目录，交给 Codex 按里程碑逐项实现。**不要一次性生成全部代码。**

## 0. 第一周范围（必须遵守）

**一周 MVP：** 一束平行白光穿过一个悬在地面上方的**三棱镜**；每个波长按照折射率独立计算光路；光线落在水平地面后，生成一张彩色焦散分布图，并输出 `output/caustics.png`。

第一版画面是**地面焦散图的俯视图**，不是同时拥有漂亮玻璃外观、体积光和真实摄影背景的成片。三棱镜通常会产生相对简单的色带，参考图那种复杂彩色纹路留待后续曲面玻璃阶段。

**实现方法：** 光源正向光线追踪（forward photon tracing）+ 地面二维能量累积。这借鉴了 Photon Mapping 的光子发射阶段，但**不是**完整 Photon Mapping（暂不需要 KD-Tree、光子邻域查询或通用全局光照）。

**本周不做：** 通用软光栅管线、Path Tracing、任意 OBJ 场景、完整 PBR、光子 KD-Tree、实时渲染、`egui`、GPU 加速、复杂 UI、体积散射。不要为了“架构完整”提前添加这些功能。

## 1. 技术选择

- 语言：Rust stable；纯 CPU。按用户要求，光谱渲染使用 Rayon 并行，固定采样块和合并顺序以保证可复现；灰度调试仍串行执行。
- 向量/矩阵：`nalgebra`。
- PNG：`image`，只负责图片编解码，不负责核心渲染算法。
- 采样：`rand`，使用固定随机种子；光谱分块用 `rand_chacha` 的直接定位接口划分同一随机序列。
- CLI：最初可以只用默认配置；需要命令行选项时使用 `clap`。
- 浮点表示：光学与几何计算优先 `f64`；帧缓冲保存线性 XYZ `f64`，输出时转换为 8-bit sRGB。
- CPU 并行：`rayon`，已按用户要求接入 `render_spectral_caustics`。
- 后续候选依赖：`egui`/`eframe`（预览窗口），**本周不加入**。

当前目录结构（按用途整理）：

```text
spectral-caustics/
├── Cargo.toml
├── Cargo.lock
├── ROADMAP.md
├── AGENTS.md
├── README.md
├── configs/
│   └── default.json           # 默认场景和渲染参数
├── assets/
│   └── color/                 # 可追溯的 CIE/D65 数据
├── output/                    # PNG 输出和已保存的阶段示例
├── src/
│   ├── main.rs                # 程序入口
│   ├── lib.rs                 # 模块入口
│   ├── camera.rs              # 针孔相机与像素主光线
│   ├── scene.rs               # 场景、求交、Cauchy 与光子追踪
│   ├── config/                # JSON 读写、CameraConfig、MaterialConfig、渲染/场景描述
│   ├── geometry/              # Ray、Hit、Hittable、地面、三角形、三棱镜
│   ├── material/              # Material trait、Lambertian、Dielectric
│   ├── optics/                # Snell 折射、折射率模式、CIE/D65 与光谱积分
│   └── render/                # Render、光子累积、Framebuffer、sRGB/PNG
├── examples/
│   └── refract_ray.rs
└── tests/
    └── config_render.rs
```

各源码目录通过 `mod.rs` 声明子模块，完整文件说明见 README。默认配置移至 `configs/default.json`，默认运行命令仍为 `cargo run --release --locked`。相机阶段的新增范围与验收见下方 V0.4 阶段 1。

模块可以随实现适度合并；避免为了匹配目录树写空文件。

## 2. 渲染流程与物理约定

```text
平行光源（二维发射孔径随机/分层采样）
  -> 对每条空间光线选择若干可见光波长（380–780 nm）
  -> 光线与三棱镜外表面求交
  -> 空气 -> 玻璃：按当前波长 Snell 折射
  -> 光线与玻璃的出射面求交
  -> 玻璃 -> 空气：再次 Snell 折射
  -> 与水平地面求交
  -> 按光子权重，将 XYZ 贡献 splat 到地面二维缓冲
  -> 固定曝光 + 色调映射（如需要）+ sRGB 编码
  -> image crate 输出 PNG
```

**坐标与单位：** 世界长度统一使用米，波长在数据接口中统一使用 `nm`；计算 Cauchy 公式时明确转换为 `μm`。地面是 `z = 0`；三棱镜完整放在地面上方；光束从上方斜射，避免初版复杂阴影和遮挡。

**三棱镜几何：** 可先用一个固定三角截面沿一轴拉伸形成 3D 三棱镜（共 8 个三角形），遍历所有面求最近的正向交点即可。注意三角形法线方向、射线进入/离开表面的判断，以及 `epsilon` 防止自相交。不必引入 BVH。

**折射：** 每条光线在入射面和出射面都要计算折射；先用固定折射率（例如 `1.52`）调通，再加入色散。全反射须正确检测，第一版可以终止该光子并记录统计，不要产生 NaN。

**色散模型：** 第一版使用可解释、可调的 Cauchy 模型：

\[
n(\lambda)=A+\frac{B}{\lambda_{\mu m}^{2}}
\]

初始示例参数可以使用 `A = 1.5046`、`B = 0.0042`（仅为演示参数，不声明它是特定真实玻璃的精确模型）。短波长通常具有更高折射率。提供“固定折射率”和“色散折射率”两种运行模式，以验证效果确实来自色散。

**光谱与颜色：**

- 尽量采用可追溯的 **CIE 1931 2° 颜色匹配函数**，配合 D65 光源 SPD；两者应统一波长范围与插值方式。记录数据来源和许可，不要杜撰标准表值。
- 第一版可从 12–16 个波长样本起步，质量改善后使用 380–780 nm、每 10 nm 一点（41 点）。
- 对离散波长积分/采样务必使用正确权重：积分求和要乘 `Δλ`；若随机抽样波长，则考虑 `1 / p(λ)`，不能仅把每条光子当成等权重的彩色点。
- 光谱贡献先映射到线性 XYZ 累积，再转到**线性** sRGB；在导出 PNG 前才进行曝光/色调映射与 sRGB 传递函数编码。需处理超出显示色域的颜色。
- 增加空间光线样本数时，总发射能量不应线性变亮；每条光线的相对通量按总发射数量归一化。光谱求和的能量尺度也要固定。
- 落点 splat 使用最近像素或双线性分配即可；后期可用规范化的核过滤改善噪点。固定曝光用于对比，**不要每张图单独按最大值自动归一化**，以免掩盖能量差异。

## 3. 七天里程碑（Codex 按顺序逐项完成）

### Day 1 — 输出链路与最小 Framebuffer

- [x] 创建可编译项目（现有目录使用 `cargo init`），加入 `nalgebra` 和 `image`。
- [x] 实现二维 `Framebuffer`：宽高、线性 `XYZ` 三通道存储、边界安全访问。
- [x] 实现 XYZ -> 线性 sRGB -> sRGB8 -> PNG 的导出接口；使用位置渐变测试图验证。
- [x] 运行 `cargo run --release --locked`，得到 512×512 的 `output/test.png`。
- **验收（已通过）：** PNG 尺寸、像素索引、sRGB 编码单测正确；`cargo test --locked` 通过。
- **验证：** PNG 读回测试检查 3×2 尺寸、RGB8 格式、六个已知位置的颜色，以及线性灰色 0.5 编码为 188；`cargo fmt --check` 和 `cargo clippy -- -D warnings` 通过。

### Day 2 — 光线、几何与求交

- [x] 实现 `Ray { origin, direction }`，方向归一化。
- [x] 实现射线与地面平面求交。
- [x] 实现射线与固定三棱镜三角面求交（Möller–Trumbore 或等效算法），取最近有效交点。
- [x] 输出一张简单的地面命中点灰度图，先不折射。
- **场景求交：** `Triangle` 已实现双面求交；`Prism` 已将固定 6 个顶点组成朝外法线的 8 个三角形并查询最近交点。`scene::Scene::add` 返回物体编号，`intersect` 查询整个场景的最近交点并填写 `object_id`，添加顺序不影响遮挡判断。
- **地面输出：** `Ground` 负责水平面求交；`caustics.rs` 从 `z = 3` 米发射固定网格的向下平行光，只累计最先命中地面的光线，把 `x、y ∈ [-2, 2)` 米映射到线性 XYZ Framebuffer，复用现有 sRGB 编码输出 512×512 的 `output/ground_hits.png`。三棱镜暂时不透明，阴影为黑色，照亮处为灰色。
- **验收（已通过）：** 已知射线的交点、表面法线、最近交点与自相交测试正确；PNG 读回验证 RGB8、灰色 188 与黑色阴影，增加采样数不会使未遮挡区域变亮。67 项单测及严格 `cargo clippy --locked --all-targets -- -D warnings` 通过，release 运行已生成默认 PNG。

### Day 3 — 双界面折射（暂不分色）

- [x] 实现向量形式 Snell 定律，区分空气 -> 玻璃、玻璃 -> 空气。
- [x] 射线完成“入射三棱镜 -> 内部传播 -> 出射 -> 命中地面”的完整路径。
- [x] 先固定折射率 `n = 1.52`；将所有落点以单色累积在地面。
- [x] 正确处理近零角度、法线方向、全反射与 `epsilon`。
- **当前实现：** `scene::take_photon` 根据首次命中的物体返回直接地面落点，或完成空气 → 玻璃 → 空气 → 地面的三次求交；其他物体、全反射与未命中返回 `None`。入射用朝外法线，出射取反，起点偏移 `1e-5` 米。`caustics.rs` 批量调用该函数并进行归一化灰度累积；主程序以相同采样和曝光输出 512×512 的 `output/no_prism.png` 与 `output/refracted_hits.png`。
- **验收（已通过）：** 已知单光线地面落点从无玻璃时 `(0.25, 0.2, 0)` 偏移至约 `(-0.7196, 0.2, 0)`；测试覆盖法向与近法向入射、临界角两侧、全反射、法线方向、三个传播阶段的其他遮挡、编号与添加顺序、批量能量归一化以及折射灰度 PNG 读回。78 项测试、严格 Clippy 及 release 图片生成通过；仍使用固定折射率，不含色散。

### Day 4 — 波长与色散

- [x] 实现 `wavelength_nm -> refractive_index`（Cauchy），完成单位换算。
- **Cauchy 与模式子项已实现：** `scene::refractive_index(wavelength_nm, mode)` 使用上述演示参数，将 nm 转为 μm。`IorMode::Constant` 忽略波长并返回默认玻璃折射率 `1.52`；`IorMode::Dispersion` 要求 `Some(波长)`，拒绝缺失、非正、非有限波长及计算溢出。`take_photon` 接收 `glass_ior: impl FnOnce() -> Option<f64>`，调用一次闭包并校验折射率，两个界面使用同一数值；调用方通过 `|| refractive_index(Some(波长), mode)` 捕获参数，也可传入 `|| Some(1.52)`。现有批量灰度对照显式使用 `Constant`，三波长调试使用 `Dispersion`。单光线测试核对 450 nm 与 650 nm 在固定模式下落点相同、在色散模式下落点不同；批量入口已统一为 `render_refracted_ground_hits(..., wavelengths_nm, mode)`，传 `[None]` 返回一张固定折射率图，传波长数组返回同长度的图像数组，每张图等权分配 `1/N` 通量。
- [x] 在相同空间光线下追踪多个波长，并正确累计采样权重。
- **三波长调试已实现：** 每条空间光线依次追踪 450、550、650 nm，分别用双线性 splat 累积到三张灰度图。光源暂定义为三个等权离散分量，每个分配 `1/3` 通量；丢失光子不重新分配能量，增加空间采样数不增加总亮度。所有光路共享种子 `42` 和空间发射位置，输出 `output/wavelength_450nm.png`、`output/wavelength_550nm.png`、`output/wavelength_650nm.png`。尚未进行连续光谱积分或真实波长颜色转换。
- [x] 增加 `constant_ior` 与 `dispersion` 两种模式，使用相同随机种子作对比。
- **统一入口已实现：** `render_refracted_ground_hits` 接收波长数组和 `IorMode`，复用同一套分层采样与双线性累积。固定模式各波长图相同，色散模式分布不同；单张与多张固定模式图的 XYZ 通量一致，空数组和非法色散波长返回错误。默认五张 PNG 的输出路径和通量尺度保持不变。
- [x] 临时输出不同波长的落点位置/调试散点图，不要求最终颜色准确。
- **验收：** `n(450 nm) > n(650 nm)`；两模式落点分布不同且结果可复现。

### Day 5 — 光谱积分与彩色焦散

- [x] 导入带来源说明的 CIE 1931 CMF / D65 SPD 数据，并做线性插值。
- [x] 把每个光子光谱贡献转为 XYZ，正确使用波长积分权重。
- **光谱计算核心已实现：** `optics::spectrum::cie_xyz` 与 `d65_spd` 查询官方 1 nm 数据，非整数波长线性插值，表外返回零；`wavelength_to_xyz(λ_nm, S, Δλ_nm, k)` 返回 `k × S × CMF × Δλ` 的线性 XYZ。`d65_normalization` 按相同的 `(波长, 积分宽度)` 样本计算 `k = 1 / Σ[S × ȳ × Δλ]`，使完整 D65 白光 Y = 1，梯形积分端点使用半宽度。数据来源、CC BY-SA 4.0 许可、原始 metadata 与校验和保存在 `assets/color/`。已测试标准表值、插值、D65 白点、步长稳定性及非法输入/溢出；光子光谱合成见下项。
- **彩色合成函数已实现：** `render::caustics::render_spectral_caustics` 在 380–780 nm 上使用 41 点梯形积分，共用 D65 的 k，独立执行固定种子的分层采样、双界面追踪和双线性 splat。预计算各波长的折射率与含空间通量权重的 XYZ 贡献，每条波长光线落地后直接累加到同一张 Framebuffer，不调用灰度渲染入口或生成 41 张中间图。固定折射率模式保持白光，色散模式产生空间颜色差异；覆盖无遮挡 XYZ 守恒、空间采样数归一化、独立单波长合成对照、遮挡通量不重分配、可复现、尺寸/编号/溢出和 PNG 读回测试。主程序输出见下项。
- [x] 以 XYZ 浮点缓冲累积，再转换为 sRGB8 PNG。
- [x] 输出第一张真正通过光路计算得到的 `output/caustics.png`。
- **主程序输出已接入：** `main` 使用相同的 512×512 场景、每轴 4 个空间分层样本、种子 42、41 点 D65/CIE 积分和曝光 1.0，分别输出色散 `output/caustics.png` 与固定折射率 `output/constant_ior.png`，并继续输出原来的五张灰度调试图。两次完整 release 运行的七张 PNG 均逐字节一致，原五张图与接入前也一致；读回确认全部为 512×512 RGB PNG。固定模式 RGB 通道最大差为 1 字节，色散图出现空间分色。109 项测试、严格 Clippy、格式与 diff 检查通过；当前设置的色带主要位于光斑边缘。
- **验收：** 固定折射率模式下白光原则上不应出现人为的空间彩虹分离；色散模式下能看到空间上分离的彩色光斑/色带；无明显异常色偏（需检查白点与曝光）。

### Day 6 — 可复现、对比与质量

- [x] 增加参数：图片宽高、空间光线数、波长采样数、折射率模式、曝光、随机种子、输出路径。
- **配置数据与 JSON 读写已定义：** 按用户指定的步骤，先实现 `RenderSetting`、`SceneConfig` 和统一入口 `Config`，由 `configs/default.json` 保存 `render`、`scene` 两部分。配置使用普通数值、数组和物体类型描述，支持默认值、JSON 往返、文件读写和基本参数校验，不保存 `Hittable` 实例、RNG、线程池或像素缓冲。现已由运行时 `Scene`、`Render` 和 `main` 读取并用于实际计算，见下面的运行时接入项。
- **配置层验收：** 新增 7 项测试覆盖默认 JSON、两种模式、文件读写、未知/缺失字段、非法参数、非有限数值写入保护和 `f64` 坐标精度往返；全套 120 项测试、严格 Clippy、格式和 diff 检查通过。
- **运行时验收：** 121 项单元测试和 7 项集成测试、严格 Clippy、格式和 diff 检查通过；集成测试实际启动程序读取临时 JSON，检查尺寸、模式、曝光、输出目录/文件名，另验证几何、光源、接收区域、种子、波长和线程/分块设置。默认 release 运行的七张 512×512 RGB PNG 与接入前逐字节一致，保留总耗时输出。
- **配置运行时已接入：** `main` 默认读取 `configs/default.json`，可传入另一配置路径。`Scene::from_config` 构建真实地面、三棱镜、归一化光源和接收区域，图片尺寸移交 `RenderSetting`；`Render` 持有参数和可选专用线程池。光源孔径/方向/高度/通量、接收范围、几何顶点、尺寸、采样、种子、波长范围/步长、模式、线程/分块数、线性曝光和输出路径均接入。默认仍输出七张图，主图采用配置模式，另输出相反模式对照；相机视角与背景不参与当前地面焦散计算。当前支持一个接收地面、至多一个凸三棱镜。
- **分层随机采样已接入：** 每个分层小格使用两轴独立的 `[0, 1)` 偏移发射一条光线，保持原有光线数量和通量归一化。每次渲染重新用种子 `42` 初始化 `StdRng`，使无玻璃和折射场景共享发射位置；85 项测试覆盖分层覆盖、格内范围、可复现、能量尺度与噪声变化。种子现由 `render.seed` 配置，光谱与灰度路径共享其序列。
- [ ] 输出 `output/constant_ior.png` 与 `output/dispersion.png`，用相同场景和相同曝光对比。
- [x] 为地面累积增加双线性 splat 或简单规范化核过滤，减轻离散噪点。
- **双线性子项已实现：** 按用户要求先接入此项。`render::utils::bilinear_weights` 计算四邻域归一化权重；地面映射保留连续像素坐标、使用像素中心约定，累积时分配四份线性 XYZ。接收区域内的边缘半像素夹紧，避免丢能量；84 项测试覆盖四点权重、非正方形映射、边界与 1×1 等尺寸、XYZ 守恒和采样数量归一化。其余 Day 6 项目仍待实现。
- [ ] 记录发射数、成功落地数、全反射/丢弃数、渲染耗时；按需增加简单性能优化。
- **直射路径优化已实现：** 每条空间光线的首次最近求交移到波长循环外。彩色渲染直接落地时，将含原有 D65/CIE、`Δλ` 和空间权重的 41 项 XYZ 合并后只做一次双线性 splat；色散模式首次命中三棱镜时，各波长复用该交点继续双界面追踪；其他遮挡或未命中直接丢弃。灰度调试复用直射落点，每张图仍使用 `1/N` 通量。保留单光线 `take_photon` 接口；运行统计和参数接口仍待实现。
- **固定模式路径合并已实现：** `Constant` 在渲染前只计算一次玻璃折射率 `1.52`，每条空间光线只追踪一条双界面路径，落地后将完整白光 `direct_xyz` 做一次双线性 splat；只有 `Dispersion` 缓存和追踪逐波长的折射率与 XYZ 贡献。完整 D65/CIE 光谱仍在发射前积分一次，以保留原有白点、`Δλ` 和空间能量权重。固定模式的 N 张灰度调试图共享同一条路径的结果，各自保留 `1/N` 通量。
- **Rayon 光谱分块并行已实现：** 按用户要求将 `render_spectral_caustics` 的空间采样行分为最多 16 个固定块，以 `into_par_iter().map(...).collect()` 并行生成各块独立的 XYZ Framebuffer，全部完成后顺序合并。各块 RNG 起始状态由原有种子 42 的串行序列划分，空间权重仍按全图光线数计算；分块和归并顺序与工作线程数无关。`Hittable` 增加 `Send + Sync`，测试计数器使用原子类型；灰度追踪入口和光学模型不变。可用 `RAYON_NUM_THREADS` 控制线程数；默认尺寸下局部图加最终图的像素缓冲峰值约为 102 MiB。
- **并行验收：** 113 项测试、严格 Clippy、格式和 diff 检查通过；测试确认 1/2/4 线程及重复运行的 XYZ 缓冲完全一致，分块 RNG 与旧串行序列相同。旧串行版、Rayon 1 线程与 4 线程的默认七张 512×512 RGB PNG 均逐字节一致。初版并行实现的本机单次完整七图运行耗时分别为 7.89、8.11、3.83 秒；这里计入仍为串行的灰度调试输出，不代表光谱追踪单独的加速比。
- **随机序列定位优化：** 去掉分块前按样本串行推进 RNG 的重复循环。显式使用当前 `StdRng` 对应的 `rand_chacha::ChaCha12Rng`，以 `set_word_pos(start_row × samples_x × 4)` 直接定位块起点；分块准备只遍历最多 16 个任务，各线程只生成实际使用的随机数，采样位置保持与原串行版本一致。
- **验收：** 增大样本数量图像更稳定，平均亮度不会随样本数直接倍增。

### Day 7 — 测试、文档、封版

- [ ] 添加光线几何、Snell、Cauchy、波长权重、Framebuffer 边界等测试。
- [ ] 用固定随机种子保存一组对比图作为视觉回归样例。
- [ ] 编写 README：项目目标、物理简化、运行命令、参数、示例图片、已知限制和下一步。
- [ ] 确认 `cargo fmt --check`、`cargo test`、`cargo clippy -- -D warnings` 通过（确有必要时可解释并调整非关键 lint）。
- **验收：** 新机器上可用一条命令输出 PNG，且能展示“无色散 vs 有色散”的明显区别。

## 4. 第一周完成定义（Definition of Done）

- [ ] 全程用 Rust CPU 运行；不依赖 GPU 或实时图形 API。
- [ ] 光束穿过**两个玻璃边界**，不是仅按波长画几条假的彩虹线。
- [ ] 有明确的“入射 -> 折射 -> 出射 -> 地面”路径与可测试的向量计算。
- [ ] 至少 12 个波长样本；光谱最终通过 CIE XYZ 转换到 sRGB。
- [ ] 输出真实计算得到的 `caustics.png` 与恒定折射率对照图。
- [ ] 同样参数 + 同样随机种子 = 可复现的图像。
- [ ] 代码能编译，测试能运行，README 有操作说明。

**延期原则：** 如时间不足，优先完成「正确两次折射 + 地面光子累积 + 恒定折射率/色散对比」；不要为了 GUI、美化或抽象接口牺牲最小结果。可以把 D65/41 波长的完整颜色精度作为额外一天的技术债明确记录，但不要用伪 RGB 着色谎称物理准确的光谱结果。

## 5. 第一周之后的扩展（不属于本周任务）

1. **V0.2：更好看的焦散**——玻璃球、透镜、波纹玻璃；增加发射图案和更好的核密度估计。复杂彩色纹理主要来自**曲面几何**，不是仅增加波长数。
2. **V0.3：更完整的光学模型**——Fresnel 反射/透射、吸收（Beer–Lambert）、多次内部反射、实际玻璃色散参数（Sellmeier）。
3. **V0.4：可见场景渲染**——简单软件光栅化或相机射线追踪，显示玻璃形状、地面、光照，并叠加焦散。注意焦散 Pass 与场景可见性 Pass 分离。
4. **V0.5：GUI 预览**——`egui` + `eframe`：调光源/材料/样本数，后台渲染，上传 CPU 输出的图像预览。
5. **V0.6：通用 Photon Mapping**——支持任意表面与场景，存储光子、KD-Tree 查询、密度估计；需要时再研究 BDPT/VCM。
6. **V0.7：进一步性能优化**——在已有 Rayon 光谱并行基础上改善负载均衡、缓冲和归并开销；如需实时再评估 `wgpu`。

### V0.4 阶段 1 — 针孔相机与 JSON

- [x] 基于新增的 `Camera` 定义实现 `ray_for_pixel(x, y, width, height)`，从相机位置穿过像素中心，返回归一化的 `Ray`。
- [x] 明确左上像素原点、图像 y 向下、垂直 FOV、宽高比与正交相机坐标系；处理零尺寸、越界、非法参数与近平行 up。
- [x] 在 JSON 的 `scene.camera` 中加入 `position`、`target`、`up`、`fov_y_degrees`，以 `CameraConfig` 进行读写/校验，由 `Scene::from_config` 建立运行时 `Camera`。
- [x] 将原来预留的 `scene.fov_degrees` / `Scene.fov` 合并到相机中，视场角只有一个来源。
- [x] 相机构建时缓存 `forward`、`right`、`camera_up` 和 `half_height`，所有像素直接复用；运行时参数只读，修改配置后重建相机以更新缓存。
- **验收：** 已知像素的方向、成像平面位置和宽高比正确；JSON 往返、错误参数与非有限值保护通过；相机主光线实际命中默认三棱镜，改变目标后命中地面。原有光源焦散渲染输出保持不变。
- **范围：** 本阶段完成相机和配置；相机视图的逐像素调试见阶段 2，材质、光照与路径追踪留待后续阶段。

### V0.4 阶段 2 — 相机物体分类调试图

- [x] 在 `render` 中实现 `render_camera_debug(scene, width, height) -> Result<RgbImage, _>`，逐像素生成主光线并查询最近交点。
- [x] 按场景物体编号使用固定 RGB8 调试色：三棱镜 `[100, 160, 230]`、地面 `[190, 190, 190]`、背景及其他物体 `[25, 30, 45]`。
- [x] 在 `main` 中调用并保存主图目录下的 `camera_debug.png`，复用配置中的尺寸和相机；名称冲突时保存为 `debug_camera_debug.png`。
- **验收：** 已知像素的三种颜色正确，物体添加顺序不影响显示；非法尺寸被拒绝；程序实际输出配置尺寸的 PNG，相机朝向改变会改变调试图，曝光为零时调试色仍保留，同名主图不会被覆盖。
- **范围：** 本阶段完成物体分类与轮廓调试；当前的法线显示见阶段 3，基础光照、玻璃材质和路径追踪留待后续阶段。

### V0.4 阶段 3 — 世界空间法线可视化

- [x] `render_camera_debug` 的所有命中改为显示 `Hit.normal`，保持最近交点查询与深色背景；函数签名及主程序输出路径不变。
- [x] 在 `render::utils` 中实现 `normal_to_rgb8`：归一化法线后，按 `(n + 1) / 2` 映射并量化到 RGB8，拒绝零向量、非有限法线和模长溢出。
- [x] x/y/z 分别编码到红/绿/蓝，使用原始世界空间几何法线，不朝相机翻转，不施加曝光或 sRGB 传递函数。
- **验收：** 六个轴向和斜向法线的颜色正确，向量长度不改变颜色；三棱镜前端面 -y、地面 +z、背景和物体顺序测试通过，从地面上下两侧观察得到相同的 +z 法线颜色，程序输出与曝光/文件名冲突测试继续通过。
- **范围：** 当前是几何法线调试图，三棱镜仍视为不透明物体。下一阶段可加入基础光照，再接入玻璃材质和路径追踪。

### V0.4 阶段 4 — 材质 JSON 与命中关联

- [x] 接入 Lambertian / Dielectric 材质数据，以独立 `MaterialConfig` 保存普通数值、数组及材质类型；运行时 trait 重构见阶段 5。
- [x] 每个物体的 JSON 加入 `material`：默认三棱镜为 `dielectric`、`ior = 1.52`，地面为 `lambertian`、线性 RGB `albedo = [0.8, 0.8, 0.8]`；旧 JSON 省略材质时按物体类型补默认值。
- [x] 校验有限且位于 `[0, 1]` 的反射率、正有限折射率、材质类型和字段；拒绝非有限数值写入或覆盖配置文件。
- [x] `Scene::from_config` 建立运行时材质和物体关联，`Scene::intersect` 填写 `Hit.material_id`，通过 `Scene::material(id)` 查询；原有仅添加几何的 `add` 接口保持未绑定材质的行为。
- **验收：** 两种材质的 JSON/文件往返、缺省材质、非法字段/参数与非有限值保护通过；改变物体添加顺序后，最近交点仍关联正确材质，非法材质编号安全返回 `None`。
- **范围：** 本阶段只接入材质数据和命中关联。法线调试图与已有焦散计算保持不变，材质的 `albedo` / `ior` 尚未参与着色、散射采样或原有光源焦散的折射率计算；下一阶段可实现基础材质着色。

### V0.4 阶段 5 — Material trait 与 BSDF 数值接口

- [x] 按用户指定结构将运行时材质从 `geometry` 拆到 `material/`：`mod.rs` 定义 `Material: Send + Sync`，两个具体材质各自实现 trait；JSON 的 `MaterialConfig` 保留 enum。
- [x] `Material::evaluate(wi, wo, normal, mode) -> BsdfValue` 统一 BSDF 求值：`EvaluateMode::Rgb` 返回 `BsdfValue::Rgb(Vector3<f64>)`，`EvaluateMode::Spectral { wavelength_nm }` 返回 `BsdfValue::Spectral(f64)`，波长单位 nm。方向都从交点向外指，归一化并检查非法数值；不包含入射辐亮度、余弦项或 PDF。按用户要求将原有 RGB/光谱两个方法合并，并保持 trait object 可用。
- [x] 光谱模式下 Lambertian 用 PBRT/Smits 反射基谱重建 `rho(λ)`，只返回 `rho(λ) / π`；CIE 匹配函数、光源和积分权重由积分器处理，理想玻璃普通求值返回所选模式的零值。RGB→光谱是近似，数据来源、许可证、插值/端点延拓与能量裁剪见 `assets/color/README.md`。
- [x] `render::utils::linear_srgb_to_xyz` 使用 W3C D65 矩阵转换线性 sRGB，保留显示范围外值并拒绝非法输入/溢出；与标量光谱 BSDF 区分。
- [x] `material::to_world(local, normal)` 以法线为局部 +Z 构造右手正交基，将局部方向变换到世界空间，保留向量长度；拒绝非法法线、非有限分量与计算溢出。
- [x] 单面 Lambertian 在法线正半球内返回 `albedo / π`；背面、切线及非法方向返回零。理想 Dielectric 的普通求值返回零，通过 `is_delta()` 和 `ior()` 表达后续采样所需的信息。
- [x] `SceneObject` 绑定几何和可选材质编号；场景保存 `Box<dyn Material>`，支持 `add_with_material` 添加自定义材质，最近交点仍通过 `Hit.material_id` 查询。
- [x] 将现有相机法线调试遍历和对应测试统一移到 `render/camera_debug.rs`，保留原 `render::render_camera_debug` 调用入口；`render/integrator.rs` 专用于后续路径追踪。
- **验收（已通过）：** BSDF 数值、半球/法线约定、方向归一化、非法输入与漫反射能量积分正确；JSON 构建和物体顺序关联、不同物体/材质编号、自定义 trait 实现通过。141 项单元测试、10 项集成测试、`cargo fmt --check`、严格 Clippy 和 diff 检查通过；默认 release 输出的八张 512×512 PNG 与重构前逐字节相同。
- **光谱接口追加验收：** 148 项单元测试和 10 项集成测试通过；覆盖基谱锚点/插值/端点延拓、六种通道排序、红蓝材质波长响应、标量 `rho(λ) / π` 数值与半球能量积分、非法输入和线性 sRGB↔XYZ 往返检查。光谱求值不受 CIE 表范围限制；材质自定义实现与 trait object 分派的光谱结果保持标量。`cargo fmt --check`、严格 Clippy 和 diff 检查通过。现有渲染仍未调用材质 BSDF。
- **范围：** 只完成材质接口和组织，不接入新的光照积分、Fresnel 采样或路径追踪。理想玻璃的 delta 贡献需要后续的离散事件采样；现有光源焦散继续使用固定值/Cauchy 折射率。
- **局部方向转换验收：** 151 项单元测试和 10 项集成测试通过；新增测试覆盖局部 +Z 与世界法线对齐、法线缩放、右手性、正交单位基、向量长度与半球方向保持、接近坐标轴的法线、零局部向量、非法输入与溢出。格式、严格 Clippy 和 diff 检查通过。
- **统一求值模式验收：** 155 项单元测试和 10 项集成测试通过；RGB/光谱模式共用方向校验，返回变体与所选模式对应，保留原有 BSDF 数值和非法输入的零值行为。JSON 材质及自定义材质通过同一 trait object 选择模式，采样的 RGB/光谱权重分别恢复反射率。`cargo fmt --check`、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 diff 检查通过。

### V0.4 阶段 6 — Lambertian 方向采样与 PDF

- [x] `Material` 新增 `sample(wo, normal, rng) -> Option<Vector3<f64>>` 和 `pdf(wi, wo, normal) -> f64`，保持 trait object 可用；默认未实现采样的材质返回 `None` / `0.0`。
- [x] 已知 `wo` 与生成的 `wi` 都从交点向外指，使用世界空间单位方向。Lambertian 在局部 +Z 半球进行余弦加权采样，再通过 `to_world` 转到世界空间。
- [x] 配套 PDF 按立体角计算：`max(n · wi, 0) / π`；背面/切线 `wo`、采样范围外方向及非法输入返回零。非法采样输入不消耗随机数。
- [x] RNG 由调用方持有和设置种子；固定种子可复现，采样分布与 albedo 无关。验证 PDF 归一化、分布矩、单位长度、正半球、`f_r cosθ / pdf` 恢复反射率以及 JSON 材质经 trait object 分派。
- **验收：** 155 项单元测试、10 项集成测试、`cargo fmt --check`、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 diff 检查通过。上传前运行 `cargo run --release --locked`，八张默认 512×512 PNG 生成成功，全部十张已保存 PNG 的 SHA-256 与运行前一致。
- **范围：** 按用户选择仅实现 Lambertian 连续方向采样；Dielectric 仍为 delta 材质，`sample = None`、普通方向 `pdf = 0.0`，后续再实现 Fresnel 离散事件采样。现有焦散追踪、相机法线图和光照积分保持当前阶段的行为。

### V0.4 阶段 7 — 相机单次直接光照

- [x] 在 `render/integrator.rs` 中实现并导出 `render_camera_lit(scene, width, height) -> Result<Framebuffer, _>`：每像素生成中心主光线，最近求交后按 `Hit.material_id` 查询材质。
- [x] 通过统一接口 `material.evaluate(wi, wo, normal, EvaluateMode::Rgb)` 读取 RGB BSDF；`wi` 指向光源，`wo` 指向相机，归一化法线并保留几何朝向，计算 `L_o = f_r L_light max(n · wi, 0)`。
- [x] 复用 `linear_srgb_to_xyz` 和 `framebuffer.set` 写入未曝光的线性 XYZ；未命中使用配置的线性 RGB 背景，未绑定材质与理想 delta 玻璃命中为黑色。
- [x] `main` 新增主图同目录的 `camera_lit.png`，默认路径为 `output/camera_lit.png`，复用 `Render::save` 施加曝光和 sRGB 编码；与主图同名时使用 `debug_camera_lit.png`，加入默认输出示例。
- **光源约定：** 以 `total_flux / 水平孔径面积` 作为白色平行光的相对方向光强度（默认 `0.5`）。本阶段不校准绝对辐射度，也不进行斜射孔径投影校正、有限孔径覆盖裁切或阴影查询。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked`（159 项单元测试、10 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 diff 检查通过。测试覆盖材质/光强/一个余弦项、法线朝向、背景和最近命中、非法输入、PNG 尺寸/曝光/sRGB 及文件名冲突。`cargo run --release --locked` 实际输出九张默认图，新增 `camera_lit.png` 为 512×512 RGB8，已查看其黑色玻璃轮廓与灰色地面；运行前已有的十张 PNG 的 SHA-256 全部保持一致。
- **范围：** 仅完成一次确定的 RGB 直接光照，不使用方向采样/PDF，不实现递归反射/折射、间接光或焦散叠加。默认玻璃仍为黑色；后续可先加阴影查询，再实现玻璃采样。

### V0.4 阶段 8 — Shadow ray 与硬阴影

- [x] 在 `render_camera_lit` 的非零直接光照路径中，从 `hit.coords + normal × 1e-5 m` 沿 `wi` 发射 shadow ray，用 `Scene::intersect` 判断遮挡。
- [x] 按 `L_o = V f_r L_light max(n · wi, 0)` 计算，有遮挡时 `V = 0`，通畅时保留原有直接光照。零贡献处跳过阴影查询，主光线未命中的背景继续按原值输出。
- [x] 将光源视为无穷远方向光，查询所有物体及同一物体的其他表面；玻璃和未绑定材质的几何也作为遮挡物，不引入透射率计算或新的光源参数。
- [x] 更新 `output/camera_lit.png` 示例及 README，补充玻璃下方的地面阴影、未遮挡地面、自相交/反向遮挡、物体顺序与无穷远方向光测试，并在实际程序的 PNG 读回中验证阴影。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（161 项单元测试、10 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 diff 检查通过。`cargo run --release --locked` 生成 512×512 RGB8 的 `camera_lit.png`，已查看到三棱镜下方的地面硬阴影。运行前十一张 PNG 中仅该相机光照图发生变化，其余十张 SHA-256 完全一致。
- **范围：** 只增加二值硬阴影；不实现玻璃反射/透射、间接光、有限孔径裁切或焦散叠加。固定起点偏移可能漏掉偏移距离内的极近遮挡。下一阶段可实现玻璃 Fresnel 离散事件采样。
- **Ray 接口精简：** 当前为静态场景，按用户要求删除未参与求交或光照计算的 `Ray.time`；所有相机、光源、折射、shadow ray、示例与测试统一使用 `Ray::new(origin, direction)`。保留方向归一化、倒数缓存与原有距离区间，未增加新的渲染阶段。
- **接口精简验收：** 160 项单元测试、10 项集成测试、`cargo fmt --check`、严格 Clippy 与 diff 检查通过。删除重复的时间位置测试，将时间合法性测试替换为起点/方向的有限值、零向量、溢出与下溢检查。实际运行默认 release 渲染和 `refract_ray` 示例成功，十一张已有 PNG 的 SHA-256 均与精简前一致。

### V0.4 阶段 9 — 玻璃 Fresnel 离散事件采样

- [x] 在 `material/dielectric.rs` 定义 `GlassEvent::{Reflection, Transmission}` 和 `GlassSample { wi, event, probability, weight }`，从 `material` 重导出。
- [x] `Dielectric::sample` 按精确 Fresnel 反射率 `F` 与 `1-F` 选择事件；全反射强制反射，相同折射率直接透射，确定事件不消耗随机数。
- [x] `wo`、`wi` 都从交点向外指；使用朝外几何法线判断进入/离开，切换 `(eta_i, eta_t)` 并朝向入射侧；折射调用 `optics::refract(-wo, n, eta_i, eta_t)`。归一化方向并拒绝非法、切线及无效计算。
- [x] `probability` 保存离散事件概率；`weight` 为已抵消 Fresnel、余弦与事件概率的 radiance 吞吐量乘数，反射为 `1`，透射为 `(eta_i/eta_t)²`。理想 delta 的普通 `evaluate`、`pdf` 仍为零。
- [x] 按用户选择将 `Material::sample` 统一为 `Option<MaterialSample>`，替换阶段 6 的方向返回值。Lambertian 返回 `Continuous { wi }`，Dielectric 返回 `Glass(GlassSample)`；两者可通过 `wi()` 读取方向，支持 `dyn Material` 分派，默认仍返回 `None`。
- **验证范围：** 内外两侧的法向/斜向方向、Snell 定律、世界空间法线、全反射、相同折射率、非法输入与 RNG 消耗、固定种子可复现、事件频率、重要性采样估计及 JSON 构建后的 trait object 分派。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（172 项单元测试、10 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 diff 检查通过。
- **发布前复查：** 默认 `cargo run --release --locked` 和 `refract_ray` 示例运行成功；十一张已有 PNG 均为 512×512 RGB8，重新生成后的 SHA-256 与运行前完全一致。
- **范围：** 仅实现空气与常量折射率玻璃之间的采样器，不接入递归相机追踪、光源光子通量权重、嵌套介质、吸收或光谱折射率。现有阴影规则和 PNG 输出流程保留；下一阶段可让相机积分器消费 `GlassSample`。

### V0.4 阶段 10 — 相机玻璃路径，最多八次散射

- [x] 在 `render/integrator.rs` 新增并导出 `render_camera_glass(scene, width, height, seed) -> Result<Framebuffer, Box<dyn Error>>`；每像素一条中心射线，固定种子可复现。
- [x] 命中玻璃后通过 `Material::sample` 获取 `GlassSample`，以 `wo = -ray.direction` 和朝外几何法线采样；只乘 `sample.weight`，按新方向所在侧偏移起点，再继续求交。
- [x] 最多采样八次玻璃反射/透射，第八次后允许命中非 delta 表面或背景并累加贡献，第九次玻璃命中处截断。非 delta 命中使用现有直接光照和硬阴影，未命中使用背景，未绑定材质/采样失败时终止。
- [x] 两个相机入口共享尺寸/方向光校验、直接光照与阴影逻辑；新增独立 `camera_glass` 示例，复用 JSON、曝光与 sRGB PNG 输出。
- **验证范围：** 无玻璃时与原直接光照一致、内外侧反射/透射与全反射、双界面权重抵消和自相交、地面直接光照/玻璃硬阴影、八次与九次界面边界、固定种子可复现、非法采样与吞吐量溢出。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（178 项单元测试、10 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 diff 检查通过。玻璃示例实际生成 512×512 RGB8 的 `output/camera_glass.png`，已查看单路径采样结果；默认 release 渲染也运行成功，十一张原有 PNG 的 SHA-256 与改动前完全一致。
- **范围：** 每像素一个随机玻璃路径样本，RGB 常量折射率，有限深度截断；不增加漫反射间接光、透明 shadow ray、光谱色散、环境光照或焦散叠加。下一阶段可先增加相机每像素采样数，降低反射/透射噪点。

### V0.4 阶段 11 — 相机 SPP 与单路径函数

- [x] 将 `render_camera_glass` 的路径循环提取为内部 `trace_camera_path`：只追踪一条 Ray，返回未曝光的线性 RGB，每次重置吞吐量/辐亮度并保留八次散射限制。
- [x] `render_camera_glass(scene, width, height, spp, seed)` 每像素追踪 `spp` 条路径，在线性 RGB 中按总样本数平均后转 XYZ；SPP=0 和总样本数量溢出返回错误，零贡献路径也计入分母。
- [x] 保留像素中心主光线，独立采样玻璃事件；整次渲染只初始化一次 RNG，不逐样本重设种子，SPP=1 保持原图与原随机序列。
- [x] 新增 `render.camera_spp`，默认 64，旧 JSON 缺省时兼容默认值；与光源 `samples_per_pixel_axis` 独立。示例读取此配置并支持可选的命令行 SPP 覆盖。
- **验证范围：** 多样本权重平均和随机序列、固定种子重现、SPP 增加时的亮度归一化、相对 Fresnel 解析期望的误差下降、非法 SPP/溢出、JSON 缺省/往返，以及相机 SPP 不影响光源焦散。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（182 项单元测试、11 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 diff 检查通过。实际运行 1 SPP 示例，512×512 PNG 与重构前逐字节一致；默认 64 SPP 示例已更新 `output/camera_glass.png`，已查看噪声降低后的结果。默认 release 渲染成功，其余十一张已有 PNG 的 SHA-256 保持一致。
- **范围：** 只增加玻璃路径的每像素样本数，不引入子像素抖动、并行相机渲染、漫反射间接光或透明阴影；八次截断仍会丢弃更长路径。下一阶段可加入像素内采样，改善几何边缘的锯齿。

### V0.4 阶段 12 — 像素内随机抖动

- [x] 新增 `Camera::ray_for_pixel_sample(..., offset: Vector2<f64>)`，接收两轴 `[0,1)` 的像素内偏移，保留 FOV、宽高比、相机位置与左上原点/y 向下约定；非法偏移返回 `None`。
- [x] `ray_for_pixel` 复用中心偏移 `(0.5,0.5)`，原有直接光照和法线调试图继续使用中心采样。
- [x] 玻璃相机渲染在每个 SPP 样本开始时均匀生成 `u、v ∈ [0,1)`，从像素内 `(x+u,y+v)` 发射射线，再采样玻璃路径；SPP=1 也使用抖动，固定种子可复现。
- [x] 保留线性 RGB 的 `1/spp` 平均与八次玻璃散射上限；将单条已知方向的光学测试放到 `trace_camera_path`，像素渲染测试验证随机位置和面积平均。
- **验证范围：** 子像素与中心坐标、已知投影位置和非方形宽高比、非法偏移/越界、黑白各半像素的线性覆盖率、多样本归一化、固定种子重现、玻璃反射/透射及光源渲染独立性。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（185 项单元测试、11 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 diff 检查通过。实际运行 64 SPP 玻璃示例并查看了更新的 512×512 RGB8 PNG；默认 release 渲染成功，其余十一张 PNG 与添加抖动前逐字节一致。
- **默认背景更新：** 按用户设置将线性 RGB 背景调整为 `[0.08, 0.10, 0.14]`，默认 JSON、`SceneConfig::default` 和 `Scene::new` 保持一致。现有 185 项单元测试、11 项集成测试、格式和严格 Clippy 通过；已重新生成 `camera_glass.png`、`camera_lit.png`，其余十张 PNG 的 SHA-256 保持一致。
- **范围：** 只做像素区域内的独立均匀采样，不增加镜头景深、运动模糊、分层采样、间接光或透明阴影。有限 SPP 的边缘仍有随机噪声；下一阶段可使用像素内分层采样降低方差。

### V0.4 阶段 13 — 地面光谱辐照度接入相机画面

- [x] `sample_ground_irradiance` 将世界交点映射到光源地面图的连续像素坐标，按原有 y 翻转/像素中心约定双线性读取 XYZ 辐照度；接收区域外为零，边缘夹紧，拒绝非法数据。
- [x] `lambertian_radiance(E_xyz, rho)` 使用 `L_o = E rho / π`，先支持波长无关的中性灰反射率；默认 `rho=0.8`，保持色度和 HDR，不对已积分的 E 再乘光照或余弦。
- [x] 新增 `render_camera_glass_with_ground`，在共用 `trace_camera_path` 中按 `Scene.ground_id` 识别地面并使用地面图替换原直接光照/硬阴影；保留原玻璃 Fresnel 路径、八次限制、SPP、抖动及旧相机入口。
- [x] 新增 `camera_glass_with_ground` example：先由 `Render::render` 生成光谱地面图，将未曝光的内存 Framebuffer 传给新相机入口，最后保存 `output/camera_glass_with_ground.png`；可选 SPP 参数沿用现有配置。
- **验证范围：** 已知世界位置与非方形图像的双线性插值、接收区边界/单像素图、非法 XYZ/元数据、中性灰 `rho/π`、HDR 与色度保持、避免重复直接光和硬阴影、玻璃吞吐量、背面/其他物体、地面图与相机分辨率独立、完整光谱→相机→PNG 流程及照明缩放线性。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（191 项单元测试、12 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 diff 检查通过。新 example 实际生成并查看了 512×512 RGB8 的 `camera_glass_with_ground.png`；原玻璃 example 与默认 release 主程序重新运行成功，十二张原有 PNG 的 SHA-256 均保持一致。
- **范围：** 只接入水平中性灰 Lambertian 地面；接收区外为零，彩色地面需后续保留波长信息。光源通道仍沿用双界面模型并忽略 Fresnel 损失/吸收，相机仍使用常量玻璃折射率；未增加间接光或通用 Photon Mapping。下一阶段可保留按波长的地面照明来支持彩色材质。

### V0.4 阶段 14 — 统一太阳光标尺，分离地面直射与折射焦散

- [x] 新增 `SpectralLighting`、`render_spectral_lighting` 和 `Render::render_lighting`：同一批分层光源样本分别累积 Direct（首次命中地面）和 Caustics（经棱镜双界面后落地）。首次求交、发射随机序列和光路继续复用，不重复追踪全部光子。
- [x] 保留 `render_spectral_caustics`、`Render::render` 的单缓冲、原累积顺序及旧输出；拆分结果的 `combined()` 可在线性 XYZ 中恢复旧地面图，允许浮点求和次序带来的舍入误差。
- [x] 明确 `total_flux = Phi` 为穿过水平发射窗口的相对通量。窗口面积为 `A`，传播单位方向为 `d`，则水平面未遮挡辐照度为 `Phi/A`，光束法向强度为 `Phi/(A*|d.z|)`。`ParallelLight::sunlight` 进行投影换算；新入口要求向下且非近水平的光源方向。太阳视作无限远，窗口不裁切解析直射。
- [x] 解析太阳 XYZ 和光子使用同一批波长节点、D65 SPD、归一化 `k` 和相对通量标尺；不从有限接收图估计太阳强度，不按接收面积改变解析直射亮度。
- [x] 新增 `render_camera_glass_with_lighting`，地面使用 `L_o = (E_direct + E_caustics)*rho/pi`。解析直射保留完整场景 Shadow Ray，玻璃也遮挡；焦散专用图已包含折射路径可见性，不再做二值阴影判断。receiver 只限定焦散查询，范围之外的普通地面仍有太阳直射，不增加边界模糊或玻璃遮挡例外。
- [x] 保留中性灰 Lambertian 校验、Fresnel 相机路径、八次散射、SPP、固定种子和子像素抖动。旧 RGB 相机入口及阶段 13 完整地面图入口保留历史行为；阶段 14 新入口采用统一的太阳投影/光谱约定。
- [x] 新增 `camera_direct_caustics` example，一次光源追踪后，以相同相机种子分别生成 `camera_direct.png`、`camera_caustics.png`、`camera_combined.png`。Direct 含原背景，Caustics 背景为零；三者满足线性辐亮度的 Direct + Caustics = Combined，不能直接对 sRGB PNG 字节相加。
- **验证范围：** 无棱镜时 receiver 内外/发射窗口外地面直射连续、只含直射时不重复计数、拆分合并恢复两种折射率模式的旧图、求交计数未增加、斜射时光子总通量与解析直射一致、不同接收范围不改变相同世界位置的太阳直射、玻璃硬阴影与真实棱镜焦散可独立检查，以及完整 Fresnel 相机→三分量 PNG 流程和非法输入。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（199 项单元测试、14 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 及 `git diff --check` 通过。新 example 已实际生成并查看三张 512×512 RGB8、64 SPP 调试图；默认 release 主程序、旧 `camera_glass` / `camera_glass_with_ground` examples 运行成功，十三张原有 PNG 的 SHA-256 均与改动前一致。
- **限制：** 相对通量尚无绝对辐射单位标定；有限发射窗口须覆盖能照亮棱镜的入射区域，焦散仍受接收范围裁切、分辨率和采样噪声影响。仅支持水平中性灰地面。光源通道仍为双界面折射，TIR 路径丢弃，**尚无 Fresnel 通量修正或吸收**；相机玻璃仍为 RGB 常量折射率并在八次后截断。背景只对未命中相机射线可见，**不提供环境照明或漫反射间接光**，无直射也无焦散的地面保持黑色。
- **下一阶段：** 阶段 15 的 Fresnel 光子通量修正留待单独任务，本次不实现。

### V0.4 阶段 15 — 正向光子双透射 Fresnel 通量修正

- [x] 新增 `scene::PhotonLanding { position, weight }` 和 `take_photon_landing`，内部 `take_photon_from_hit` 返回带权重落点。原 `take_photon` 参数与坐标返回接口保留，委托新接口后只取 position。
- [x] 入射计算 `F_enter = fresnel_dielectric(-d_air·n_enter, 1, n_lambda)`，出射计算 `F_exit = fresnel_dielectric(d_inside·n_exit, n_lambda, 1)`，确定双透射路径的通量权重为 `(1-F_enter)*(1-F_exit)`。不调用相机事件采样，不除透射概率、不乘 radiance 折射率平方项。
- [x] `Constant` 每条玻璃光路只追踪/计算一次权重；`Dispersion` 对每个波长的折射率和实际内部方向分别计算。首次交点复用、空间采样、固定随机序列、光谱积分、分块合并保持原有流程。
- [x] 光谱单图、Direct/Caustics 拆分及灰度能量对照均在落地累积时乘一次通量权重；直射权重保持 1。丢弃的反射、TIR、遮挡或 receiver 外通量不重新归一化，完整 D65 归一化和解析太阳标尺不变。
- [x] 保留相机 Fresnel 路径、八次限制、SPP、抖动、Shadow Ray 和 receiver 外解析直射；仍只先支持水平中性灰地面。
- **验证范围：** 直射/相同折射率权重 1、`n=1.5` 正入射双透射 `0.9216`、60°/80° 斜入射的独立 Fresnel 参考值、实际棱镜两面不同的入射余弦与权重、逐波长权重差异、TIR/三段遮挡、旧坐标接口和已知落点兼容、加权光谱与独立逐波长追踪一致、仅折射通量减少且不重新分配，以及阶段 14 的拆分、相机和并行回归。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（204 项单元测试、14 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 `git diff --check` 通过。默认 release 主程序、`camera_direct_caustics`、`camera_glass_with_ground`、`camera_glass` examples 实际运行成功；已查看更新的 512×512 RGB8 Combined。含折射能量的九张 PNG 已更新，Direct、纯相机玻璃、法线/直射调试和无棱镜等七张 PNG 的 SHA-256 与阶段 14 完全一致。
- **范围与限制：** 本次只追踪确定的透射→透射光路，反射通量被丢弃，不追踪入口反射、内部反射续追、更长光子路径或吸收；不引入环境照明。含折射能量的图像应比阶段 14 的无损基线变暗，直射及只追踪相机的输出应保持一致。
- **下一阶段：** 是否继续光子反射路径另行确定，本次不自动扩展。

### V0.4 阶段 16 — 均匀天空照明与渐变背景

- [x] `Scene.environment` / `Environment` 保存天空入射辐亮度及地平线/天顶颜色，JSON `scene.environment` 支持读写、HDR 有限非负校验；省略/null 兼容旧常量背景且关闭天空。
- [x] 对当前 Lambertian 材质解析积分均匀上半球天空：`E_sky = pi * L_sky * (1+n.z)/2`，朝上地面 `L_environment = rho * L_sky`。反射率仍由已有材质 BSDF 提供，不引入新材质模型。
- [x] 四个相机照明入口的漫反射终点单独加一次环境辐亮度，并乘原玻璃吞吐量。环境中不包含太阳；保留解析太阳/地面图原有互斥分支，不叠加 sampled Direct，不改变光源追踪或焦散通量。
- [x] 未命中时按最终射线的世界 z 方向查询地平线→天顶渐变，包含玻璃反射/透射后的背景。渐变仅用于可见背景，照明仍采用独立均匀天空。
- [x] 天空照明及背景归入 Direct，Caustics 保持纯焦散；不增加 RNG 消耗，保留相同路径样本下 `Direct + Caustics = Combined`。
- **验证范围：** 朝上/侧向/朝下法线的天空积分、彩色材质反射率、单面/Delta、阴影补光、玻璃吞吐量、最终射线背景、HDR/非法参数、旧 JSON、四个相机入口的单次补光、太阳标尺和光子输出不受天空影响，以及含天空的分量相加。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（210 项单元测试、15 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 `git diff --check` 通过。release 主程序及三个相机 examples 实际运行成功，已查看默认 512×512、64 SPP 的 Direct/Combined；16 张已有 PNG 均为 512×512 RGB8，仅五张含天空的相机照明图改变，纯 Caustics、光源输出及法线调试等其余十一张逐字节一致。
- **范围与限制：** 天空采用无遮挡解析近似，不追踪环境遮挡、AO、玻璃引起的天空焦散或漫反射间接反弹；渐变背景与均匀照明为独立的艺术化参数，没有物理大气或光谱天空。下一阶段可单独增加天空采样与可见性测试。

### V0.4 阶段 17 — 地面棋盘格与斜射光构图

- [x] 按用户参数更新默认配置及 Rust 默认值：光源传播方向 `[0.45,0,-1]`，相机位置 `[2.5,-4.5,2.6]`、目标 `[0,0,0.9]`、up `[0,0,1]`、FOV `35°`；运行时方向归一化。
- [x] `scene.ground_checkerboard` 支持每格米制尺寸及两种 `[0,1]` 内的反射率乘数，缺省/null 为纯色地面；默认每格 `0.5 m`，材质 `0.8` 对应亮格 `0.8`、暗格 `0.2`。
- [x] 程序化棋盘格采用世界 x/y 坐标和 floor，正确处理原点两侧、格子边界、非法参数和数值溢出；只调制绑定接收地面的漫反射率。
- [x] 四个相机照明入口在漫反射终点乘一次纹理系数，统一作用于太阳、天空和焦散，保留玻璃吞吐量及 `Direct + Caustics = Combined`；不改变光源入射辐照度图。
- **验证范围：** 负坐标与格子边界、仅绑定地面受纹理影响、JSON 缺省/往返/非法参数、所有相机入口只调制一次、玻璃路径上的天空/焦散权重、光子输出独立性，以及新的默认构图实际图片。历史垂直光参考测试显式指定垂直光，不依赖展示场景的新默认值。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（214 项单元测试、16 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 及 `git diff --check` 通过。release 主程序和三个相机 examples 实际运行成功，已查看新构图下 512×512、64 SPP 的 Direct/Combined；默认输出图已更新。
- **范围与限制：** 仅加入程序化灰度反射率纹理；不加载图片或改变法线，不做纹理足迹过滤，远处格子仍可能混叠。下一阶段可单独增加纹理过滤。

### V0.4 阶段 18 — 有限地面，略大于 receiver

- [x] `Ground::with_bounds` 在水平面交点处按世界 x/y 裁切闭矩形，保留双面求交、+z 法线、epsilon 与非法数值保护；几何示例/测试的 `Ground::new` 无限平面接口保留。
- [x] `scene.ground_margin_m` 默认 `0.5 m`，配置场景地面按 receiver 每边外扩此余量构建；修改 receiver 配置并重建场景时同步更新。拒绝非正/非有限余量、范围溢出及浮点精度无法表示的外扩。
- [x] 默认 receiver 仍为 `[-2,2] × [-2,2] m`，实际地面为 `[-2.5,2.5] × [-2.5,2.5] m`。receiver 外、地面内保留太阳/天空/棋盘格；地面外相机和玻璃路径正常返回背景。
- [x] 不修改 receiver 的记录范围、光子权重、色彩积分或相机光照分量；有限地面完整包含 receiver，区内光子能量图与无限地面对照一致。
- **验证范围：** 矩形四边/角点、正反两侧、斜射的实际交点裁切、非法边界与溢出、配置缺省/往返、非对称 receiver 外扩、地面外天空、receiver 外的直射和天空、两种折射率模式/拆分光子图与无限平面对照，以及默认相机实际渲染。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（218 项单元测试、18 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 与 `git diff --check` 通过。release 主程序及三个相机 examples 实际运行成功，已查看 512×512、64 SPP 的有限地面 Combined；纯 Caustics 与所有光源输出保持逐字节一致。
- **范围与限制：** 地面仍是无厚度水平矩形，没有侧面、倒角或边框；下一阶段可单独增加厚度或边框。

### V0.4 阶段 19 — 统一方向环境与 Monte Carlo 照明

- [x] `Environment` 只保留地平线色/天顶色；`radiance(direction)` 统一方向归一化、z 渐变、HDR/非法数值保护。背景和环境积分使用同一函数，删除独立 `sky_radiance` 配置及旧解析均匀上半球近似。
- [x] 复用 `Material::sample` / `pdf` 的 Lambertian 余弦加权半球采样，累加 `f_r * L_env(wi) * V * cos/pdf`；Lambertian 时恢复 `rho * L_env(wi) * V`。沿法线偏移并查询完整场景，被遮挡样本记零且仍计入总分母。
- [x] `scene.environment_samples` 默认 1，控制每条相机路径的环境方向数，配合 SPP 平均；校验非零及总样本数溢出。环境 RNG 与像素/玻璃 RNG 分离，保持相同玻璃路径下 Direct + Caustics = Combined，以及固定种子可复现。
- [x] 四个相机照明入口共用估计器；保留玻璃吞吐量、棋盘格单次调制、有限地面、解析太阳/焦散互斥分支。环境不含太阳，也不写入光子能量图。
- **验证范围：** 背景与积分查询统一、全方向均匀环境 `rho*L`、朝上渐变解析参考 `rho*(horizon/3+2*zenith/3)`、样本数增加时 MSE 下降、半遮挡/全遮挡与分母、自遮挡偏移、HDR/非法参数、配置迁移、相机随机流独立性、分量相加及光子输出独立性。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（221 项单元测试、18 项集成测试）、严格 `cargo clippy --locked --all-targets -- -D warnings` 及 `git diff --check` 通过。release 主程序和三个相机 examples 实际运行成功，已查看 512×512、64 SPP 的 Combined 与单路径直接光调试图；16 张已有 PNG 均为 RGB8，仅五张含环境照明的相机图变化，纯 Caustics、光子图和法线调试等十一张逐字节不变。
- **范围与限制：** 环境遮挡为二值，玻璃也遮挡；不追踪透明环境阴影、环境透射焦散或漫反射间接反弹。默认每路径 1 个环境样本，有 Monte Carlo 噪声；下一阶段可单独改善方差或实现玻璃环境透射。

### V0.4 阶段 20 — 默认棱镜保留 3 cm 离地间隙

- [x] 以用户指定的 3 cm 间隙为目标，将默认场景六顶点整体下移 `0.97 m`（97 cm）：底面 `z=0.03 m`，顶棱 `z=0.83 m`，棱镜形状和尺寸保持不变。配置坐标单位为米。
- [x] 同步 JSON 与 `SceneConfig::default`，更新依赖旧棱镜高度的法线/阴影/求交测试参考位置；`Prism::new` 几何示例保留原坐标。
- **验收：** 六顶点位移、3 cm 离地间隙及 0.8 m 截面高度核对通过；`cargo fmt`、`cargo fmt --check`、221 项单元测试、18 项集成测试、严格 `cargo clippy --locked --all-targets -- -D warnings` 和 `git diff --check` 通过。release 主程序及三个相机 examples 实际运行成功，已查看更新的 512×512、64 SPP Combined。
- **范围与限制：** 本次仅调整配置场景位置；相机、光源、采样参数和光照模型保持原设置。下一步可按新的棱镜高度单独调整相机构图。

### V0.4 阶段 21 — 相机对准降低后的棱镜

- [x] 按用户参数将默认相机目标调整为 `[0,0,0.4]`，位置 `[2.5,-4.5,2.6]`、up `[0,0,1]` 与 FOV `35°`；同步 JSON、Rust 默认值与文档。
- **验收：** `cargo fmt`、`cargo fmt --check`、221 项单元测试、18 项集成测试、严格 Clippy 与 `git diff --check` 通过。release 主程序和三个相机 examples 运行成功，已查看更新的 512×512、64 SPP Combined；七张相机图更新，其余九张光源/历史图逐字节不变。
- **范围与限制：** 仅调整相机朝向并更新相机示例图，光源地面图不受影响；仍使用针孔相机，后续可单独调整构图或景深。

### HDR 阶段 B — 将 Environment Map 接入 Scene

- [x] `EnvironmentConfig` 增加可缺省的 `map_path`、`rotation_degrees`、`intensity`，默认分别为 `None`、`0`、`1`；保留渐变颜色及旧 JSON，验证有限旋转角与有限非负强度。
- [x] 将运行时 `Environment` 放入 `environment` 模块并保留 `scene::Environment` 重导出；独占可选 `EnvironmentMap`，移除 `Copy/Clone`。`Scene::from_config` 有路径时加载一次 HDR，文件错误向上传递，查询只访问缓存像素。
- [x] HDR 优先于渐变；使用 `Rz(-rotation)` 将世界方向转入贴图坐标，正角度表示环境绕世界 +Z 旋转。采样后乘线性强度，保留 HDR 并拒绝溢出；没有贴图时保留原渐变公式。
- [x] 相机背景、玻璃反射/透射背景及 Lambertian 环境照明通过原 `radiance(direction)` 自动使用 HDR；积分公式、太阳与光子焦散实现保持不变。README 说明相对路径以程序启动工作目录为基准。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked`（240 项单元测试、29 项集成测试）、`cargo clippy --locked --all-targets -- -D warnings` 和 `git diff --check` 通过。本阶段新增 2 项配置测试和 11 项合成 HDR 集成测试，覆盖兼容/往返、数据源优先级、旋转/强度、非法输入/溢出、文件错误、删除源文件后的查询、背景、真实玻璃双事件、四个相机照明入口与太阳/光子回归。
- **范围与限制：** 仅完成阶段 B；不实现 HDRI 重要性采样、MIS、透明环境阴影或环境光子追踪。环境仍用材质余弦采样；下一阶段可单独评估 HDRI 重要性采样，本次不提前实现。

### 相机三图合并 — 单次路径生成 Direct / Caustics / Combined

- [x] 新增 `CameraLightingImages` 与 `render_camera_glass_components`；单次像素/SPP 遍历调用一次路径追踪，同时累计两个线性 RGB 分量，平均后相加生成 Combined，再分别转换为未曝光的 XYZ。
- [x] 地面解析太阳及其阴影与焦散贴图查询分开计算，分别应用一次 `rho/pi`、棋盘格和玻璃吞吐量。背景、HDRI、环境照明及非地面表面的太阳照明仅进入 Direct；Caustics 仅保存焦散图的地面反射。
- [x] 保留原单图接口及按分量跳过光照查询的行为，单图入口仍只分配一张图。示例改为一次相机渲染生成三图，保留参数和输出文件名。
- [x] 保留原 `StdRng(seed)` 与独立环境 RNG；记录测试确认子像素方向、反射/透射事件和吞吐量顺序不变，三图玻璃采样由 3N 次降为 N 次。光子、光谱、材质公式、默认配置、曝光和线程池均未修改。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked`（242 项单元测试、30 项集成测试）、严格 Clippy 和 `git diff --check` 通过。覆盖新旧三入口逐像素对比、固定种子复现、HDRI 背景/照明归属、太阳不重复计数、非地面 Lambertian、棋盘格/吞吐量及光子回归。与重构前保存的 31×21、16 SPP 浮点图相比，Direct/Caustics 最大误差为 0，Combined 最大 XYZ 分量误差约 `1.78e-15`。
- **实际运行：** release 示例在默认 512×512、256 SPP 下成功生成三张 PNG；三张图均与重构前逐字节一致。相机路径总数从 201,326,592 减至 67,108,864，减少三分之二。
- **范围与限制：** 仅合并三图路径；新接口的三张 XYZ 结果缓冲约 18 MiB，单图仍约 6 MiB。不引入相机 Rayon 并行或新的采样方案；下一阶段另行选择。

### 阶段 C — Rayon 相机像素并行

- [x] `render_camera_buffers` 使用 `par_iter_mut` 并行处理行优先像素，像素内 SPP 顺序执行，每条路径只调用一次 `trace_camera_path`；三图分支 zip 三个缓冲，单图分支只分配 primary。
- [x] 以 `pixel_seed(seed, y*width+x, stream_id)` 的 SplitMix64 风格整数混合派生每像素两个 `StdRng`，分别用于子像素/玻璃及环境方向。种子不依赖线程编号，像素内采样消耗和平均顺序保留；具体序列允许与旧全图串行 RNG 不同。
- [x] 并行闭包使用 `String` 传递错误，外层恢复公共 `Box<dyn Error>`；每个像素由一个任务独占写入，无共享 RNG 或跨线程浮点归约。
- [x] 新增 `Render::render_camera_glass_components`，使用配置尺寸及 `pool.install`；示例通过该方法使 `thread_count` 控制相机线程池，原自由函数保留。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked`（243 项单元测试、32 项集成测试）、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check` 通过。新增种子区分、1/2/4 线程及全局池的三图/重复运行完全一致、单图兼容、分量相加、实际线程池大小探针及并行错误回传测试；原玻璃事件、环境 RNG 分离和光源回归通过。
- **实际性能：** 新增 `camera_parallel_benchmark` release 示例，固定默认 HDRI/场景、512×512、256 SPP、seed=42、environment_samples=1。1/2/4/6 线程的相机三图耗时分别为 `34.741153 / 16.997655 / 8.400291 / 5.692999 s`，相机加速比为 `1.000 / 2.044 / 4.136 / 6.102×`；光源+相机总耗时为 `39.702723 / 19.531569 / 9.801827 / 6.949445 s`。所有线程数的三张相机及两张光源 XYZ 缓冲逐位一致，完整口径与结果见 README。
- **范围与限制：** 只并行相机玻璃入口共用的像素循环，不改变光源端实现、物理公式、默认 SPP/环境采样数或颜色/曝光流程。`render_camera_lit`、法线调试、灰度光源调试和 PNG 编码仍为串行。基准为本机单轮测量，不计 HDR 加载、线程池构建和 PNG 编码；后续优化另行选择。

### HDR 阶段 D1 — HDRI 重要性分布、方向采样与 PDF

- [x] `EnvironmentMap` 加载时按线性 Rec.709 亮度乘精确像素立体角构建行优先 `f64` CDF，缓存纬度边界的 cosθ；采样及查询只读，无文件 IO。
- [x] 新增 `EnvironmentSample { direction, pdf }`、`sample_direction(rng)` 和 `pdf_direction(direction)`。以 95% 概率按重要性 CDF 选像素，在像素内均匀采样 φ 与 cosθ；其余 5% 均匀采样球面。完整 PDF 为 `0.95 P_ij/Δω_ij + 0.05/(4π)`，P 从实际 CDF 区间宽度取得。
- [x] 全黑 HDRI 和没有贴图的渐变天空均使用均匀球面分布；黑像素仍有正 PDF。沿用 Z-up、−X 接缝 wrap 与极点 U=0.5，PDF 无 sinθ 除法；拒绝非法方向并稳定处理极端有限向量。
- [x] `Environment` 用正向环境旋转生成世界单位方向、逆旋转查询 PDF。保持 `radiance()`、intensity 标尺、积分器、材质、光子追踪、SPP、颜色/曝光和 Rayon/RNG 设计的现有行为。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked`（252 项单元测试、37 项集成测试）、`cargo clippy --locked --all-targets -- -D warnings` 及 `git diff --check` 通过。新增 9 项单元测试、5 项集成测试，并扩展原方向/文件缓存测试；覆盖概率归一化、单位方向、样本 PDF 一致、均匀球面矩与频率、亮区偏好/黑区支持、黑图、接缝/极点/随机数端点、单行/单列/非方形、极端 HDR、旋转/强度、固定种子及 1/2/4 线程只读共享采样。合成 HDR 的 `L_env/pdf` 估计恢复已知辐亮度积分。
- **范围与限制：** 仅完成 D1，不接入 `environment_lighting()`、不修改 `trace_camera_path()`、不实现 MIS。分布使用像素亮度且在像素内分段常数，辐亮度继续双线性插值；额外 CDF/行边界缓存为 O(W×H)，4096×2048 约 64 MiB。下一阶段接入环境积分时使用 `f_r L_env V max(n·wi,0)/pdf` 并按总样本数平均，背半球与遮挡计零且保留分母；提高采样频率不能改变 HDRI 光照强度。

### HDR 阶段 D2 — BSDF / HDRI 混合环境积分

- [x] HDRI 环境每次用独立环境 RNG 按 50% BSDF / 50% HDRI 选择一个方向，两种来源均使用 `p_mix=0.5 p_bsdf+0.5 p_env`；D1 的 95%/5% 已包含在 p_env，不重复添加，也不额外乘除选择概率。
- [x] 保留 `f_r L_env V max(n·wi,0)/p_mix` 的渲染方程及总样本数平均。接受零 BSDF PDF，完整 PDF 必须有限正数；背半球/遮挡计零且保留分母，检查 BSDF、辐亮度、贡献与累积的有限性。
- [x] 增加只读 `Environment::has_map()`；无 HDRI 时维持原 BSDF 采样和 RNG 状态。生产改动仅涉及环境采样策略，不改玻璃/Fresnel、光源焦散、Lambertian、相机/Rayon/RNG 派生、三图归属、棋盘格、太阳、可见性、SPP 或输出颜色管理。
- [x] 新增受控 PDF/旋转、合法零 PDF、非法数值、均匀/黑图、分母、渐变精确兼容和多种子高对比统计测试；实际 HDRI 相机三图在 1/2/4 线程下逐位一致，关闭环境不改变光源图与相机 Caustics。
- [x] 增加默认忽略的测试专用比较器 `compare_hdri_sampling`，显式运行 `cargo test --release --locked compare_hdri_sampling -- --ignored --nocapture`。同一 64×48 漫反射场景、HDRI、32/128 SPP、环境样本数 1、种子 0–7，记录时间、均值、线性 Y RMSE；新比较路径与生产相机 XYZ 完全相同。2048×1024 半球求积提供参考，均匀/高对比两类场景均实际运行。
- **实际结果：** 高对比 64 种子×256 方向的单点 RMSE 从 `3.70390` 降为 `1.45367`（MSE 比值 `0.1540`）。相机比较的同 SPP MSE 比值为 `0.1903–0.1952`，每方向成本约为 `1.50×`；混合 32 SPP 用 `42.035 ms` 获得低于余弦 128 SPP（`111.728 ms`）的误差。均匀 HDRI 余弦近乎零方差，混合引入噪声及约 1.5 倍成本；完整测量口径、亮度、参考值和数据表见 README。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked --quiet`（259 项单元测试通过、1 项比较默认忽略，38 项集成测试通过）、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check` 通过。忽略的比较另以 release 实际运行通过。
- **范围与限制：** 仅完成 D2 的固定混合采样，不实现 Power Heuristic、间接漫反射、完整路径追踪或 Path Guiding。性能数据是合成纯漫反射相机循环，未计玻璃、太阳、光源追踪或 PNG；固定比例不保证所有场景降噪，后续可单独比较比例、分层采样和进一步 PDF/可见性优化。

### HDR 阶段 D3 — 固定配额的分层 Balance MIS

- [x] HDRI 且 N>=2 时按 `n_b=floor(N/2)`、`n_e=N-n_b` 固定执行 BSDF 后 HDRI 的采样配额；N=2/3/4/8 对应 1+1、1+2、2+2、4+4。不再消耗策略选择随机数，零贡献或 None 不补采。
- [x] 两种来源共用单方向贡献辅助函数，直接累加 `f/(n_b*p_b+n_e*p_e)`，不再额外除 N；奇数使用实际计数。D1 完整环境 PDF、有限性检查和二值可见性直接复用。
- [x] N=1 保持 D2 随机 50/50 与浮点/RNG 序列；渐变天空保留原纯 BSDF 行为。冻结 f12677f 的 D2 估计器仅供测试，不增加生产配置开关。
- [x] 新增 9 项单元测试和默认忽略的比较，覆盖 D2 单样本 RGB 位/RNG 一致（debug/release）、实际配额与随机数消耗、偶数/奇数完整分母、合法零 PDF/None、背半球/遮挡/溢出、均匀亮度、高对比有/无遮挡的独立积分收敛。扩展实际 HDRI 相机回归到 N=1/2/3/4/8、1/2/4 线程及重复运行，三图逐位一致、分量相加正确，环境不改变光源图和相机 Caustics。
- [x] 运行 `cargo test --release --locked compare_stratified_sampling -- --ignored --nocapture`：32×24、32 SPP、N=2/4/8、种子 0–15，比较 D2/D3 的均匀、高对比、部分遮挡 HDRI，记录平均 Y、偏差、RMSE、每帧时间与同等误差效率估算。完整数值、参考口径和限制见 README。
- **实测：** 三类场景的 D3/D2 MSE 比值分别为 `0.5967–0.6137`、`0.9571–0.9817`、`0.8917–0.9378`，同预算耗时比为 `0.9473–0.9627`。高对比降噪幅度较小，未宣称普遍显著提升；按 MSE×耗时外推的同等误差耗时比为 `0.5733–0.9390`，并非直接测量匹配画质耗时。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked`（268 项单元测试、38 项集成测试通过，两个比较默认忽略）、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check` 通过；D3 release 比较及 release 单样本位一致性另行通过。
- **范围与限制：** 只完成 D3，分层仅指策略配额。默认 SPP/环境样本数、HDRI CDF/方向采样、Lambertian/玻璃/Fresnel、光子焦散、相机逐像素 RNG/Rayon、颜色管理均未修改；不实现 Power Heuristic、间接漫反射、完整 Path Tracing、Path Guiding 或新反弹。后续单独扩大真实 HDRI/遮挡测试范围，评估剩余方差和开销。

### HDR 扩展 — β=2 Power Heuristic

- [x] 在 D3 固定配额上增加可选 Power 权重：`q_B=n_B*p_B`、`q_E=n_E*p_E`，`w_i=q_i²/(q_B²+q_E²)`，贡献为 `f*w_i/q_i`。样本数相同时化为用户给出的 p² 公式；奇数保留实际计数，不额外除总数。
- [x] 使用最大 q 缩放后平方并稳定求 `w_i/q_i`，处理零 PDF/非法值；样本来源显式参与计算，不误用 Balance 分母。两模式共用所有光照与可见性逻辑，方向样本和 RNG 消耗不变。
- [x] 配置 `scene.environment_mis_heuristic` 支持 balance/power，旧配置缺省 Balance；默认 JSON 显式 Power，默认 N=1/SPP 不变。单样本 D2 和渐变天空保持原结果与随机流。
- [x] 新增 6 项单元测试及默认忽略的 Power 比较（包含配置测试），扩展两模式 1/2/4 线程、分量与焦散回归。实际运行 release 比较及两项 release 单样本兼容测试；均匀/高对比积分、互补权重、极端尺度、来源、奇数及零贡献边界均通过。
- **实测：** 相同 32×24、32 SPP、16 种子、N=2/3/8 下，Power/Balance MSE 比为均匀 `0.5321–0.9067`、高对比 `1.0325–1.0633`、部分遮挡 `1.0074–1.0180`，耗时比 `1.0060–1.0809`。Power 并不普遍降噪；保留 Balance 可选，详细数值与限制见 README。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked`（274 项单元测试、38 项集成测试通过，三个比较默认忽略）、严格全目标 Clippy、`git diff --check` 通过。不修改 HDRI 分布、材质/玻璃/Fresnel、光子追踪、相机并行/种子派生或颜色管理，不新增反弹。

### 阶段 E1 — 单次 Lambertian 漫反射间接光照

- [x] `SceneConfig` / `Scene` 增加 `max_diffuse_bounces: usize`，缺省为 0，只允许 0/1；不改变 `MAX_GLASS_BOUNCES=8`。旧 JSON 兼容，默认配置、SPP、环境样本数、HDRI 分布、曝光和颜色管理不变。
- [x] 保留第一次表面的原光照，通过 `sample_diffuse_ray` 和 `sample_one_diffuse_indirect` 额外连接一个 Lambertian 表面；RGB 吞吐量为 `f_r*cos/pdf*checker_A`，Lambertian 等于 `albedo_A*checker_A`，与原玻璃标量权重相乘。方向/法线/BSDF/PDF/吞吐量检查及法线偏移完整处理。
- [x] B 复用现有太阳和 `environment_lighting`（Balance/Power MIS），应用自己的材质与棋盘格，仅返回直接光照。绑定阶段 14 地面时只用 Direct 分支，不查询焦散图；旧完整地面图无法分离焦散，第二表面采用原 RGB 解析太阳及环境，第一表面旧行为保留。
- [x] miss、玻璃、未绑定/不支持材质和背面返回零；不加入逃逸 HDRI、不返回二次焦散、不生成第三次漫反射。新增贡献仅进入 Direct。
- [x] 每像素独立漫反射 RNG 同时供 A 的方向与 B 的环境估计使用，关闭 E1 无新随机数消耗；保持相机/玻璃/第一表面环境流、Caustics、固定 seed 和 1/2/4 线程一致性。
- [x] 新增 `diffuse_indirect` 示例，运行时添加红色竖墙与灰地面，在相同参数下输出 0/1 对照。默认三棱镜、JSON 几何格式及所有光源端实现不变。
- **验证：** 新增 14 项测试，冻结 `2521356` 相机控制流逐位核对 0 的输出/RNG；比较实施前记录的光源图指纹。两垂直平面的太阳参考为 `rho_A*rho_B/(sqrt(2)*pi)`，均匀 HDRI 参考为 `rho_A*rho_B*L_env/4`，各用 32768 样本核对解析值/标准误差；覆盖彩色权重、真实玻璃事件、偏移、遮挡、背面、非法数值、地面绑定、两种 MIS、无重复 HDRI/焦散、分量、seed 和线程。
- **验收：** `cargo fmt`、`cargo fmt --check`、`cargo test --locked`（288 项单元测试、38 项集成测试通过，3 项既有 MIS 比较默认忽略）、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check` 全部通过。13 项 E1 测试另在 release 模式通过；release 示例实际生成并查看 320×240、64/256 SPP 对照 PNG。
- **范围与限制：** 只完成 E1，属于截断后的受限 RGB 间接光估计器，不是通用无偏 Path Tracer。无多次漫反射、Russian Roulette、漫反射后的玻璃续追、透明阴影、环境/二次焦散、Path Guiding 或通用 Photon Mapping；下一阶段需单独定义计光规则和验收。

## 6. Codex 执行约定

1. **每次只实现当前一个里程碑**，完成后总结修改文件、运行命令、测试结果和下一步；不要抢跑后续阶段。
2. 编写可运行代码而不是伪代码；缺少标准光谱数据时，明确说明来源问题和当前阻碍，不要捏造数据或悄悄退化为假颜色。
3. 先写可测的核心函数，再连接 CLI 与 PNG 输出。优先简洁实现，避免过早抽象。
4. 凡涉及几何、折射、光谱单位与 sRGB 编码的计算，都要加边界条件检查和至少一个对应测试。
5. 修改代码后至少执行 `cargo fmt`、`cargo test`；里程碑完成后执行 `cargo clippy -- -D warnings`，报告真实结果。
6. 未经明确要求，不增加 GUI、GPU、路径追踪、第三方渲染框架或完整光子映射数据结构。

### 建议给 Codex 的第一条指令

> 请先阅读 `ROADMAP.md` 和 `AGENTS.md`（若存在）。**只完成 Day 1**：创建 Rust 工程、添加最小依赖、实现浮点 Framebuffer 与 PNG 输出、编写必要测试，并运行 `cargo fmt`、`cargo test`。不要实现 Day 2 及以后的功能。完成后列出修改文件、如何运行、测试结果，以及下一步计划。

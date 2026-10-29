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

## 6. Codex 执行约定

1. **每次只实现当前一个里程碑**，完成后总结修改文件、运行命令、测试结果和下一步；不要抢跑后续阶段。
2. 编写可运行代码而不是伪代码；缺少标准光谱数据时，明确说明来源问题和当前阻碍，不要捏造数据或悄悄退化为假颜色。
3. 先写可测的核心函数，再连接 CLI 与 PNG 输出。优先简洁实现，避免过早抽象。
4. 凡涉及几何、折射、光谱单位与 sRGB 编码的计算，都要加边界条件检查和至少一个对应测试。
5. 修改代码后至少执行 `cargo fmt`、`cargo test`；里程碑完成后执行 `cargo clippy -- -D warnings`，报告真实结果。
6. 未经明确要求，不增加 GUI、GPU、路径追踪、第三方渲染框架或完整光子映射数据结构。

### 建议给 Codex 的第一条指令

> 请先阅读 `ROADMAP.md` 和 `AGENTS.md`（若存在）。**只完成 Day 1**：创建 Rust 工程、添加最小依赖、实现浮点 Framebuffer 与 PNG 输出、编写必要测试，并运行 `cargo fmt`、`cargo test`。不要实现 Day 2 及以后的功能。完成后列出修改文件、如何运行、测试结果，以及下一步计划。

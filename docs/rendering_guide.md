# 渲染技术指南

运行第一张图请从[快速开始](../README.md#快速开始)进入。以下示例命令均在仓库根目录执行。

## 从入口阅读代码

```text
main.rs
  Config::load → Config::build → Renderer::new
  Renderer::render
    Photon Integrator → XYZ 焦散图 / Eλ 图集
    Camera Integrator → Direct / Caustics / Combined XYZ
  output::save_components → 曝光 → XYZ 到 linear sRGB → ACES → display sRGB → PNG
```

```rust
use spectral_caustics::{config::Config, renderer::Renderer, output};
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::load("configs/default.json")?;
    let job = config.build()?;
    let renderer = Renderer::new(config.render.clone())?;
    let images = renderer.render(&job)?;
    output::save_components(&images, &config.render.output_path, config.render.exposure)?;
    Ok(())
}
```

`Renderer::render(&RenderJob)` 是唯一的高层渲染入口。Renderer 只持有设置和可选 Rayon 线程池，负责选择 Pipeline 和执行调度。它不实现散射、Fresnel、路径循环或图像编码。`Scene` 只保存物体、材质、光源、环境、背景、可选介质并求最近交点。`RenderJob` 将 Scene、Camera、相机采样选项、光子接收区及物体编号组合起来。

目录按职责组织：

| 路径 | 职责 |
| --- | --- |
| `config/{camera,light,material,scene,render}.rs` | JSON 数据与输入检查 |
| `config/{build,job}.rs` | 构建运行时对象及编号绑定 |
| `scene/`、`geometry/` | 场景、几何求交；GeometryHit 由 Scene 绑定成带有效编号的 Hit |
| `light/`、`environment/` | 平行光 / 圆盘的发射分布、环境辐亮度与方向分布 |
| `material/` | RGB / 标量光谱 BSDF、采样事件及方向 PDF |
| `medium/` | 均匀/密度网格介质、局部 Majorant/Delta Tracking、私有 DDA、精确 Beer–Lambert、HG 相函数 |
| `optics/interface.rs` | Snell、Fresnel、介质界面事件及两种传输权重 |
| `optics/dispersion.rs` | Constant / Cauchy 折射率；波长 nm、Cauchy 内部 μm |
| `integrator/camera/path.rs` | RGB / Hero 共用的路径主循环、预算、RR、分量归属 |
| `integrator/camera/{rgb,spectral,transport,types}.rs` | 像素 / 波长采样，数值表示与状态 |
| `integrator/camera/lighting/` | 表面 / 体散射 NEE、环境 MIS、几何遮挡与介质透射率 |
| `integrator/photon/{generation,path}.rs` | 共享发射 / 沉积内核、光谱积分及单光子 Forward 循环 |
| `caustics/` | 接收区、只读 XYZ / Eλ 数据、空间 / 波长插值 |
| `sampling/` | 确定性随机流派生 |
| `spectrum/` | 可追溯 CIE / D65 数据及 RGB 光谱近似 |
| `film.rs`、`color.rs`、`output.rs` | 线性 XYZ 存储、颜色变换、曝光 / ACES / PNG |
| `diagnostics/` | feature 控制的路径观察、AOV、统计与固定波长实验 |
| `tests/`、`examples/` | 数学 / 物理 / 确定性回归，以及独立实验入口 |

低层实验可直接调用 `integrator::camera::{integrate_rgb, integrate_spectral}`，两者均接收完整 RenderSetting 和可选只读焦散数据，并返回三个线性 XYZ 缓冲。`integrator::photon::{integrate_xyz, integrate_spectral, trace_photon}` 分别用于地面照明预览、Eλ 图集及单路径实验；它们不通过 Scene 的方法执行。正常相机 Pipeline 不分配 sampled Direct 光子缓冲。

## 传输约定

BSDF 的 `wi` / `wo` 均从交点向外。连续事件返回方向 PDF；Delta 事件没有普通立体角 PDF。`BsdfSample<Vector3<f64>>` 与 `BsdfSample<f64>` 的 weight 都已包含 `f·|n·wi|/p` 或离散事件的概率补偿。积分器不再重新乘一次 PDF 或 Fresnel。

相机与光子共享介质界面事件，但保留物理权重的区别：反射抽样概率为 F，透射为 1−F。光子通量的权重是事件系数除以抽样概率，不含折射率平方；相机 Radiance 透射另外乘 `(η_i/η_t)²`。进入 / 离开同一介质时这两个平方因子互相抵消。全反射是真实的确定事件，不消耗额外选择随机数。

相机主循环在首次命中连续材质时立即着色。后续连续顶点仍接收太阳、圆盘和环境直接照明，计入 Direct；只有第一个连续顶点且为接收地面时读取 Caustics。后续地面不再读取图集。环境 NEE 将玻璃视为二值遮挡；漫反射后只有最近事件为 Delta 的环境逃逸会额外计入 Direct，最近为 Diffuse 的 miss 不再累加环境。常量背景只照亮相机 / Delta 前缀的背景查询。

Delta 预算为 32 次，相机漫反射预算为 0..1024。第三次连续表面散射后、仍有后续表面预算时执行 RR：RGB 使用 throughput 最大分量，光谱使用标量，`p=clamp(importance,0.05,0.95)`，存活路径除以 p。零贡献、最后顶点和浅层路径不抽 RR。预算截断的能量不补偿。

Forward 光子只追踪选中的一个理想介质和接收地面。其他物体阻断路径。每条光子的初始通量为 Φ/N，地面辐照度除以像素面积，四邻域沉积保持区域内能量。`photon_max_events=1..1024`，默认 32；到达上限后仍可落地 / 逃逸，遇到下一个界面才截断。从真实界面交点继续追踪，避免棱边法线偏移跨入错误介质。

发射阶段缓存命中选中玻璃的光子的起射区域、首交点和首段 Tr，供各波长复用。XYZ 与 Eλ 共用同一发射、Forward 和沉积内核。Constant 追踪一次几何，Dispersion 从首交点逐波长续追。所有并行结果按固定索引顺序归约。

Forward 对每个有限命中段用活跃介质的 `Medium::transmittance_ray` 计算 Tr，并乘入通量。截取范围是当前射线起点到下一命中，覆盖发射、玻璃内续追和落地。未绑定介质界面时，`Scene.medium` 在其空间盒内覆盖空气段和玻璃段。独立光子 Direct 预览使用起射区域计算光源到地面的 Tr。透射率须在 `[0,1]` 内，非法值返回错误，零值或浮点下溢对应零通量。消光无需随机抽样，沉积沿用发射归一化 Φ/N。

平行光总通量定义在水平发射窗口上。窗口面积为 A、单位传播方向为 d 时，光束法向辐照度为 `Φ/(A·|d_z|)`，水平地面收到 `Φ/A`。窗口限制光子发射位置，不裁切解析太阳。圆盘按面积均匀、方向余弦分布发射；相机使用面积 NEE 和有限距离阴影线段。光子 Direct 只用于独立地面预览，不能和解析直射重复相加。

Hero 相机在 380..780 nm 分层抽样，每项 XYZ 权重为 `CMF(λ)·Lλ·400/camera_spp`，不再乘 Δλ。Packet=4 的 camera_spp 仍为总光谱路径数，空间主射线数为 camera_spp/4；每个 Lane 覆盖独立 100 nm 波段的细分层，主射线和路径随机序列使用 CRN 副本。光源离散积分另用梯形 Δλ；Eλ 图集保存光谱密度，不提前乘 CMF 或 Δλ。

## 输入和错误边界

JSON 解析和运行时构建分开。`Config::load/from_json` 解析结构；`Config::build` 在实际创建相机、材质、几何、光源和环境时验证参数。`Renderer::new` 验证渲染设置并创建线程池。`Config::validate` 可用于显式配置检查 / 保存，但主渲染不先构建一套临时几何再重新构建。

Scene 的命中总有有效 object_id / material_id；Framebuffer 尺寸和 Vec 长度由构造函数绑定，外部只获得像素切片。静态光子目标与 IOR / 预算在 PhotonTracer 创建时检查，路径初态检查一次。环境参数只在构造或显式修改时检查；圆盘面积和发光辐亮度在构造时缓存；太阳参数在像素循环外准备。核心计算仍处理合法缺失、TIR、PDF=0、退化方向和实际浮点溢出；自定义 Material 返回非法权重时会明确失败。不会用 debug_assert 掩盖 Release 中正常输入可能出现的数值错误。

## 配置与 Diagnostics

| 字段 | Rust 缺省 | 说明 |
| --- | --- | --- |
| `camera_mode` | `rgb` | `rgb` / `spectral` |
| `camera_spp` | 64 | 光谱模式是总路径数；展示 JSON 通常显式使用 256 |
| `spectral_packet_size` | 1 | 仅 1 / 4，SPP 必须整除 |
| `photon_max_events` | 32 | Forward 界面事件上限 |
| `samples_per_pixel_axis` | 4 | 光子发射网格每轴分层，和相机 SPP 独立 |
| `scene.environment_samples` | 1 | 展示 JSON 显式设为 8 |
| `scene.environment_mis_heuristic` | `balance` | 展示 JSON 使用 `power` |
| `scene.max_diffuse_bounces` | 0 | 展示 JSON 使用 4 |
| `scene.max_volume_scatters` | 1 | 相机体散射预算 0..1024；0 只计算消光，雾示例使用 8 |

Sphere / Prism / VerticalPrism 的 `caustics` 均缺省关闭；开启时必须绑定 Dielectric，并且只能选择一个目标。

光源 JSON 必须有 `"type":"parallel"` 或 `"type":"disk_area"`。旧 photon_trace_mode 和旧调试字段已删除，不接受兼容格式。

正常构建不编译 Diagnostics 类型，不分配诊断缓冲。需要路径 AOV 时显式开启 feature，再在 render 中设置：

```json
"diagnostics": {"path": true, "target": 0, "disable_ground_sun": false}
```

```sh
cargo run --release --locked --features diagnostics -- /path/to/diagnostic-config.json
cargo run --release --locked --features diagnostics --example spectral_wavelength_projection -- 64 64 output/fixed_wavelength
```

Diagnostics 在 Debug / Release 都由 feature 控制。target 省略时选择光源焦散目标；没有目标则不统计目标路径。Observer 只记录实际相机事件，不采样、不计光，不改变 RNG。RGB 和光谱均支持 AOV；`disable_ground_sun` 是光谱实验选项，只跳过地面解析太阳，不改变源端通量、环境或焦散。未开启 feature 的构建会拒绝 diagnostics 配置。

## 示例与验证

```sh
cargo run --release --locked --example camera_direct_caustics -- 8
cargo run --release --locked --example camera_parallel_benchmark -- 16 64
cargo run --release --locked --example spectral_camera_compare -- 64 64 output/compare configs/vertical_prism_spectral_caustics.json
cargo run --release --locked --example sphere_wavelengths
cargo run --release --locked --example refract_ray

cargo fmt --check
cargo build --locked --all-targets
cargo test --locked
cargo test --locked --features diagnostics
cargo test --locked --all-targets --features diagnostics
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features diagnostics -- -D warnings
git diff --check
```

固定配置 / seed 在当前实现的 Rayon 1/2/4 线程下逐位可复现。改变采样块数量会改变浮点加法分组，测试使用数值容差；不冻结旧算法的 RNG 消耗或旧版本逐位结果。

当前不支持嵌套介质、透明 Shadow Ray、环境焦散、通用 Photon Mapping、BVH / KD-Tree、GUI / GPU 或光谱 GGX。RGB 到光谱是近似模型，CIE / D65 及 PBRT 基谱来源、许可证与校验和见 [颜色数据说明](../assets/color/README.md)。

## 均匀介质与多次体散射

`configs/homogeneous_fog.json` 提供球体、棋盘地面和逆光雾。省略 `scene.medium` 或设为 `null` 时，外部按真空处理。这个示例使用以下均匀介质。

```json
"medium": {
  "min_m": [-3.0, -2.5, 0.02],
  "max_m": [3.0, 3.0, 3.5],
  "sigma_a": 0.03,
  "sigma_s": 0.22,
  "g": 0.7
}
```

边界是世界坐标轴对齐盒，距离单位为米，`sigma_a`、`sigma_s` 单位为 m⁻¹，`sigma_t = sigma_a + sigma_s`；`-1 < g < 1`。这些系数目前与波长无关；RGB 和 Spectral 在各自 Transport 中乘同一个标量权重，没有 RGB 插值介质光谱。盒边界没有可见表面、不折射。省略物体的介质界面绑定时，`scene.medium` 的空间盒也覆盖盒内玻璃的内部。

每条当前射线先求最近表面，再把盒交区间裁到该距离（miss 则裁到盒出口）。体散射次数尚未达到 `scene.max_volume_scatters` 时，用 `s = -ln(1-u)/sigma_t` 抽取区间入口后的自由程。如果 `s < L`，距离 PDF 是 `sigma_t exp(-sigma_t s)`，因此 `sigma_s Tr(s) / pdf = sigma_s/sigma_t`，把这项乘入 throughput；这是隐式吸收，每次散射均按单次散射反照率缩权。若自由程越过区间，离散生存概率为 `Tr(L)`，其权重 `Tr(L)/Tr(L)=1`，不能再乘衰减。达到预算后使用解析 `exp(-sigma_t L)`，不再采样自由程；`sigma_s=0` 时无论预算多少都使用解析 `exp(-sigma_a L)`。解析分支不消费介质 RNG。

介质点的 `wo=-ray.direction`、`wi` 都远离交点，HG 为
`p(wo,wi)=(1-g²)/[4π(1+g²+2g wo·wi)^(3/2)]`。`g=0` 为 `1/(4π)`；正 g 的峰在 `wi=-wo`，表示前向散射。采样通过逆 CDF 绕 `-wo` 生成方向，返回 `p=pdf`，续追 throughput 乘 `p/pdf=1`，没有表面余弦。方向约定及公式参考 [PBRT Phase Functions](https://pbr-book.org/4ed/Volume_Scattering/Phase_Functions)。

散射点直接照明复用太阳、圆盘和渐变/HDRI 环境：太阳乘 `p Li V Tr`，圆盘乘 `p Le cos_light V Tr/(r² pdf_area)` 并按样本数平均，环境将 HG 和环境方向采样用 Balance/Power MIS 组合。环境配额沿用 `environment_samples`，圆盘沿用 `light_samples`；HG/环境均覆盖整球，没有接收面余弦。所有阴影段保持几何二值遮挡（包括玻璃），并乘盒内实际长度的 `Tr`；圆盘阴影裁到采样光源位置。已有表面的太阳、环境、圆盘 NEE 同样受到介质衰减。

每个散射点均调用上述体照明，HG 抽取的续追射线回到同一个 Camera Path 循环，可再次体散射、命中并着色表面、继续既有 BSDF 路径。`CameraPathState.volume_scatters` 只记录体散射次数，预算放在 `CameraOptions.max_volume_scatters` 中，不占表面 `max_diffuse_bounces` 或 Delta 预算；Diagnostics 的表面事件计数也不统计体散射。预算缺省为 1，0 关闭体散射但保留总消光，值越大允许的散射阶数越高。体散射不新增 RR，现有表面 RR 保持原规则。体散射及其后续表面贡献归入 Direct，不读取 Caustics。每次 HG 续追直接逃逸环境均由当地环境 NEE/MIS 负责，不再次累加；经过后续表面 Delta 链逃逸时仍沿用既有规则。

介质自由程、体照明和 HG 使用单独 `MEDIUM_STREAM`，不消费像素抖动、BSDF、首次环境或漫反射续追的随机数。Packet 的各波长 lane 从相同介质 RNG 状态的副本开始；seed 和 Rayon 1/2/4 线程保持确定性。低层调用 `trace_camera_path` 时须为 `CameraPathRngs.medium` 提供独立 RNG。

```sh
# 单个场景：编辑 scene.medium 的系数和 g、scene.max_volume_scatters
cargo run --release --locked -- configs/homogeneous_fog.json
# 四组对照：无介质 / 同等消光的纯吸收 / g=0 / g=0.7
# 参数：SPP、宽度（高为 2/3）、输出目录、rgb|spectral、密度倍率
cargo run --release --locked --example homogeneous_medium_compare -- 256 384 output/medium_compare rgb 1.0
cargo run --release --locked --example homogeneous_medium_compare -- 256 384 output/medium_spectral spectral 1.0
# 数学与端到端验证
cargo test --locked --lib medium::tests
cargo test --locked --test volume
```

每个对照子目录包含三个分量 PNG 和可独立重跑的 `config.json`。纯吸收组把相同总消光全部设为 `sigma_a`；两个散射组保持相同 `sigma_a/sigma_s`，只改变 g。密度倍率同时缩放两个系数，0 等价于无介质。

相机预算截断后丢弃更高阶散射。焦散图记录未发生体散射的源端通量，由相机在符合条件的地面交点读取。Ratio Tracking、介质发光和波长相关系数仍未实现。平行光沿用无限解析太阳模型，发射窗口只裁剪光子发射位置。

### 玻璃介质界面

每个场景可以为一个理想玻璃物体绑定 `medium_interface`。字段写在 `scene.objects[i]` 中，与 `material`、`caustics` 同级。支持 `sphere`、`prism` 和 `vertical_prism`，材质须为 `dielectric`。绑定本身独立于焦散目标选择。

| JSON 写法 | 介质行为 |
| --- | --- |
| 省略 `medium_interface` 或设为 `null` | 无区域绑定，路径按 `scene.medium` 的空间范围计算介质效果。 |
| `"medium_interface": { "interior": null }` | 绑定边界，内部为参与介质真空，表面按玻璃 IOR 折射和反射。空对象 `{}` 同样表示内部真空。 |
| `"medium_interface": { "interior": { ... } }` | 绑定边界，内部使用指定的介质。内部介质接受与 `scene.medium` 相同的均匀或网格配置。 |

绑定后外侧使用 `scene.medium`。内部介质的 `min_m` 和 `max_m` 仍是世界坐标，内部消光和散射只发生在玻璃内部与介质盒重叠的范围。

`configs/sphere_medium_interface.json` 在有雾的场景中把玻璃球内部设为真空。运行下列命令会把三张分量图写到 `output/sphere_medium_interface/`。

```sh
cargo run --release --locked -- configs/sphere_medium_interface.json
```

场景构建取得物体编号后调用 `Scene::bind_medium_interface(object_id, MediumInterface { interior })`。Rust 调用方可以直接使用同一个接口，`interior: None` 表示内部真空。

Camera Path 首段用 `initial_medium_region` 对起点分类。位于几何边界或其浮点舍入范围内的起点，须通过低层 `CameraPathState.medium_region` 显式给出出发侧。每段用 `active_medium` 解析介质，表面和体积顶点的局部阴影查询使用同一区域的介质，玻璃仍按二值遮挡处理。接受界面事件后，`region_after_event` 用入射传播方向决定下一段区域。反射（含 TIR）和体散射保留当前侧，透射切换到另一侧。路径使用期间应保持场景物体索引稳定。区域只描述参与介质，IOR 和事件权重由光学接口负责。

Photon Path 在发射点分类。位于选中的已绑定玻璃内部时，以 `PhotonMedium::Glass` 起射。反射（含 TIR）保留折射率侧别和参与介质区域，透射切换折射率侧别，并在穿过绑定界面时切换区域。Forward 的起射点须避开介质边界及其浮点舍入范围。非焦散目标的玻璃仍阻断光子。

绑定介质边界的场景中，光子续追、相机续追和表面阴影射线均从实际交点发出，避免法线偏移跨过近处的玻璃。几何求交仍排除距离不大于 1 µm 的命中。

## 非均匀密度网格

运行时接口为 `Medium: Send + Sync` trait，`HomogeneousMedium` 与 `GridMedium` 各自实现它，`Scene.medium` 保存 `Option<Box<dyn Medium>>`。这与 `Material` 的接口和所有权方式一致，自定义介质可以直接 `Some(Box::new(medium))` 放入 Scene，无需修改内置类型列表。

相机传输只使用三个关键接口：`sample_segment(...)` 返回 `Result<MediumSample, String>`，包含真实散射点（可缺省）和已补偿权重；`transmittance_ray(ray, max_distance)` 返回段透射率；`phase()` 返回相函数。内部算法的封装原则见 [架构职责边界](../.agents/architecture.md#当前物理范围)。另有只读的 `density_at(position)`、`coefficients_at(position)`、`sigma_t_majorant()` 供检查介质数据；直接在具体类型上调用 trait 方法需导入 `medium::Medium`。局部系数由 `MediumCoefficients` 返回，其 `sigma_t()` 为吸收与散射系数之和。

JSON 的 `scene.medium` 根据互斥字段结构区分 `MediumConfig` 两个配置变体，再由 `medium::from_config` 构造 `Box<dyn Medium>`：已有均匀配置保留 `sigma_a/sigma_s`，网格使用 `resolution/densities/sigma_a_base/sigma_s_base`；不需要 `type` 字段。最小双层网格配置：

```json
"max_volume_scatters": 8,
"medium": {
  "min_m": [-2, -2, 0],
  "max_m": [2, 2, 2],
  "resolution": [1, 1, 2],
  "densities": [1.0, 3.0],
  "sigma_a_base": 0.03,
  "sigma_s_base": 0.22,
  "g": 0.7
}
```

`resolution=[nx,ny,nz]` 表示世界 AABB 内各轴等分的体素数，全部为正，数组长度必须是 `nx*ny*nz`。密度无量纲，非负且有限，按 **x 最快、其次 y、最后 z** 排列：`index=x+nx*(y+ny*z)`。第 i 个轴向分界面是 `min+(max-min)*i/n`；位置映射相当于 `floor((p-min)/(max-min)*n)`，并对分界面的浮点舍入与遍历保持一致。内部面归较大下标，外部 max 面归最后一格，闭合外边界与原均匀盒一致；盒外为真空。构造时拒绝尺寸溢出、无效系数、密度缩放溢出及世界坐标精度下塌缩的体素。

每格满足 `sigma_a=density*sigma_a_base`、`sigma_s=density*sigma_s_base`。`sigma_t_majorant()` 仍返回全局最大消光，供数据检查；采样使用 Phase 3.3 的私有粗块 Majorant 缓存。每块覆盖 **8×8×8 个细体素**（不足八格的尾块按实际范围覆盖），保存块内最大的 `sigma_a+sigma_s`。用与系数查询相同的乘加顺序计算，不能用平均密度代替上界。块边界直接使用原细网格的分界面，尾块不会重新等分或拉伸。数据构造后只读，缓存不会失效。若所有块的上界相同，内部合并成一个段，避免无收益的粗块遍历。

透射率先将射线裁到 `[0,max_distance]` 与 AABB 的交集，再用 3D DDA 遍历体素，计算 `tau=Σ(sigma_a,i+sigma_s,i)*length_i`、`Tr=exp(-tau)`。这是分块常数模型的精确积分（受浮点舍入限制），不使用固定步长。每次跨面至少推进一个体素下标，边/角处同时推进所有相交轴；零方向轴不跨面，面上反向起步通过零长度跨面处理，不用 epsilon 丢弃薄层。相机路径的确定性消光分支和太阳/圆盘/环境阴影共用此透射率。

`GridMedium::sample_segment` 内使用分段局部 Majorant `M_b >= sigma_t(x)` 进行 [Delta Tracking](https://pbr-book.org/4ed/Volume_Scattering/Transmittance)。粗块 DDA 按前后顺序给出射线段与其局部上界，迭代器只在介质模块内部使用，不暴露给 Camera Integrator：

1. 延迟抽取单位指数光学深度 `tau=-ln(1-u)`，`u` 取开区间 `(0,1)`。`M_b=0` 的块直接跳过，不消费 RNG。
2. 当前段剩余长度为 L。若 `tau >= M_b*L`，扣除 `M_b*L`，携带剩余光学深度进入下一块；否则候选点在当前起点后 `tau/M_b` 米处。跨块携带的是光学深度，不能直接复用米制自由程。
3. 候选点 x 以 `sigma_t(x)/M_b` 接受为真实消光碰撞；空碰撞后重新抽取光学深度，继续内部循环。跨块不额外乘 Tr，也不产生物理路径顶点。
4. 真实碰撞的边缘距离密度仍为 `sigma_t(x) Tr(x)`，隐式吸收权重为 `sigma_s(x)/sigma_t(x)`。存活至段尾的概率已是 Tr，因此返回无碰撞、权重 1。

空碰撞不返回给相机、不调用 HG/NEE，也不占 `max_volume_scatters`；没有任意候选次数截断。真实散射继续复用现有多次体散射循环，支持 RGB/Hero/Packet。纯吸收、全零有效散射或预算耗尽时返回精确细体素 DDA 消光；不相交或空区间返回权重 1，这些分支不消费介质 RNG。即使全局上界非零，仅穿过零上界块的射线也不消费 RNG。随机采样仅使用介质流，Packet 各 lane 持有各自的 CRN 副本。

构造保证每块 Majorant 覆盖其局部消光；碰撞距离无法以 f64 表达在段内、区间/位置溢出或舍入后查询到的消光超出当前局部上界时返回错误，不推进 epsilon、夹紧接受概率或静默截断。细体素与粗块复用私有 DDA 步进，阴影与确定性分支仍使用精确 Tr，未引入 Ratio Tracking。改变分块布局会改变随机数消耗，版本间比较统计分布；同一设置/seed 在 1/2/4 线程下仍逐位一致。

```sh
cargo run --release --locked -- configs/grid_fog.json
cargo test --locked --lib medium::
cargo test --locked --test grid_medium
cargo test --locked --test grid_scattering

# 七组对照，固定 seed=42；参数为 SPP、宽度、输出目录、模式
cargo run --release --locked --example grid_medium_compare -- 256 384 output/grid_compare rgb
cargo run --release --locked --example grid_medium_compare -- 256 384 output/grid_hero hero
cargo run --release --locked --example grid_medium_compare -- 256 384 output/grid_packet packet
```

`configs/grid_fog.json` 是 4×2×3 密度变化的雾场，默认 192×128、32 SPP、seed=42、体预算 8；输出在 `output/grid_fog/`。设预算为 0 可观察相同总消光下不含散射光的图像。

`grid_medium_compare` 输出 `homogeneous`、`uniform_grid`、`layered_single`、`layered_multiple`、`clumped_extinction`、`clumped_single`、`clumped_multiple` 七组，每组包含三个分量 PNG 和可重跑的 `config.json`。均匀/分层/团块密度均归一到体积平均密度 1，系数、相机、光源、曝光与 seed 相同；散射预算分别为 0/1/8。分层与团块网格为 12×10×8 的分块常数场。均匀网格与均匀介质的随机数消耗不同，比较统计分布与能量，不要求低 SPP 图像逐位相同。`hero` 使用单波长，`packet` 使用四波长 CRN，SPP 须能被 4 整除。

局部上界性能入口（计时不作为 CI 断言）：

```sh
# 参数：射线数、每轴体素数、重复次数、输出目录、整图 SPP（0 跳过）、整图宽度
cargo run --release --locked --example grid_majorant_benchmark -- 100000 64 5 output/grid_bench64 16 96
cargo run --release --locked --example grid_majorant_benchmark -- 100000 128 5 output/grid_bench128 0 96

# 在介质内部比较 2³/4³/8³ 块，使用上一步保存的配置作为密度输入
GRID_BENCHMARK_CONFIG=output/grid_bench64/sparse_clumps/config.json cargo test --release --locked --lib medium::grid::tests::benchmark_block_sizes -- --ignored --nocapture
```

基准覆盖均匀、稀疏团块、单体素尖峰，输出 `benchmark.json`；整图开启时另存三个分量和可重跑配置。候选数通过该采样器每个候选的两次 u64 RNG 调用统计，不向相机或 `MediumSample` 添加统计字段。8³ 的选择、缓存成本、测量条件和局限见 [Phase 3.3 基准记录](medium_majorant_benchmark.md)。

Phase 3.1 及此前的文件迁移、清理统计与验收明细见 [重构记录](refactor.md)。

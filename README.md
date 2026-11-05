# Spectral Caustics Renderer

用于学习 Rust 与计算机图形学的 CPU 离线渲染器。相机端是有界的 RGB / Hero Wavelength 路径估计器，光源端使用 Forward Photon Tracing，生成单个理想介质在地面上的焦散。它仍采用有限深度、二值玻璃阴影和首次连续地面顶点的混合估计，并不是通用无偏路径追踪器。

## 运行

```sh
cargo run --release --locked -- configs/default.json
cargo run --release --locked -- configs/sphere_spectral.json
cargo run --release --locked -- configs/vertical_prism_spectral_caustics.json
cargo run --release --locked -- configs/vertical_prism_area_light_preview.json
```

配置和资源路径相对于启动目录。默认场景为 Glass / Mirror / GGX 三球；RGB 支持这些材质，光谱相机目前支持 Dielectric / Lambertian / Mirror，拒绝 GGX Metal。HDRI 缺失会返回文件错误，不会静默替换照明。

每次渲染生成 `camera_direct.png`、`camera_caustics.png`、`camera_combined.png`，并在 `render.output_path` 保存 Combined。主图文件名与某个分量重名时，分量改用 `aux_` 前缀。无焦散目标时 Caustics 全黑，仍输出三个分量。三个图在线性空间满足 Direct + Caustics = Combined；PNG 已经过非线性色调映射，不能直接相加。

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

`Renderer::render(&RenderJob)` 是唯一的高层渲染入口。Renderer 只持有设置和可选 Rayon 线程池，负责选择 Pipeline 和执行调度。它不实现散射、Fresnel、路径循环或图像编码。`Scene` 只保存物体、材质、光源、环境、背景并求最近交点。`RenderJob` 将 Scene、Camera、相机采样选项、光子接收区及物体编号组合起来。

目录按职责组织：

| 路径 | 职责 |
| --- | --- |
| `config/{camera,light,material,scene,render}.rs` | JSON 数据与输入检查 |
| `config/{build,job}.rs` | 构建运行时对象及编号绑定 |
| `scene/`、`geometry/` | 场景、几何求交；GeometryHit 由 Scene 绑定成带有效编号的 Hit |
| `light/`、`environment/` | 平行光 / 圆盘的发射分布、环境辐亮度与方向分布 |
| `material/` | RGB / 标量光谱 BSDF、采样事件及方向 PDF |
| `optics/interface.rs` | Snell、Fresnel、介质界面事件及两种传输权重 |
| `optics/dispersion.rs` | Constant / Cauchy 折射率；波长 nm、Cauchy 内部 μm |
| `integrator/camera/path.rs` | RGB / Hero 共用的路径主循环、预算、RR、分量归属 |
| `integrator/camera/{rgb,spectral,transport,types}.rs` | 像素 / 波长采样，数值表示与状态 |
| `integrator/camera/lighting/` | 圆盘 NEE、环境 NEE / MIS、二值 Shadow Visibility |
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

Delta 预算为 32 次，相机漫反射预算为 0..1024。第三次连续散射后、仍有后续预算时执行 RR：RGB 使用 throughput 最大分量，光谱使用标量，`p=clamp(importance,0.05,0.95)`，存活路径除以 p。零贡献、最后顶点和浅层路径不抽 RR。预算截断的能量不补偿。

Forward 光子只追踪选中的一个理想介质和接收地面。其他物体阻断路径。每条光子的初始通量为 Φ/N，地面辐照度除以像素面积，四邻域沉积保持区域内能量。`photon_max_events=1..1024`，默认 32；到达上限后仍可落地 / 逃逸，遇到下一个界面才截断。从真实界面交点继续追踪，避免棱边法线偏移跨入错误介质。

发射阶段只求一次首交点，保存选中介质的命中记录；Direct、未命中和其他遮挡不会在每个波长重新查询。XYZ 与 Eλ 共用同一发射、Forward 和沉积内核。Constant 只追踪一次几何；Dispersion 逐波长追踪。代价是临时缓存选中物体的首交点，内存随这部分光子数量增长。所有并行结果按固定索引顺序归约。

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

当前不支持内部光子发射、嵌套介质、透明 Shadow Ray、环境焦散、通用 Photon Mapping、BVH / KD-Tree、GUI / GPU 或光谱 GGX。RGB 到光谱是近似模型，CIE / D65 及 PBRT 基谱来源、许可证与校验和见 [颜色数据说明](assets/color/README.md)。

本次文件迁移、清理统计与验收明细见 [重构记录](docs/refactor.md)。

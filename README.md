# Spectral Caustics Renderer

Rust CPU 离线光谱焦散渲染器。平行光经过三棱镜的双界面折射形成地面焦散；相机追踪玻璃 Fresnel 事件、有界 Lambertian 多次反弹和 Russian Roulette。环境支持 Radiance HDR 贴图或渐变天空，以 Balance/Power MIS 估计照明，Rayon 并行执行光源采样块和相机像素。

当前 API 已完成 Breaking Cleanup。使用方式和物理边界以本文与当前源码为准，不以历史阶段输出、浮点分组或 RNG 消耗顺序为兼容目标。

## 运行

从仓库根目录执行，需要支持 Rust 2024 edition 的工具链：

```sh
cargo run --release --locked
cargo run --release --locked -- /path/to/config.json
```

默认读取 [configs/default.json](configs/default.json)，使用随仓库提供的 `assets/HDRI/autoshop_01_4k.hdr`。配置、HDR 路径及输出路径中的相对路径均相对于启动目录。主程序接受一个可选 JSON 路径，配置读写不加载 HDR；运行时构建 Scene 才加载并缓存贴图。

主程序生成五张图，辅助图与配置指定的主图保存在同一目录：

| 输出 | 内容 |
| --- | --- |
| `render.output_path`，默认 `output/caustics.png` | 光源端地面总 XYZ 辐照度预览，包含直接落地与折射焦散 |
| `constant_ior.png` 或 `dispersion.png` | 相反折射率模式的地面预览 |
| `camera_direct.png` | 解析太阳、环境背景/照明及漫反射续追贡献 |
| `camera_caustics.png` | 首次地面命中反射的焦散贡献 |
| `camera_combined.png` | Direct + Caustics |

主图占用某个辅助图名称时，该辅助图加 `aux_` 前缀，避免覆盖。所有物理图像使用同一配置曝光，经 XYZ → linear sRGB → 裁剪 → display sRGB → RGB8 PNG 输出。生成图不纳入版本控制；历史阶段 PNG 已移除。

默认展示配置为 512×512、256 SPP、8 个环境样本、Power MIS、4 次额外漫反射连接。快速查看可降低尺寸、`camera_spp` 和光源 `samples_per_pixel_axis`；三者控制不同工作量。

## 主要示例

```sh
# 默认场景的三张相机图；省略 SPP 时使用展示 JSON 的值
cargo run --release --locked --example camera_direct_caustics -- 8

# 彩色墙照亮中性地面，比较 0/1/4 次额外漫反射连接
cargo run --release --locked --example diffuse_indirect -- 16

# 当前实现的 1/2/4/6 线程一致性与耗时，参数为 SPP、正方形边长
cargo run --release --locked --example camera_parallel_benchmark -- 16 64

# 逐界面输出单条光线的方向和落点，便于核对 Snell 几何
cargo run --release --locked --example refract_ray
```

`diffuse_indirect` 输出 `output/diffuse_0.png`、`diffuse_1.png`、`diffuse_4.png`。线程示例比较当前实现的原始 XYZ 缓冲，不与历史版本或历史 hash 比较；计时不含 HDR 加载、线程池创建和 PNG 编码。

## 配置

[默认 JSON](configs/default.json) 包含 `render` 和 `scene`。未知字段、非法数值和溢出工作量返回错误。可省略字段的默认值是当前配置规则，不表示冻结历史算法。

| 参数 | Rust 默认/省略字段 | 展示 JSON |
| --- | --- | --- |
| `render.camera_spp` | 64 | 256 |
| `scene.environment_samples` | 1 | 8 |
| `scene.environment_mis_heuristic` | `balance` | `power` |
| `scene.max_diffuse_bounces` | 0 | 4 |
| `scene.environment.map_path` | 无贴图 | 本地 autoshop HDR |

`SceneConfig::default()` 启用渐变天空；JSON 省略 `environment` 或设为 `null` 则关闭环境照明，miss 使用 `background_linear_rgb`。环境对象内的 `map_path` 缺省/null 使用渐变；HDR 存在时优先查询贴图，`rotation_degrees` 绕世界 +Z 旋转，`intensity` 在线性空间缩放背景和照明。

其他常用参数：

- `render.width/height`：输出尺寸；`samples_per_pixel_axis`：光源每轴分层数量，4 表示每个像素对应 4×4 条空间光线；`seed` 固定随机种子。
- `render.mode`：`constant` 或 `dispersion`；波长范围在 380–780 nm 内，默认 10 nm 步长，包含两端点的梯形积分共 41 节点，最后一段允许短于步长。
- `render.thread_count`：null 使用 Rayon 默认线程数，正整数创建专用池；`max_spectral_blocks` 控制光源采样块，独立于线程数。
- `scene.camera`：位置、目标、up 和垂直 FOV；世界 +Z 向上，位置以米计。
- `scene.objects`：配置流程支持一个三棱镜和一个地面；地面可用 `ground_margin_m` 在 receiver 四边外扩，默认 0.5 m。代码可构建额外三角形/漫反射物体。
- `scene.light`：平行光方向、水平矩形发射窗口、高度和总通量；`receiver` 只限制光子图范围，不限制解析太阳照明。相机超出有限地面的边缘看到背景。
- `scene.ground_checkerboard`：世界 x/y 的棋盘格，按 floor 处理负坐标；比例在每个地面顶点各应用一次，不改变光子图。缺省/null 关闭。

相机分量入口要求绑定地面为中性灰 Lambertian。彩色漫反射墙可以续追；彩色地面可通过独立 RGB 相机 API 渲染，但不能绑定当前中性灰光谱焦散图。

## API 与模块

推荐通过 `Render` 使用已校验参数和专用线程池：

```rust
use spectral_caustics::{config::Config, render::Render, scene::Scene};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load("configs/default.json")?;
    let scene = Scene::from_config(&config.scene)?;
    let renderer = Render::new(config.render.clone())?;
    let lighting = renderer.render_lighting(&scene)?;
    let images = renderer.render_camera_glass_components(
        &scene, &lighting, config.render.camera_spp, config.render.seed,
    )?;
    renderer.save(&images.combined, "output/camera_combined.png")?;
    Ok(())
}
```

- `render_lighting` 一批光子产生 `SpectralLighting`：`direct`、`caustics` 是未曝光的 XYZ 辐照度，另带 receiver、太阳方向和太阳光谱。`combined()` 相加生成地面总辐照度；不能再把它叠加到相机的解析太阳上。
- `render_camera_glass_components` 一批路径同时生成三张未曝光的 XYZ 辐亮度图。相机只读取 `lighting.caustics`；`lighting.direct` 用于光源能量诊断。
- `Render::render` / `render_mode` 输出地面总辐照度，可独立生成光谱/固定折射率对照；内部共用同一分量累积实现。
- 自由函数 `render_camera_glass(scene, width, height, spp, seed)` 保留为无需地面贴图的 RGB 相机，支持自定义场景和颜色材质，使用全局 Rayon 池。它与光谱相机共用路径和像素采样实现，仅分配一个结果缓冲，不包含地面焦散。
- `take_photon_landing` 返回 `PhotonLanding { position, weight }`，用于单条双界面光子路径；批量追踪使用内部 `take_photon_from_hit` 复用首交点。权重不能在累积能量时丢弃。
- `sample_ground_irradiance`、`lambertian_radiance` 是世界坐标查图与中性反射换算；`save_png_with_exposure` 显式指定输出曝光。

`src/render/integrator.rs` 组织 camera、path、diffuse、environment、direct、ground、rng、types 子模块；`src/scene.rs` 保留主类型，构建、物体、光照、棋盘格和光子追踪分别实现。测试共用当前状态 helper，未保留生产代码依赖测试模块的适配入口。 `CameraPathState` 用 `LastScatter::{Camera, Diffuse, Delta}` 记录最近事件，并分别累计漫反射连接和玻璃事件。F2b 第一阶段允许 Diffuse → Glass → Diffuse 的混合传播；经玻璃逃逸环境的贡献仍待下一阶段处理。

## 计光与物理边界

**光源端。** 空气 → 玻璃 → 空气 → 地面，Cauchy 模型 `n(λ)=1.5046+0.0042/λ_μm²` 仅是演示玻璃；固定模式使用 1.52。双透射通量乘 `(1-F_enter)(1-F_exit)`，不含相机 radiance 的折射率平方因子。全反射、其他遮挡、未落地或落在 receiver 外的通量直接丢弃，不重新分配；不续追反射光子。

光谱贡献为 `k × D65(λ) × CMF(λ) × Δλ`，D65 归一化使完整发射光谱 Y=1；空间通量除以总发射数和接收像素面积。固定模式只追踪一条白光折射路径，色散模式逐波长追踪；直接落地仅合成并 splat 一次。双线性 splat 保持区域内能量，非法光谱能量返回错误。

**相机端。** 相机路径在任何漫反射深度都可以采样玻璃 Fresnel 反射/折射；整条路径合计最多 8 个玻璃事件。反射权重为 1，透射权重为 `(η_i/η_t)²`，已抵消事件概率，不再额外乘 Fresnel、余弦或除概率。首先命中的可着色表面立即计光，随后最多续追 `max_diffuse_bounces` 次 Lambertian 连接，范围 0..1024；4 表示最多访问 5 个漫反射顶点。

首个地面顶点将解析太阳和焦散图分别计入 Direct/Caustics，环境只加一次。后续 Lambertian 顶点仅累加太阳和环境直接光到 Direct，不读取焦散图；玻璃顶点可在漫反射之间继续反射/折射，不读取焦散图。未绑定/不支持的材质及背面终止；漫反射之后的 miss 暂不添加逃逸环境辐亮度，即使此前又经过玻璃（已知漏计，下一阶段处理）。背景归入 Direct，因此背景的 Caustics 为零。

第三次漫反射连接后的顶点已着色且仍有预算时执行 RR：`p=clamp(max(RGB throughput),0.05,0.95)`，存活吞吐量除以 p；不会缩放已累计辐亮度。浅层与最后顶点不抽 RR。各顶点使用 `f_r cosθ/pdf` 更新 RGB 吞吐量；Lambertian 配套余弦 PDF 为 `cosθ/π`。

**环境和太阳。** HDRI 按亮度×球面像素立体角建立 CDF，95% 重要性采样与 5% 全球均匀采样混合，使用完整 PDF。N≥2 时，BSDF 配额 floor(N/2)，HDRI 配额为剩余数；Balance 分母为 `n_b p_b+n_e p_e`，Power 使用 β=2 和真实样本数量。N=1 使用随机 50/50 混合；无贴图的渐变使用纯 BSDF 采样。零贡献、遮挡和背半球样本保留在原配额内。

解析太阳和环境的 shadow ray 均是二值遮挡，玻璃也会遮光。光谱太阳使用水平发射窗口投影后的法向辐照度，使无遮挡水平地面得到 `Φ/A_horizontal`；独立 RGB 相机使用中性白光 `Φ/A_horizontal` 再乘表面余弦，两种强度约定有意区分。

这是有界 Lambertian/玻璃混合路径估计器。已支持漫反射后的玻璃继续传播，但尚未估计 Diffuse → Glass → Environment 的逃逸环境贡献。未实现透明 shadow ray、环境焦散、通用无偏 Path Tracer、通用 Photon Mapping、BVH/KD-Tree、体积、GUI 或 GPU。

## 数据与可复现性

CIE 1931 2°/D65 原始 CSV、官方 metadata、DOI、许可和校验和，以及 PBRT/Smits RGB 反射基谱的来源与许可证，见 [assets/color/README.md](assets/color/README.md)。RGB 不能唯一确定真实反射光谱；该反射基谱是有来源的近似模型，不能称为实测光谱。相机当前以 RGB 追踪，不执行逐波长色散。

几何/光学和 XYZ 使用 `f64`。光源固定采样块与顺序合并，相机按像素派生独立 camera/environment/diffuse RNG 流，像素内顺序累积；camera 只负责抖动和首次漫反射前的玻璃，漫反射后的玻璃与其余续追共享 diffuse 流，避免改变后续 SPP 的相机路径及 Caustics；固定配置和种子在当前实现的 1/2/4 线程下结果一致。改变采样块划分可能改变末位浮点舍入；改变算法或依赖版本也不承诺历史逐位输出。

## 验证

```sh
cargo fmt --check
cargo build --locked --all-targets
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
git diff --check
```

测试覆盖光谱积分/颜色与 PNG、双界面 Fresnel/总通量、MIS/PDF/数值积分参考、路径深度和 RR 补偿、玻璃上限、分量可加性、棋盘格、非法输入/溢出、固定种子和线程一致性。清理范围、API 迁移和验收记录见 [ROADMAP.md](ROADMAP.md)。

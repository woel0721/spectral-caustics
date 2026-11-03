# Spectral Caustics Renderer

Rust CPU 离线渲染器，支持理想 Dielectric Sphere/Prism 的光源端光谱焦散，以及 HDRI、MIS、有界 RGB BSDF 路径和 Rayon 并行。默认展示 Glass、Mirror、GGX 三个球体；主要光谱测试场景为 [configs/sphere.json](configs/sphere.json)。当前能力与开发边界见 [ROADMAP.md](ROADMAP.md)。

## 运行与输出

从仓库根目录执行，需要支持 Rust 2024 edition 的工具链：

```sh
cargo run --release --locked
cargo run --release --locked -- configs/sphere.json
cargo run --release --locked -- /path/to/config.json
```

默认读取 [configs/default.json](configs/default.json)，使用随附的 `assets/HDRI/herkulessaulen_4k.hdr`。画面从左到右为玻璃（IOR=1.52）、Mirror（反射率 0.95）、GGX（roughness=0.4，RGB eta=0.2、k=3），地面为棋盘格。配置、HDR 和输出的相对路径均以启动目录为基准。

| 输出 | 内容 |
| --- | --- |
| `render.output_path`，默认 `output/spheres/camera_combined.png` | Combined = Direct + Caustics |
| 同目录的 `camera_direct.png` | 太阳、环境与相机续追贡献 |
| 同目录的 `camera_caustics.png` | 首次地面着色的光谱焦散贡献 |

三图共用一批相机路径。默认三球对照场景没有开启焦散；`sphere.json` 显式选择色散玻璃球，输出到 `output/sphere/`，其中 `camera_caustics.png` 包含球体的光谱焦散，`camera_combined.png` 为合成图。多波长重叠处接近白色，色散主要体现在焦散色边；高亮可能被显示裁剪。自定义主图文件名时只保存指定名称；辅助图与主图重名时加 `aux_` 前缀。PNG 使用同一曝光，经 XYZ → linear sRGB → 裁剪 → display sRGB 输出，生成图位于忽略的 `output/` 中。

## 配置

| 参数 | Rust 内置默认 | 展示 JSON |
| --- | --- | --- |
| `render.width/height` | 512×512 | 768×432 |
| `render.camera_spp` | 64 | 256 |
| `scene.environment_samples` | 1 | 8 |
| `scene.environment_mis_heuristic` | `balance` | `power` |
| `scene.max_diffuse_bounces` | 0 | 4 |
| `render.debug_path` | `false` | `false` |

`Config::default()` 保留单三棱镜实验场景和渐变天空；主程序加载上述 JSON。JSON 省略 `scene.environment` 或设为 `null` 会关闭环境照明，使用常量背景；环境中的 `map_path` 省略/null 时使用渐变，否则加载 HDR。`rotation_degrees` 绕 +Z 旋转，`intensity` 在线性空间缩放。

- `camera_spp` 控制相机采样；`samples_per_pixel_axis` 控制光源每轴分层采样，二者独立。快速预览可同时降低尺寸和采样数。
- `seed` 固定随机种子；`thread_count=null` 使用 Rayon 默认线程数，正整数创建专用池；`max_spectral_blocks` 控制光源采样块数。
- `max_diffuse_bounces` 表示额外连续 BSDF 连接数，包含 Diffuse/Glossy，范围 0..1024；4 最多访问 5 个连续着色顶点。
- 场景采用米制、Z-up；JSON 支持一个地面、至多一个三棱镜及多个球体/圆角三棱镜。Sphere/Prism 的 `caustics` 开关显式选择至多一个理想 Dielectric 焦散目标，不依赖物体下标或 `debug_object_id`。Sphere 缺省关闭；Prism 缺省开启以兼容旧配置，非介质 Prism 仍只遮挡光线。选择 Sphere 时应关闭其他 Prism 的开关；Sphere 开启时绑定非介质材质会报错。
- `receiver` 限制光子图范围，地面在其外扩 `ground_margin_m`，不限制解析太阳照明。
- `render.mode=constant` 使用所选材质的 `ior`；`dispersion` 使用其 Cauchy 模型，未配置模型则仍为常数。波长以 nm 配置，支持 380–780 nm。相机始终以 RGB 和材质的常数 `ior` 追踪。

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
cargo run --release --locked --example diffuse_indirect -- 16
cargo run --release --locked --example camera_parallel_benchmark -- 16 64
cargo run --release --locked --example refract_ray
```

依次演示默认场景三分量、单球四材质对照（Glass/Mirror/GGX 0/0.4）、0/1/4 深度间接光、1/2/4/6 线程 XYZ 一致性，以及单光线双界面折射。`sphere_materials` 使用 debug 构建时还会输出各材质 AOV。

## 实现与物理边界

- 入口流程见 [src/main.rs](src/main.rs)：`Config::load` → `Scene::from_config` → `Render::render_lighting` → `render_camera_glass_components` → `Render::save`。相机只读取光源图的 Caustics；sampled Direct 用于能量诊断，不能与解析太阳重复相加。
- 相机由统一 `BsdfSample` 推进 RGB 吞吐量，Dielectric/Mirror/光滑 GGX 合计最多 32 次 Delta 事件。第三次连续连接后的顶点已着色且仍有预算时执行 RR，`p=clamp(max(throughput),0.05,0.95)`，存活除以 p 补偿。
- 首次表面立即着色；仅首次连续着色顶点为地面时读取焦散图，续追不再读取。已有连续散射历史时，仅最近事件为 Delta 且有环境的 miss 累加环境到 Direct；非 Delta miss 由局部 NEE/MIS 负责。太阳/环境 shadow ray 对玻璃仍采用二值遮挡。
- GGX 为各向同性、单次散射金属，`alpha=roughness²`，使用完整 NDF 采样；无效反射保留零贡献、不补采。`roughness≤0.001` 切换为理想镜面。默认 eta/k 为演示参数，RGB 参数不代表真实光谱，未做微表面多次散射补偿。
- 光源焦散只追踪选中理想介质 Sphere/Prism 的两次透射：首次求交进入后，通过 `Scene::intersect()` 查找同一物体出射面，再求地面交点。折射率由绑定材质查询，两个界面使用同一波长的 `n(λ)`；Snell/Fresnel 函数保持原有数值行为。光子通量乘 `(1-F_enter)(1-F_exit)`，不乘相机 radiance 的 IOR 平方因子；反射、TIR、其他物体遮挡和区域外通量不再分配。D65/CIE XYZ 积分只计一次 `Δλ`，保留原有空间发射归一化与地面双线性 splat。相机三分量 API 要求接收地面为中性 Lambertian；彩色地面可使用独立 RGB 入口 `render_camera_glass`。
- 当前是有界 RGB 路径估计器。尚无透明阴影、嵌套介质、环境/金属焦散、通用 Photon Mapping、BVH/KD-Tree、体积、GUI 或 GPU；有限深度仍会截断路径。

几何/光学与 XYZ 使用 `f64`；固定配置和 seed 在当前实现的 1/2/4 线程下结果一致。模块入口为 [src/render/integrator.rs](src/render/integrator.rs) 和 [src/scene.rs](src/scene.rs)。CIE/D65、PBRT 反射基谱的来源、许可证与校验和见 [颜色数据说明](assets/color/README.md)；RGB 到光谱是近似模型。

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

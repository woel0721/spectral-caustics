# 架构与职责边界

完整模块导航和运行示例复用 [README 的入口说明](../README.md#从入口阅读代码)，能力范围复用 [ROADMAP](../ROADMAP.md)。这里记录职责和重构时应保留的边界。

## 执行链

```text
main: Config::load → Config::build → Renderer::new → Renderer::render
  Photon Integrator（有焦散目标时）→ XYZ 焦散图或 Eλ 图集
  Camera Integrator → Direct / Caustics / Combined 线性 XYZ
  output::save_components → 显示变换与 PNG
```

- [Scene](../src/scene/mod.rs) 只保存物体、材质、光源、环境、背景、可选介质并求交；[Scene::add/intersect](../src/scene/objects.rs) 将 GeometryHit 绑定为含 object_id/material_id 的 Hit。相机配置、路径循环和光子算法不放回 Scene。
- [RenderJob](../src/config/job.rs) 组合 Scene、Camera、相机采样选项、光子接收区/目标绑定和棋盘格；[config/build](../src/config/build.rs) 负责从配置构造这些运行时对象。
- [Renderer](../src/renderer.rs) 持有 RenderSetting 和可选 Rayon 线程池，提供唯一高层 `render(&RenderJob)`，选择 Pipeline 并调度；不实现 BSDF、Fresnel、路径循环或编码。
- [Camera Path](../src/integrator/camera/path.rs) 共享求交、即时着色、续追、预算、RR 和分量归属。[CameraTransport](../src/integrator/camera/transport.rs) 提供 RGB/标量运算及材质/光照适配；[rgb](../src/integrator/camera/rgb.rs) 和 [spectral](../src/integrator/camera/spectral.rs) 保留各自像素/波长采样。
- [Photon generation](../src/integrator/photon/generation.rs) 负责发射、首交点缓存、标量沉积、XYZ 积分和 Eλ 图集；[Photon Path](../src/integrator/photon/path.rs) 负责单条 Forward 路径。无焦散目标时相机不需要图集；正常相机 Pipeline 不生成光子 Direct 预览缓冲。
- [Material](../src/material/mod.rs) 拥有 BSDF 求值、采样、方向 PDF 和事件能力；[optics/interface](../src/optics/interface.rs) 共享 Snell/Fresnel/界面事件，保留 radiance/flux 权重区别。光源与环境提供自己的发射/采样分布，相机 [lighting](../src/integrator/camera/lighting/mod.rs) 负责局部 NEE、MIS、阴影和介质透射率。
- [Framebuffer](../src/film.rs) 保存线性 XYZ；[color](../src/color.rs) 做颜色转换；[output](../src/output.rs) 做曝光、ACES、display sRGB 和 PNG。不能在传输或焦散图中提前做显示变换。

## RGB、Spectral 与光子边界

RGB 相机传输 `Vector3<f64>`，使用材质的常数 IOR，支持 Lambertian、Dielectric、Mirror 和 GGX Metal。Spectral 相机固定当前波长、传输 `f64`；内置材质中支持 Lambertian、Dielectric、Mirror，拒绝未实现光谱接口的材质（包括 GGX Metal）。自定义 Material 也须声明并实现光谱能力；不以 RGB eta/k 插值得到光谱 GGX。

Hero 与四波长 Packet 使用同一个标量路径循环。Packet 共享主射线和路径 RNG 的副本，不共享可变路径游标，也不强制各波长发生相同事件。不要再建立一套 SpectralPathState、独立 RR 或独立环境估计器。

源端 XYZ/Eλ 共用 Forward 内核；Constant 复用几何结果，Dispersion 逐波长追踪。RGB 相机也可接收经过光谱积分的色散焦散图，因此 `camera_mode=rgb` 不代表源端必须使用 Constant。权重及波长积分区别见 [transport.md](transport.md)。

## 当前物理范围

- 这是有限深度、二值几何阴影与地面焦散图结合的估计器，不应描述成通用无偏 Path Tracer。不要顺带加入透明阴影、环境焦散、新材质、通用 Photon Mapping、BVH/KD-Tree、GUI 或 GPU。
- JSON 光子焦散目标限一个 Dielectric Sphere/Prism/VerticalPrism；其他物体阻断。XYZ 图绑定 RGB 相机时要求中性 Lambertian 地面；Eλ 图集支持 Lambertian 地面的波长反射率。图外通量丢弃，不把有限接收区当作整个实体地面。
- [Dielectric](../src/material/dielectric.rs) 按几何法线和方向判断空气/玻璃两侧，外部 IOR 固定为 1；光子有显式 Air/Glass 状态。没有嵌套介质栈，也不支持从玻璃内部启动光子。
- [medium/mod.rs](../src/medium/mod.rs) 只定义 `Medium: Send + Sync` trait 并声明、导出模块；[HomogeneousMedium](../src/medium/homogeneous.rs) 与 [GridMedium](../src/medium/grid.rs) 分别实现局部系数、全局 Majorant、透射率与分段采样。配置及构造在 [config](../src/medium/config.rs)，采样数据在 [types](../src/medium/types.rs)，共享盒边界计算在 [bounds](../src/medium/bounds.rs)，通用介质测试在 [tests.rs](../src/medium/tests.rs)，局部上界与 DDA 测试在 [grid/tests.rs](../src/medium/grid/tests.rs)。细体素精确消光与粗块采样共用私有 [grid/dda.rs](../src/medium/grid/dda.rs) 的步进；细体素路径做编译期专门化，避免粗块索引开销。Camera Integrator 只使用路径段采样、透射率和相函数接口：密度遍历、Majorant、碰撞接受/拒绝与补偿均属于介质内部，不向相机暴露空碰撞或要求类型/算法分支。只读密度/系数/Majorant 查询供检查数据，不参与相机控制流。Scene 持有 `Option<Box<dyn Medium>>`，与 Material 的方式一致。单个轴对齐盒内的无色空间介质掩膜覆盖盒内固体内部；盒边界不折射、不表示 MediumInterface。均匀介质使用指数自由程，网格使用私有局部 Majorant 缓存和 DDA，携带剩余光学深度跨块进行 Delta Tracking；两者返回相同的真实散射样本/补偿权重，并共用 `CameraOptions.max_volume_scatters` 限制多次体散射，独立于表面预算。空碰撞不占预算；达到上限后只计算精确消光，网格使用 DDA，详见 [README](../README.md#非均匀密度网格)。
- 光子不感知雾；现有焦散图只受相机侧介质权重影响，不代表有雾时源端到地面的完整能量。层次化空块跳跃、Ratio Tracking、波长相关介质、介质发光和内外介质切换未实现。

## Diagnostics

[diagnostics](../src/diagnostics/mod.rs) 由显式 feature 控制，Debug/Release 均可开启；普通构建没有诊断类型与缓冲，并拒绝 diagnostics JSON 字段。观察器记录实际相机表面事件，不采样、不计光、不修改 RNG；体散射不进入表面事件计数。

统计 AOV 直接量化，不走物理图像的曝光/XYZ/sRGB 流程。`disable_ground_sun` 是独立的光谱实验选项，会有意跳过地面解析太阳，不能与纯观察器等价。[固定波长实验](../src/diagnostics/wavelength.rs) 返回未乘 CMF/波长权重的 Lλ，并不读取焦散图。已知诊断验证限制见 [testing.md](testing.md#当前验证限制)。

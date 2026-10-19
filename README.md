# Rust 光谱焦散渲染器

一个分阶段学习的 Rust CPU 离线光谱焦散渲染器。第一周目标是计算平行白光穿过三棱镜后落到地面的彩色能量分布，输出地面焦散俯视图。开发范围和验收标准见 [ROADMAP.md](ROADMAP.md)。

## 当前进度

**Day 1 已完成：输出链路与最小 Framebuffer。** 已实现：

- 二维 `Framebuffer`，用 `Vector3<f64>` 保存线性 XYZ，支持带边界检查的读写。
- XYZ → 线性 sRGB 矩阵转换，检查非有限值和计算溢出。
- 单通道线性 RGB → 显示 sRGB 编码，检查输入范围与非有限值。
- 线性 RGB 的显示范围裁剪、sRGB 8 位量化和 PNG 保存，自动创建输出目录。
- Day 1 曾通过 512×512 的 `output/test.png` 位置渐变图验证输出链路。
- Framebuffer、颜色转换与 PNG 读回测试。

**Day 2 进行中：已实现 Ray、地面与单个三角形求交。** `Ray::new` 检查输入并归一化方向，`at(t)` 计算光线上的位置，`intersect_ground()` 返回无限地面 `z = 0` 的交点距离。`Triangle` 实现 `Hittable::intersect` 并返回 `Option<Hit>`；三棱镜组合和地面命中点灰度图尚未实现。

`Ray` 保存 `origin`、单位方向 `direction`、方向各分量的倒数 `direction_inv`、时间参数 `t` 与距离区间 `t_min/t_max`。统一使用 `Ray::new(origin, direction, time)` 构造；Rust 不支持默认函数参数，因此普通光线显式传入 `0.0`。默认距离区间为 `(0, f64::MAX)`。时间字段目前仅保存数值，不模拟传播时间，与 `at(distance)` 的距离参数独立。零方向分量的倒数为 `±∞`，这是允许的缓存值；后续包围盒求交需单独处理平行轴，避免 `0 × ∞` 产生 NaN。

`Hit` 是纯数据记录，保存 `coords`、`tcoords`、`normal`、`emit`、`distance`，以及可选的 `object_id/material_id`。`Hit::default()` 仅提供字段初始值：向量全零、距离为 `f64::MAX`、编号为 `None`。命中状态统一由 `Option<Hit>` 表达：`Some(hit)` 表示命中，`None` 表示未命中。位置计算、法线归一化、合法性检查和正反面判断均由求交算法或调用方负责。`distance` 是距离，与 `Ray.t` 的时间含义独立；`tcoords/emit` 当前仅预留，`emit` 不表示已经实现了光谱发光模型。

物体和材质关联用 `Option<usize>` 保存编号，`None` 对应 C++ 示例中的空指针；后续场景管理负责按编号访问实际对象。`Hittable::intersect(&self, ray: &Ray)` 只接收光线并返回 `Option<Hit>`，具体实现填写命中记录后返回 `Some(hit)`。

`Triangle::new(v0, v1, v2)` 保存三个顶点并缓存两条边和单位几何法线，拒绝非有限顶点、退化三角形及计算溢出。法线按 `(v1 - v0) × (v2 - v0)` 定向，顶点和缓存保持私有，保证二者一致。求交采用双面的 Möller–Trumbore 算法，求解 `origin + t * direction = v0 + u * edge1 + v * edge2`；交点需满足 `u >= 0`、`v >= 0`、`u + v <= 1`，以及光线自带的距离区间 `ray.t_min < distance < ray.t_max`。算法填写 `Hit.coords/normal/distance`，其余预留字段沿用默认值，`Hit` 本身保持纯数据结构。

## 运行

安装 Rust stable 后，在仓库根目录执行：

```bash
cargo run --release --locked
```

当前程序对同一条光线分别查询地面和单个三角形，输出：

```text
归一化后的方向：[[0.0, 0.0, -1.0]]
方向分量的倒数：[[inf, inf, -1.0]]
光线携带的时间：0
是否命中：true
命中地面的距离：2 米
地面交点：[[0.0, 0.0, 0.0]]
地面朝外法线：[[0.0, 0.0, 1.0]]
是否正面命中：true
三角形交点距离：1 米
三角形交点：[[0.0, 0.0, 1.0]]
三角形法线：[[0.0, 0.0, 1.0]]
```

光线从 `(0, 0, 2)` 出发，输入方向 `(0, 0, -5)` 归一化后为 `(0, 0, -1)`。令光线位置的 z 分量为零，得到 `t = -origin.z / direction.z = 2`，再用 `at(t)` 计算完整交点。当前入口不生成 PNG，原有输出函数仍通过 PNG 读回测试验证。

示例三角形的三个顶点为 `(-1, -1, 1)`、`(1, -1, 1)`、`(0, 1, 1)`，法线为 `(0, 0, 1)`，上述光线在距离 `1` 米处命中。两次查询彼此独立；场景中的最近物体选择将在网格/三棱镜组合时实现。

检查格式、运行测试与 Clippy：

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --tests -- -D warnings
```

`Hittable` 已由 `Triangle` 实现。当前 `Hit` 仍有预留字段，因此 Clippy 的测试目标会报告未使用的字段；普通程序还会报告暂未使用的 `framebuffer`、`render` 代码。接回图片输出并使用对应字段后，需重新验证 `cargo clippy --locked -- -D warnings`。

单三角形的 16 项测试涵盖已知交点和法线、斜射、双面命中、顶点顺序、三条边内外与顶点、平行/共面、自相交、光线距离区间、退化输入、非有限值、溢出及小尺寸三角形。

PNG 读回测试在独立临时目录中保存一张 3×2 小图，再解码检查尺寸、RGB8 像素格式和六个位置的红、绿、蓝、白、灰、黑颜色。线性灰色 `0.5` 必须保存为 `[188, 188, 188]`，用于检查 sRGB 编码和量化是否正确。测试成功后清理临时文件。

## 文件与颜色约定

- `src/framebuffer.rs`：二维图像缓冲，图片坐标通过 `y * width + x` 转成数组下标。
- `src/render.rs`：XYZ → 线性 sRGB → sRGB8 → PNG，以及颜色转换和 PNG 读回测试。
- `src/ray.rs`：Ray、方向归一化、沿光线取点、无限地面求交及对应测试。
- `src/hit.rs`：求交结果的数据结构与字段默认值。
- `src/object.rs`：返回 `Option<Hit>` 的统一求交接口。
- `src/triangle.rs`：单三角形、几何法线、Möller–Trumbore 求交及测试。
- `src/main.rs`：输出方向归一化、地面和单三角形求交的演示结果。

XYZ 转换使用 D65 白点约定，参考白的 Y = 1。矩阵来自 [W3C CSS Color 4 的颜色转换示例](https://www.w3.org/TR/css-color-4/#color-conversion-code)，保留负值和大于 1 的线性 RGB 分量。

单通道编码公式来自 [ICC sRGB 编码登记表](https://registry.color.org/rgb-registry/srgb)。编码函数接收 `[0, 1]` 内的有限线性值，非法输入返回 `None`。PNG 导出时先将线性 RGB 逐通道裁剪到 `[0, 1]`，再编码、乘 255 并四舍五入为 `u8`。

## 当前简化与限制

- 地面为双面、无限大的 `z = 0` 平面；方向保持单位长度，返回的 `t` 才表示米。
- 地面求交遵守 `ray.t_min < distance < ray.t_max`，距离区间非法时返回 `None`。手动修改公开的 `direction` 时，应重新归一化并同步 `direction_inv`，或重新构造光线。
- 求交忽略 `t <= 1e-6` 米的交点，以防止自相交；单位方向的 `|direction.z| <= 1e-12` 时视为平行。两个阈值分别作用于距离和方向分量。
- 三角形复用 `1e-6` 米的距离阈值；平行判断使用单位方向与单位法线的点积绝对值，阈值为 `1e-12`，与三角形面积独立。当前尚未组合或验证完整封闭三棱镜的法线朝向。
- 固定曝光为 1.0；不会根据每张图片的最大值自动归一化。
- 显示范围采用逐通道裁剪，可能丢失高亮和色域外的颜色细节；Framebuffer 中的线性 XYZ 数据保留原值。
- Day 1 的 RGB → XYZ 仅用于构造已知测试颜色，尚未实现玻璃折射、光谱积分或物理焦散。

使用 `nalgebra` 进行数学计算，`image` 编解码 PNG。`Cargo.lock` 随项目提交，编译目录 `target/` 和生成图像目录 `output/` 不纳入版本控制。

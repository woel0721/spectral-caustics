# Rust 光谱焦散渲染器

一个分阶段学习的 Rust CPU 离线光谱焦散渲染器。第一周目标是计算平行白光穿过三棱镜后落到地面的彩色能量分布，输出地面焦散俯视图。开发范围和验收标准见 [ROADMAP.md](ROADMAP.md)。

## 当前进度

当前处于 Day 1，已实现：

- 二维 `Framebuffer`，用 `Vector3<f64>` 保存线性 XYZ，支持带边界检查的读写。
- XYZ → 线性 sRGB 矩阵转换，检查非有限值和计算溢出。
- 从 Framebuffer 读取 D65 白点并转换的命令行示例。
- Framebuffer 与颜色转换的单元测试。

下一步是 sRGB 编码和 PNG 导出。当前示例只验证颜色转换，还没有光线追踪、玻璃折射、光谱采样或焦散计算。

## 运行

安装 Rust stable 后，在仓库根目录执行：

```bash
cargo run --locked
```

程序会输出 D65 白点的 XYZ 和转换结果：

```text
XYZ ≈ (0.95046, 1.0, 1.08906)
线性 sRGB ≈ (1.0, 1.0, 1.0)
```

检查格式并运行测试：

```bash
cargo fmt --check
cargo test --locked
```

`Pixel` 类型目前独立保留，尚未使用，因此编译时会出现未使用代码的警告。

## 文件与颜色约定

- `src/framebuffer.rs`：二维图像缓冲，图片坐标通过 `y * width + x` 转成数组下标。
- `src/render.rs`：XYZ → 线性 sRGB 转换。
- `src/main.rs`：D65 白点示例。
- `src/pixel.rs`：尚未整合的类型。

转换使用 D65 白点约定，参考白的 Y = 1。矩阵来自 [W3C CSS Color 4 的颜色转换示例](https://www.w3.org/TR/css-color-4/#color-conversion-code)。当前函数保留负值和大于 1 的线性 RGB 分量；显示范围处理、sRGB 编码和 8 位量化会在后续 PNG 输出阶段实现。

使用 `nalgebra` 进行数学计算，`image` 已加入 PNG 依赖。`Cargo.lock` 随项目提交，编译目录 `target/` 和生成图像目录 `output/` 不纳入版本控制。

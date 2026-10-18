# Rust 光谱焦散渲染器

一个分阶段学习的 Rust CPU 离线光谱焦散渲染器。第一周目标是计算平行白光穿过三棱镜后落到地面的彩色能量分布，输出地面焦散俯视图。开发范围和验收标准见 [ROADMAP.md](ROADMAP.md)。

## 当前进度

**Day 1 已完成：输出链路与最小 Framebuffer。** 已实现：

- 二维 `Framebuffer`，用 `Vector3<f64>` 保存线性 XYZ，支持带边界检查的读写。
- XYZ → 线性 sRGB 矩阵转换，检查非有限值和计算溢出。
- 单通道线性 RGB → 显示 sRGB 编码，检查输入范围与非有限值。
- 线性 RGB 的显示范围裁剪、sRGB 8 位量化和 PNG 保存，自动创建输出目录。
- 根据像素位置生成 512×512 渐变测试图，保存到 `output/test.png`。
- Framebuffer、颜色转换与 PNG 读回测试。

下一阶段是 Day 2：光线、地面与三棱镜求交。当前图片用于验证输出链路，光线追踪、玻璃折射、光谱采样和焦散计算将在后续阶段实现。

## 运行

安装 Rust stable 后，在仓库根目录执行：

```bash
cargo run --release --locked
```

程序会生成 PNG，并输出：

```text
已保存 512×512 渐变测试图：output/test.png
```

图片从左向右增加红色，从上向下增加绿色，线性蓝色通道固定为 0.25。图片左上角是 `(0, 0)`，像素位置先归一化为 `[0, 1]`，再用于构造测试颜色。

检查格式、运行测试与 Clippy：

```bash
cargo fmt --check
cargo test --locked
cargo clippy -- -D warnings
```

PNG 读回测试在独立临时目录中保存一张 3×2 小图，再解码检查尺寸、RGB8 像素格式和六个位置的红、绿、蓝、白、灰、黑颜色。线性灰色 `0.5` 必须保存为 `[188, 188, 188]`，用于检查 sRGB 编码和量化是否正确。测试成功后清理临时文件。

## 文件与颜色约定

- `src/framebuffer.rs`：二维图像缓冲，图片坐标通过 `y * width + x` 转成数组下标。
- `src/render.rs`：XYZ → 线性 sRGB → sRGB8 → PNG，以及颜色转换和 PNG 读回测试。
- `src/main.rs`：位置渐变测试图；先用线性 RGB 定义颜色，再转成 XYZ 写入缓冲区。

XYZ 转换使用 D65 白点约定，参考白的 Y = 1。矩阵来自 [W3C CSS Color 4 的颜色转换示例](https://www.w3.org/TR/css-color-4/#color-conversion-code)，保留负值和大于 1 的线性 RGB 分量。

单通道编码公式来自 [ICC sRGB 编码登记表](https://registry.color.org/rgb-registry/srgb)。编码函数接收 `[0, 1]` 内的有限线性值，非法输入返回 `None`。PNG 导出时先将线性 RGB 逐通道裁剪到 `[0, 1]`，再编码、乘 255 并四舍五入为 `u8`。

## 当前简化与限制

- 固定曝光为 1.0；不会根据每张图片的最大值自动归一化。
- 显示范围采用逐通道裁剪，可能丢失高亮和色域外的颜色细节；Framebuffer 中的线性 XYZ 数据保留原值。
- 当前颜色由图片坐标生成。RGB → XYZ 仅用于构造已知测试颜色，尚未实现光谱积分或物理焦散。

使用 `nalgebra` 进行数学计算，`image` 编解码 PNG。`Cargo.lock` 随项目提交，编译目录 `target/` 和生成图像目录 `output/` 不纳入版本控制。

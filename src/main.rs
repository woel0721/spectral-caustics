//! 位置渐变测试图：图片坐标 -> 线性 RGB 测试颜色 -> XYZ 缓冲 -> sRGB PNG。
//! 用已知测试颜色验证图片输出链路；光谱与折射计算在后续阶段实现。

mod framebuffer;
mod render;

// use 将模块中的类型引入当前作用域，后面可以直接写 Framebuffer、Vector3。
use framebuffer::Framebuffer;
use nalgebra::{Matrix3, Vector3};

fn main() {
    let width = 512;
    let height = 512;

    let mut framebuffer = Framebuffer::new(width, height);

    // 为了直观控制红、绿通道，先用线性 RGB 定义测试颜色。
    // Framebuffer 保存 XYZ，因此用此矩阵转换后再写入；导出时会转换回线性 RGB。
    // 来源：W3C CSS Color 4 的 lin_sRGB_to_XYZ（D65 白点）。
    // https://www.w3.org/TR/css-color-4/#color-conversion-code
    let rgb_to_xyz = Matrix3::new(
        506752.0 / 1228815.0,
        87881.0 / 245763.0,
        12673.0 / 70218.0,
        87098.0 / 409605.0,
        175762.0 / 245763.0,
        12673.0 / 175545.0,
        7918.0 / 409605.0,
        87881.0 / 737289.0,
        1001167.0 / 1053270.0,
    );

    // 坐标范围是 0..=width-1，归一化后两端恰好映射为 0 和 1。
    // 若某个方向只有一个像素，分母取 1，使对应坐标为 0，避免除零。
    let x_span = (width - 1).max(1) as f64;
    let y_span = (height - 1).max(1) as f64;

    for y in 0..height {
        for x in 0..width {
            // 先转成 f64 再相除，得到浮点比例，避免整数除法。
            let u = x as f64 / x_span;
            let v = y as f64 / y_span;

            // 从左向右红色增加，从上向下绿色增加，蓝色固定为 0.25。
            let linear_rgb = Vector3::new(u, v, 0.25);
            let xyz = rgb_to_xyz * linear_rgb;
            assert!(framebuffer.set(x, y, xyz));
        }
    }
    render::save_png(&framebuffer, "output/test.png").expect("保存渐变 PNG 失败");
    println!("已保存 {width}×{height} 渐变测试图：output/test.png");
}

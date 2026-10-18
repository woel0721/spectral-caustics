mod framebuffer;
mod pixel;
mod render;

use framebuffer::Framebuffer;
use nalgebra::Vector3;

fn main() {
    let mut framebuffer = Framebuffer::new(1, 1);
    let white_xyz = Vector3::new(0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290);
    assert!(framebuffer.set(0, 0, white_xyz));

    let xyz = *framebuffer.get(0, 0).expect("像素坐标应当有效");
    let linear_rgb = render::xyz_to_linear_srgb(xyz).expect("颜色转换结果应当有限");
    println!("D65 白点 XYZ：{xyz:?}");
    println!("线性 sRGB：{linear_rgb:?}");
}

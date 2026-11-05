//! 生成用于玻璃色散测试的线性 equirectangular HDRI；不做曝光、色调映射或 sRGB 编码。

use image::Rgb;
use image::codecs::hdr::HdrEncoder;
use std::error::Error;
use std::fs;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;

const WIDTH: usize = 2048;
const HEIGHT: usize = 1024;
const PANEL_U_CENTER: f64 = 0.5;
const PANEL_V_CENTER: f64 = 0.5;
const PANEL_U_HALF: f64 = 0.18;
const PANEL_V_HALF: f64 = 0.14;
// 12 条交替竖条，总共 6 条白色、6 条黑色。
const STRIPE_COUNT: usize = 12;
const BACKGROUND: f32 = 0.12;
const WHITE: f32 = 6.0;
const BLACK: f32 = 0.01;

#[derive(Clone, Copy)]
enum Pattern {
    Stripes,
    SingleLightBar,
}

fn pixel_value(u: f64, v: f64, pattern: Pattern) -> f32 {
    if (u - PANEL_U_CENTER).abs() >= PANEL_U_HALF || (v - PANEL_V_CENTER).abs() >= PANEL_V_HALF {
        return BACKGROUND;
    }
    let is_white = match pattern {
        Pattern::Stripes => {
            let panel_u = (u - (PANEL_U_CENTER - PANEL_U_HALF)) / (2.0 * PANEL_U_HALF);
            let stripe = ((panel_u * STRIPE_COUNT as f64) as usize).min(STRIPE_COUNT - 1);
            stripe.is_multiple_of(2)
        }
        // 黑色面板中央放一条与普通白条等宽的竖直灯带。
        Pattern::SingleLightBar => (u - PANEL_U_CENTER).abs() < PANEL_U_HALF / STRIPE_COUNT as f64,
    };
    if is_white { WHITE } else { BLACK }
}

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let (pattern, output) = match args.next().as_deref() {
        None | Some("stripes") => (Pattern::Stripes, "assets/HDRI/dispersion_stripes.hdr"),
        Some("single_light_bar") => (
            Pattern::SingleLightBar,
            "assets/HDRI/dispersion_single_light_bar.hdr",
        ),
        Some(other) => {
            return Err(format!("未知模式 {other:?}；仅支持 stripes / single_light_bar").into());
        }
    };
    if args.next().is_some() {
        return Err("用法：generate_test_hdri [stripes|single_light_bar]".into());
    }

    let count = WIDTH.checked_mul(HEIGHT).ok_or("HDRI 像素数量溢出")?;
    let mut pixels = Vec::with_capacity(count);
    for y in 0..HEIGHT {
        let v = (y as f64 + 0.5) / HEIGHT as f64;
        for x in 0..WIDTH {
            let u = (x as f64 + 0.5) / WIDTH as f64;
            pixels.push(Rgb([pixel_value(u, v, pattern); 3]));
        }
    }
    let path = Path::new(output);
    fs::create_dir_all(path.parent().ok_or("输出路径缺少目录")?)?;
    let mut writer = BufWriter::new(fs::File::create(path)?);
    HdrEncoder::new(&mut writer).encode(&pixels, WIDTH, HEIGHT)?;
    writer.flush()?;
    println!(
        "已生成 {}（{}×{}，线性 RGB：背景 {}，白 {}，黑 {}）",
        path.display(),
        WIDTH,
        HEIGHT,
        BACKGROUND,
        WHITE,
        BLACK
    );
    Ok(())
}

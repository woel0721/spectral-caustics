//! 固定 450/550/650 nm、共享主射线和 CRN，测量球内灯带的图像坐标。
use nalgebra::Vector3;
use serde::Serialize;
use spectral_caustics::color::linear_srgb_to_xyz;
use spectral_caustics::config::Config;
use spectral_caustics::diagnostics::render_fixed_wavelength_camera;
use spectral_caustics::film::Framebuffer;
use spectral_caustics::spectrum::emission::d65_spectrum;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

const WAVELENGTHS_NM: [f64; 3] = [450.0, 550.0, 650.0];

#[derive(Clone, Copy, Debug, Serialize)]
struct Projection {
    y_px: f64,
    left_px: f64,
    right_px: f64,
    center_px: f64,
    width_px: f64,
}

/// 半高边界取相邻像素中心之间的线性插值；优先取最长的亮区，避免小反射斑。
fn locate_bar(row: &[f64], start: usize, end: usize, y: usize) -> Option<Projection> {
    if end <= start + 2 || end > row.len() {
        return None;
    }
    let region = &row[start..end];
    let low = region.iter().copied().fold(f64::INFINITY, f64::min);
    let high = region.iter().copied().fold(0.0, f64::max);
    // 本工具配合亮度 6 的诊断灯带；排除暗背景/球面反射的微小随机峰。
    if !low.is_finite() || !high.is_finite() || high < 1.0 || high - low < 0.5 {
        return None;
    }
    let half = (low + high) * 0.5;
    let mut best = (0, 0);
    let mut x = start;
    while x < end {
        if row[x] < half {
            x += 1;
            continue;
        }
        let left = x;
        while x < end && row[x] >= half {
            x += 1;
        }
        if x - left > best.1 - best.0 {
            best = (left, x);
        }
    }
    let (left, end_run) = best;
    if end_run <= left + 1 || left == start || end_run == end {
        return None;
    }
    let right = end_run - 1;
    let left_px = left as f64 - 0.5 + (half - row[left - 1]) / (row[left] - row[left - 1]);
    let right_px = right as f64 + 0.5 + (half - row[right]) / (row[right + 1] - row[right]);
    Some(Projection {
        y_px: y as f64 + 0.5,
        left_px,
        right_px,
        center_px: (left_px + right_px) * 0.5,
        width_px: right_px - left_px,
    })
}

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() > 4 {
        return Err(
            "用法：spectral_wavelength_projection [每波长SPP] [边长] [输出目录] [配置路径]".into(),
        );
    }
    let mut config = Config::load(
        args.get(3)
            .map(String::as_str)
            .unwrap_or("configs/sphere_spectral.json"),
    )?;
    if let Some(spp) = args.first() {
        config.render.camera_spp = spp.parse()?;
    }
    if let Some(size) = args.get(1) {
        config.render.width = size.parse()?;
        config.render.height = config.render.width;
    }
    let directory = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| "output/fixed_wavelength_projection".into());
    fs::create_dir_all(&directory)?;
    config.save(directory.join("config.json"))?;
    let scene = config.build()?;
    let object_id = scene
        .photon
        .caustics_object_id
        .ok_or("请显式选择一个玻璃球 caustics 目标")?;
    let material = scene
        .scene
        .material(scene.scene.objects[object_id].material_id());
    let indices: Vec<_> = WAVELENGTHS_NM
        .iter()
        .map(|&nm| {
            material
                .photon_ior(Some(nm), config.render.mode)
                .ok_or("目标没有折射率")
        })
        .collect::<Result<_, _>>()?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(
            config
                .render
                .thread_count
                .unwrap_or_else(rayon::current_num_threads),
        )
        .build()?;
    println!(
        "固定波长相机：{}×{}，每波长 {} SPP，seed={}，{} 线程",
        config.render.width,
        config.render.height,
        config.render.camera_spp,
        config.render.seed,
        pool.current_num_threads()
    );
    let start = Instant::now();
    let images = pool.install(|| {
        render_fixed_wavelength_camera(
            &scene,
            config.render.width,
            config.render.height,
            config.render.camera_spp,
            config.render.seed,
            config.render.mode,
            &WAVELENGTHS_NM,
        )
        .map_err(|e| e.to_string())
    })?;
    println!(
        "三张固定波长图已完成，{:.3}s",
        start.elapsed().as_secs_f64()
    );
    let width = config.render.width;
    let height = config.render.height;
    let mut normalized = Vec::new();
    for image in &images {
        let source = d65_spectrum(image.wavelength_nm).ok_or("D65 查询失败")?;
        let values: Vec<_> = image.radiance.iter().map(|v| v / source).collect();
        let mut gray = Framebuffer::new(width, height)?;
        for (dst, &value) in gray.pixels_mut().iter_mut().zip(&values) {
            *dst = linear_srgb_to_xyz(Vector3::repeat(value)).ok_or("灰度转换失败")?;
        }
        spectral_caustics::output::save_png_with_exposure(
            &gray,
            directory.join(format!("camera_{:.0}nm.png", image.wavelength_nm)),
            config.render.exposure,
        )?;
        normalized.push(values);
    }
    // 伪彩叠加 R=650/G=550/B=450，专用于对比位置，不是全光谱积分结果。
    let mut overlay = Framebuffer::new(width, height)?;
    for (i, pixel) in overlay.pixels_mut().iter_mut().enumerate() {
        *pixel = linear_srgb_to_xyz(Vector3::new(
            normalized[2][i],
            normalized[1][i],
            normalized[0][i],
        ))
        .ok_or("叠加转换失败")?;
    }
    spectral_caustics::output::save_png_with_exposure(
        &overlay,
        directory.join("overlay_650_550_450.png"),
        config.render.exposure,
    )?;

    // 用真实主射线命中范围排除直接背景灯带；再裁掉球体两侧 20% 的反射边缘。
    let mask: Vec<_> = (0..width * height)
        .map(|i| {
            let ray = scene
                .camera
                .ray_for_pixel(i % width, i / width, width, height)
                .unwrap();
            scene
                .scene
                .intersect(&ray)
                .is_some_and(|hit| hit.object_id == (object_id))
        })
        .collect();
    let top = mask
        .chunks_exact(width)
        .position(|row| row.iter().any(|v| *v))
        .ok_or("球体不在画面中")?;
    let bottom = mask
        .chunks_exact(width)
        .rposition(|row| row.iter().any(|v| *v))
        .unwrap();
    let central_row = top + (bottom - top).div_ceil(2);
    let half_band = ((bottom - top + 1) / 8).max(1);
    let mut projections = vec![Vec::new(); 3];
    for y in central_row.saturating_sub(half_band)..=(central_row + half_band).min(height - 1) {
        let row_mask = &mask[y * width..(y + 1) * width];
        let Some(left) = row_mask.iter().position(|v| *v) else {
            continue;
        };
        let right = row_mask.iter().rposition(|v| *v).unwrap() + 1;
        let margin = (right - left) / 5;
        for (lane, values) in normalized.iter().enumerate() {
            if let Some(p) = locate_bar(
                &values[y * width..(y + 1) * width],
                left + margin,
                right - margin,
                y,
            ) {
                projections[lane].push(p);
            }
        }
    }
    let mut profile = fs::File::create(directory.join("central_profiles.csv"))?;
    writeln!(profile, "x_px,L450_over_D65,L550_over_D65,L650_over_D65")?;
    for x in 0..width {
        let i = central_row * width + x;
        writeln!(
            profile,
            "{},{},{},{}",
            x as f64 + 0.5,
            normalized[0][i],
            normalized[1][i],
            normalized[2][i]
        )?;
    }
    let central: Vec<_> = projections
        .iter()
        .map(|rows| {
            rows.iter()
                .find(|p| p.y_px == central_row as f64 + 0.5)
                .copied()
                .ok_or("中央扫描线未找到折射灯带，请调整 HDRI 角度")
        })
        .collect::<Result<_, _>>()?;
    let mut csv = fs::File::create(directory.join("projection_positions.csv"))?;
    writeln!(
        csv,
        "wavelength_nm,y_px,left_px,right_px,center_px,width_px,center_shift_vs_550_px"
    )?;
    for (lane, rows) in projections.iter().enumerate() {
        for p in rows {
            let shift = projections[1]
                .iter()
                .find(|q| q.y_px == p.y_px)
                .map(|q| p.center_px - q.center_px);
            writeln!(
                csv,
                "{},{},{},{},{},{},{}",
                WAVELENGTHS_NM[lane],
                p.y_px,
                p.left_px,
                p.right_px,
                p.center_px,
                p.width_px,
                shift.map(|v| v.to_string()).unwrap_or_default()
            )?;
        }
    }
    let mut summaries = Vec::new();
    for lane in 0..3 {
        let shift = central[lane].center_px - central[1].center_px;
        println!(
            "{:.0} nm: n={:.9}, y={:.1}, left={:.4}, right={:.4}, center={:.4}, shift550={:+.4} px",
            WAVELENGTHS_NM[lane],
            indices[lane],
            central[lane].y_px,
            central[lane].left_px,
            central[lane].right_px,
            central[lane].center_px,
            shift
        );
        summaries.push(
            serde_json::json!({"wavelength_nm":WAVELENGTHS_NM[lane],"ior":indices[lane],
            "projection":central[lane],"center_shift_vs_550_px":shift}),
        );
    }
    let report = serde_json::json!({
        "width":width,"height":height,"spatial_spp_per_wavelength":config.render.camera_spp,
        "seed":config.render.seed,"ior_mode":config.render.mode,"wavelengths_nm":WAVELENGTHS_NM,
        "shared_primary_rays":width.checked_mul(height).and_then(|n| n.checked_mul(config.render.camera_spp)).ok_or("光线数量溢出")?,
        "coordinates":"pixel-edge coordinates, top-left origin; pixel center=(x+0.5,y+0.5)",
        "measurement":"longest bright run inside central 60% of the primary glass hit mask; linearly interpolated half-height edges; center=(left+right)/2; central quarter of sphere height",
        "display":"grayscale L_lambda/D65(lambda), same exposure/ACES; overlay R=650 G=550 B=450 is false color",
        "measurements":summaries,
    });
    fs::write(
        directory.join("projection.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interpolated_half_height_position_ignores_short_reflection_peak() {
        let row = [0.0, 0.0, 1.0, 5.0, 5.0, 5.0, 1.0, 0.0, 0.0, 5.0, 0.0];
        let p = locate_bar(&row, 0, row.len(), 10).unwrap();
        assert_eq!(p.y_px, 10.5);
        assert_eq!(p.left_px, 2.875);
        assert_eq!(p.right_px, 6.125);
        assert_eq!(p.center_px, 4.5);
    }
    #[test]
    fn dark_or_clipped_profiles_do_not_fabricate_a_position() {
        assert!(locate_bar(&[0.1; 12], 0, 12, 0).is_none());
        assert!(locate_bar(&[6.0; 12], 0, 12, 0).is_none());
        assert!(locate_bar(&[0.0, 6.0, 6.0, 6.0, 0.0], 1, 4, 0).is_none());
    }
}

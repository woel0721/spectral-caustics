//! Release experiment for the spectral procedural fog scene.
use nalgebra::Vector3;
use spectral_caustics::{
    config::{Config, RenderJob},
    diagnostics::render_fixed_wavelength_camera,
    film::Framebuffer,
    geometry::ray::Ray,
    medium::MediumConfig,
    output,
    renderer::Renderer,
    spectrum::cie_xyz,
};
use std::{
    error::Error,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};

type ExperimentResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const SCATTER_LIMITS: [usize; 4] = [0, 1, 4, 8];
const CAMERA_MODES: [(&str, usize); 2] = [("hero", 1), ("packet", 4)];
const VERIFY_WIDTH: usize = 48;
const VERIFY_HEIGHT: usize = 32;
const VERIFY_SPP: usize = 4096;
const REFERENCE_SPP: usize = 128;
const SEEDS: std::ops::Range<u64> = 42..50;
// Half-open pixel rectangles at the verification resolution, origin at top left.
const REGIONS: [(&str, [usize; 4]); 3] = [
    ("fog_core", [20, 28, 8, 15]),
    ("ground_through_fog", [21, 27, 18, 21]),
    ("clear_ground", [4, 13, 22, 28]),
];

fn write_transmittance(config: &Config, directory: &Path) -> ExperimentResult<()> {
    let Some(MediumConfig::Procedural(grid)) = &config.scene.medium else {
        return Err("expected the spectral procedural fog configuration".into());
    };
    let job = config.build()?;
    let medium = job.scene.medium.as_deref().ok_or("missing fog")?;
    let dy = (grid.max_m[1] - grid.min_m[1]) / grid.resolution[1] as f64;
    let mut csv = BufWriter::new(File::create(directory.join("transmittance.csv"))?);
    writeln!(
        csv,
        "probe,x_m,z_m,wavelength_nm,sigma_a_base,sigma_s_base,column_density_m,expected,actual,abs_error"
    )?;
    let mut max_error: f64 = 0.0;
    for (probe, x, z) in [
        ("core", 0.0, 0.81),
        ("edge", 0.70, 0.81),
        ("outside", 0.81, 0.81),
    ] {
        let column_density = (0..grid.resolution[1])
            .map(|i| Vector3::new(x, grid.min_m[1] + (i as f64 + 0.5) * dy, z))
            .map(|point| medium.density_at(point) * dy)
            .sum::<f64>();
        let ray = Ray::new(Vector3::new(x, -2.0, z), Vector3::y()).ok_or("invalid probe ray")?;
        let mut previous = 0.0;
        // Independently specified knot and midpoint values for the scene's two curves.
        for (nm, absorption, scattering) in [
            (380.0, 0.04, 2.40),
            (450.0, 0.05, 2.10),
            (500.0, 0.10, 1.55),
            (550.0, 0.15, 1.00),
            (600.0, 0.25, 0.675),
            (650.0, 0.35, 0.35),
            (780.0, 0.45, 0.15),
        ] {
            let position = Vector3::new(0.0, 0.0, 0.81);
            let density = medium.density_at(position);
            let local = medium.coefficients_at(position, nm);
            if density <= 0.0
                || (local.sigma_a / density - absorption).abs() > 1e-12
                || (local.sigma_s / density - scattering).abs() > 1e-12
            {
                return Err(
                    format!("absorption or scattering coefficient mismatch at {nm} nm").into(),
                );
            }
            let expected = (-column_density * (absorption + scattering)).exp();
            let actual = medium.transmittance_ray(&ray, 4.0, nm);
            let error = (actual - expected).abs();
            if !error.is_finite()
                || error > 1e-12
                || (probe == "outside" && actual != 1.0)
                || (probe != "outside" && !(actual > previous && actual < 1.0))
            {
                return Err(
                    format!("{probe} at {nm} nm: expected {expected}, got {actual}").into(),
                );
            }
            max_error = max_error.max(error);
            previous = actual;
            writeln!(
                csv,
                "{probe},{x},{z},{nm},{absorption},{scattering},{column_density},{expected},{actual},{error}"
            )?;
        }
    }
    csv.flush()?;
    println!("Transmittance passed (21 measurements, max abs error {max_error:.3e})");
    Ok(())
}

fn render_images(mut config: Config, directory: &Path) -> ExperimentResult<()> {
    for (name, packet_size) in CAMERA_MODES {
        config.render.spectral_packet_size = packet_size;
        let renderer = Renderer::new(config.render.clone())?;
        for limit in SCATTER_LIMITS {
            config.scene.max_volume_scatters = limit;
            save_image(
                &mut config,
                &renderer,
                &directory.join(format!("{name}_{limit}")),
            )?;
        }
    }
    config.scene.medium = None;
    config.scene.max_volume_scatters = 0;
    config.render.spectral_packet_size = 1;
    let renderer = Renderer::new(config.render.clone())?;
    save_image(&mut config, &renderer, &directory.join("vacuum"))
}

fn save_image(config: &mut Config, renderer: &Renderer, directory: &Path) -> ExperimentResult<()> {
    let start = Instant::now();
    config.render.output_path = directory.join("camera_combined.png");
    let image = renderer.render(&config.build()?)?.combined;
    output::save_png_with_exposure(&image, &config.render.output_path, config.render.exposure)?;
    config.save(directory.join("config.json"))?;
    println!(
        "{} ({:.2}s)",
        directory.display(),
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

fn region_pixels() -> [Vec<usize>; REGIONS.len()] {
    REGIONS.map(|(_, [left, right, top, bottom])| {
        (top..bottom)
            .flat_map(|y| (left..right).map(move |x| y * VERIFY_WIDTH + x))
            .collect()
    })
}

fn region_mean(colors: &Framebuffer, indices: &[usize]) -> Vector3<f64> {
    indices
        .iter()
        .map(|&i| colors.pixels()[i])
        .sum::<Vector3<f64>>()
        / indices.len() as f64
}

fn fixed_wavelength_colors(
    config: &Config,
    job: &RenderJob,
    regions: &[Vec<usize>; REGIONS.len()],
) -> ExperimentResult<[[Vector3<f64>; REGIONS.len()]; 2]> {
    let wavelengths: Vec<_> = (0..=160).map(|i| 380.0 + i as f64 * 2.5).collect();
    let images = render_fixed_wavelength_camera(
        job,
        config.render.width,
        config.render.height,
        REFERENCE_SPP,
        config.render.seed,
        config.render.mode,
        &wavelengths,
    )?;
    let mut reference = [[Vector3::zeros(); REGIONS.len()]; 2];
    for (i, image) in images.iter().enumerate() {
        let cmf = cie_xyz(image.wavelength_nm).ok_or("missing CIE color matching functions")?;
        let endpoint_weight = if i == 0 || i == wavelengths.len() - 1 {
            0.5
        } else {
            1.0
        };
        for (r, indices) in regions.iter().enumerate() {
            let mean_radiance =
                indices.iter().map(|&p| image.radiance[p]).sum::<f64>() / indices.len() as f64;
            // L_lambda is per nm and unweighted. Integrate it against the CMFs in nm.
            let integrand = cmf * (mean_radiance * endpoint_weight);
            reference[1][r] += integrand * 2.5;
            if i % 2 == 0 {
                reference[0][r] += integrand * 5.0;
            }
        }
    }
    Ok(reference)
}

fn write_color(
    csv: &mut impl Write,
    limit: usize,
    seed: u64,
    method: &str,
    region: &str,
    xyz: Vector3<f64>,
) -> std::io::Result<()> {
    let sum = xyz.sum();
    let (x, y) = if sum > 0.0 {
        ((xyz.x / sum).to_string(), (xyz.y / sum).to_string())
    } else {
        (String::new(), String::new())
    };
    writeln!(
        csv,
        "{limit},{seed},{method},{region},{},{},{},{x},{y}",
        xyz.x, xyz.y, xyz.z
    )
}

fn verify_colors(mut config: Config, directory: &Path) -> ExperimentResult<()> {
    config.render.width = VERIFY_WIDTH;
    config.render.height = VERIFY_HEIGHT;
    config.render.camera_spp = VERIFY_SPP;
    let regions = region_pixels();
    let mut csv = BufWriter::new(File::create(directory.join("colors.csv"))?);
    writeln!(csv, "scatter_limit,seed,method,region,X,Y,Z,x,y")?;

    let mut vacuum = config.clone();
    vacuum.scene.medium = None;
    vacuum.scene.max_volume_scatters = 0;
    let vacuum_job = vacuum.build()?;

    for limit in SCATTER_LIMITS {
        config.scene.max_volume_scatters = limit;
        let job = config.build()?;
        let start = Instant::now();
        for seed in SEEDS {
            config.render.seed = seed;
            let [coarse, fine] = fixed_wavelength_colors(&config, &job, &regions)?;
            for (method, colors) in [("reference_5nm", coarse), ("reference_2.5nm", fine)] {
                for ((region, _), xyz) in REGIONS.into_iter().zip(colors) {
                    write_color(&mut csv, limit, seed, method, region, xyz)?;
                }
            }
            for (method, packet_size) in CAMERA_MODES {
                config.render.spectral_packet_size = packet_size;
                let renderer = Renderer::new(config.render.clone())?;
                let colors = renderer.render(&job)?.combined;
                for ((region, _), indices) in REGIONS.iter().zip(&regions) {
                    write_color(
                        &mut csv,
                        limit,
                        seed,
                        method,
                        region,
                        region_mean(&colors, indices),
                    )?;
                }
                if limit == 0 && packet_size == 1 {
                    let colors = renderer.render(&vacuum_job)?.combined;
                    for ((region, _), indices) in REGIONS.iter().zip(&regions) {
                        write_color(
                            &mut csv,
                            0,
                            seed,
                            "vacuum_hero",
                            region,
                            region_mean(&colors, indices),
                        )?;
                    }
                }
            }
            csv.flush()?;
        }
        println!(
            "Color measurements for {limit} scatters ({:.2}s)",
            start.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

fn main() -> ExperimentResult<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [command, directory] = args.as_slice() else {
        return Err("usage: spectral_fog_compare <verify|render> <output_dir>".into());
    };
    let config = Config::load("configs/spectral_procedural_fog.json")?;
    let directory = PathBuf::from(directory);
    fs::create_dir_all(&directory)?;
    match command.as_str() {
        "verify" => {
            write_transmittance(&config, &directory)?;
            verify_colors(config, &directory)
        }
        "render" => render_images(config, &directory),
        _ => Err("expected verify or render".into()),
    }
}

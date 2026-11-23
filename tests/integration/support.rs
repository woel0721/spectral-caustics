//! Helpers shared by integration tests.

use spectral_caustics::config::material::MaterialConfig;
use spectral_caustics::film::Framebuffer;
use spectral_caustics::integrator::camera::CameraLightingImages;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[track_caller]
pub fn assert_framebuffer_bits_eq(actual: &Framebuffer, expected: &Framebuffer) {
    assert_eq!(
        (actual.width(), actual.height()),
        (expected.width(), expected.height()),
        "framebuffer dimensions differ"
    );
    assert_eq!(actual.pixels().len(), expected.pixels().len());
    for (index, (actual, expected)) in actual.pixels().iter().zip(expected.pixels()).enumerate() {
        assert_eq!(
            actual.map(f64::to_bits),
            expected.map(f64::to_bits),
            "XYZ pixel {index} differs"
        );
    }
}

pub fn assert_camera_images_bits_eq(
    actual: &CameraLightingImages,
    expected: &CameraLightingImages,
) {
    assert_framebuffer_bits_eq(&actual.direct, &expected.direct);
    assert_framebuffer_bits_eq(&actual.caustics, &expected.caustics);
    assert_framebuffer_bits_eq(&actual.combined, &expected.combined);
}

pub fn run_in_threads<T: Send>(threads: usize, work: impl FnOnce() -> T + Send) -> T {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(work)
}

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(prefix: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.0.join(name)
    }

    pub fn write_hdr(
        &self,
        name: &str,
        width: usize,
        height: usize,
        pixels: &[[f32; 3]],
    ) -> PathBuf {
        let path = self.0.join(name);
        let encoded: Vec<_> = pixels.iter().copied().map(image::Rgb).collect();
        image::codecs::hdr::HdrEncoder::new(fs::File::create(&path).unwrap())
            .encode(&encoded, width, height)
            .unwrap();
        path
    }

    pub fn write_rgbe_4x2_hdr(
        &self,
        name: &str,
        uniform: bool,
        double_bottom_row: bool,
    ) -> PathBuf {
        let path = self.0.join(name);
        let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n".to_vec();
        for row in 0..2 {
            for mut pixel in [
                [128, 64, 32, 130],
                [64, 128, 32, 131],
                [32, 64, 128, 132],
                [128, 32, 64, 133],
            ] {
                if uniform {
                    pixel = [128, 64, 32, 132];
                } else if double_bottom_row {
                    pixel[3] += row;
                }
                bytes.extend_from_slice(&pixel);
            }
        }
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn test_materials() -> [MaterialConfig; 4] {
    [
        MaterialConfig::Dielectric {
            ior: 1.52,
            dispersion: None,
        },
        MaterialConfig::Mirror {
            reflectance: [0.95; 3],
        },
        MaterialConfig::GgxMetal {
            eta: [0.2; 3],
            k: [3.0; 3],
            roughness: 0.0,
        },
        MaterialConfig::GgxMetal {
            eta: [0.2; 3],
            k: [3.0; 3],
            roughness: 0.4,
        },
    ]
}

pub struct TransmissionRng;
impl rand::RngCore for TransmissionRng {
    fn next_u64(&mut self) -> u64 {
        u64::MAX
    }
    fn next_u32(&mut self) -> u32 {
        u32::MAX
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        dest.fill(255);
    }
}

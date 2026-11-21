//! Helpers shared by integration tests.

use spectral_caustics::film::Framebuffer;
use spectral_caustics::integrator::camera::CameraLightingImages;

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

// Each integration test builds its own copy of this module.
#[allow(dead_code)]
pub fn assert_camera_images_bits_eq(
    actual: &CameraLightingImages,
    expected: &CameraLightingImages,
) {
    assert_framebuffer_bits_eq(&actual.direct, &expected.direct);
    assert_framebuffer_bits_eq(&actual.caustics, &expected.caustics);
    assert_framebuffer_bits_eq(&actual.combined, &expected.combined);
}

#[allow(dead_code)]
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

//! Controlled event samples for analytic transmission tests.
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

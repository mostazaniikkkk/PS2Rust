//! Puerto serie del Emotion Engine: la consola de texto (stdout/stderr).
//!
//! En PCSX2 aparece en el log como consola del EE.

const SIO_ISR: u32 = 0x1000_f130;
const SIO_TXFIFO: u32 = 0x1000_f180;

pub fn write(bytes: &[u8]) {
    for &b in bytes {
        unsafe {
            let mut spins = 0u32;
            while core::ptr::read_volatile(SIO_ISR as *const u32) & 0xf000 == 0x8000 && spins < 100_000 {
                spins += 1;
            }
            core::ptr::write_volatile(SIO_TXFIFO as *mut u8, b);
        }
    }
}

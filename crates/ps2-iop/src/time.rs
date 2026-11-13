//! Reloj del sistema del IOP.

use crate::kernel;

/// Microsegundos desde que arrancó el IOP.
pub fn uptime_usec() -> u64 {
    let mut clock = kernel::SysClock { lo: 0, hi: 0 };
    let (mut sec, mut usec) = (0u32, 0u32);
    unsafe {
        kernel::GetSystemTime(&mut clock);
        kernel::SysClock2USec(&clock, &mut sec, &mut usec);
    }
    sec as u64 * 1_000_000 + usec as u64
}

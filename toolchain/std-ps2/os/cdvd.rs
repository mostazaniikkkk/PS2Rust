//! Reloj de tiempo real de la consola, a través del servidor de comandos
//! "S" del CD/DVD en el IOP (RPC 0x80000593).

use super::kernel::uncached;
use super::sif::{self, Client};
use core::ptr::{addr_of, addr_of_mut, read_volatile};

const SID_SCMD: u32 = 0x8000_0593;
const SCMD_READ_CLOCK: u32 = 0x01;

#[repr(C, align(64))]
struct Buf([u8; 64]);

static mut CLIENT: Client = Client::new();
static mut RECV: Buf = Buf([0; 64]);
static mut UNAVAILABLE: bool = false;

/// Fecha y hora del reloj de la consola, en hora de Japón (UTC+9), que es
/// como la guarda el hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

fn bcd(b: u8) -> u32 {
    (b >> 4) as u32 * 10 + (b & 0xf) as u32
}

/// Olvida lo que se sabía del IOP (tras reiniciarlo).
pub fn forget() {
    unsafe {
        *addr_of_mut!(CLIENT) = Client::new();
        UNAVAILABLE = false;
    }
}

/// Lee el reloj. `None` si el servidor no está disponible.
pub fn read_clock() -> Option<Clock> {
    unsafe {
        if read_volatile(addr_of!(UNAVAILABLE)) {
            return None;
        }
        let client = &mut *addr_of_mut!(CLIENT);
        if !client.is_bound() && !sif::bind(client, SID_SCMD, 50) {
            UNAVAILABLE = true;
            return None;
        }
        let recv = uncached(addr_of!(RECV.0) as *const u8);
        core::ptr::write_bytes(recv, 0, 16);
        sif::call(client, SCMD_READ_CLOCK, core::ptr::null(), 0, recv, 16);
        let ok = read_volatile(recv as *const u32) != 0;
        // sceCdCLOCK: stat, second, minute, hour, pad, day, month, year (BCD).
        let b = |i: usize| read_volatile(recv.add(4 + i));
        if !ok || b(0) != 0 {
            return None;
        }
        Some(Clock {
            second: bcd(b(1)),
            minute: bcd(b(2)),
            hour: bcd(b(3)),
            day: bcd(b(5)),
            month: bcd(b(6) & 0x1f),
            year: 2000 + bcd(b(7)),
        })
    }
}

/// Días desde 1970-01-01 (calendario gregoriano proléptico).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

impl Clock {
    /// Segundos desde la época Unix (UTC).
    pub fn unix_seconds(&self) -> i64 {
        let days = days_from_civil(self.year as i64, self.month, self.day);
        days * 86_400 + (self.hour * 3600 + self.minute * 60 + self.second) as i64 - 9 * 3600
    }
}

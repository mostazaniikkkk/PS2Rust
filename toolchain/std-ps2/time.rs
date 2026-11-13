//! `Instant` con el contador de ciclos de la CPU y `SystemTime` con el reloj
//! de tiempo real de la consola.

use crate::sync::atomic::{AtomicBool, Ordering};
use crate::sys::os::{cdvd, sched};
use crate::time::Duration;

fn nanos(n: u128) -> Duration {
    Duration::new((n / 1_000_000_000) as u64, (n % 1_000_000_000) as u32)
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Instant(Duration);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct SystemTime(Duration);

pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::from_secs(0));

impl Instant {
    pub fn now() -> Instant {
        Instant(nanos(sched::cycles_to_nanos(sched::now())))
    }

    pub fn checked_sub_instant(&self, other: &Instant) -> Option<Duration> {
        self.0.checked_sub(other.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<Instant> {
        Some(Instant(self.0.checked_add(*other)?))
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<Instant> {
        Some(Instant(self.0.checked_sub(*other)?))
    }
}

/// Momento de la primera lectura del reloj, para no hacer una llamada al
/// IOP cada vez: (segundos Unix del RTC, ciclos de CPU en ese instante).
static mut ANCHOR: (u64, u64) = (0, 0);
static ANCHORED: AtomicBool = AtomicBool::new(false);

/// 2000-01-01T00:00:00Z: se usa si el reloj de la consola no responde.
const FALLBACK_EPOCH: u64 = 946_684_800;

fn anchor() -> (u64, u64) {
    if !ANCHORED.load(Ordering::Acquire) {
        let secs = cdvd::read_clock().map(|c| c.unix_seconds().max(0) as u64).unwrap_or(FALLBACK_EPOCH);
        let cycles = sched::now();
        unsafe { ANCHOR = (secs, cycles) };
        ANCHORED.store(true, Ordering::Release);
    }
    unsafe { ANCHOR }
}

/// Convierte una fecha de la PS2 (hora de Japón, UTC+9) a `SystemTime`.
pub(crate) fn from_ps2_datetime(year: u32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> Option<SystemTime> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let clock = cdvd::Clock { year, month, day, hour, minute: min, second: sec };
    let s = clock.unix_seconds();
    if s < 0 { None } else { Some(SystemTime(Duration::from_secs(s as u64))) }
}

impl SystemTime {
    pub const MAX: SystemTime = SystemTime(Duration::MAX);
    pub const MIN: SystemTime = SystemTime(Duration::ZERO);

    pub fn now() -> SystemTime {
        let (secs, cycles) = anchor();
        let elapsed = sched::cycles_to_nanos(sched::now().saturating_sub(cycles));
        SystemTime(Duration::from_secs(secs) + nanos(elapsed))
    }

    pub fn sub_time(&self, other: &SystemTime) -> Result<Duration, Duration> {
        self.0.checked_sub(other.0).ok_or_else(|| other.0 - self.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<SystemTime> {
        Some(SystemTime(self.0.checked_add(*other)?))
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<SystemTime> {
        Some(SystemTime(self.0.checked_sub(*other)?))
    }
}

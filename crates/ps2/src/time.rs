//! Medición de tiempo con el contador de ciclos de la CPU.

use crate::kernel::cpu_ticks;
use core::cell::Cell;

/// Frecuencia del Emotion Engine: 294,912 MHz.
pub const CPU_HZ: u64 = 294_912_000;

struct State {
    last: Cell<u32>,
    total: Cell<u64>,
}

// El runtime es de un solo hilo.
unsafe impl Sync for State {}

static STATE: State = State { last: Cell::new(0), total: Cell::new(0) };

/// Ciclos de CPU acumulados. El contador de hardware es de 32 bits (da la
/// vuelta cada ~14,5 s), así que hay que llamar a esta función al menos una
/// vez en ese intervalo; `Gs::present` lo hace en cada frame.
pub fn ticks() -> u64 {
    let now = cpu_ticks();
    let delta = now.wrapping_sub(STATE.last.get());
    STATE.last.set(now);
    let total = STATE.total.get() + delta as u64;
    STATE.total.set(total);
    total
}

/// Microsegundos transcurridos desde la primera llamada a [`ticks`].
pub fn micros() -> u64 {
    ticks() * 1000 / (CPU_HZ / 1000)
}

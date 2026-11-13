//! Reloj monótono, temporizador de 1 kHz y espera/despertar de hilos.
//!
//! * El tiempo se mide con el contador de ciclos del COP0 (294,912 MHz),
//!   extendido a 64 bits por la interrupción del Timer 1 del EE, que salta
//!   cada milisegundo.
//! * Cada hilo tiene una ranura de espera: la dirección por la que espera
//!   (futex) y/o un plazo. La interrupción despierta a los hilos cuyo plazo
//!   venció; `wake` despierta a los que esperan una dirección. Las dos rutas
//!   limpian la ranura dentro de una sección crítica, así que cada espera
//!   recibe como mucho un despertar.

use super::kernel;

/// Ciclos de CPU por segundo.
pub const CPU_HZ: u64 = 294_912_000;

const T1_COUNT: u32 = 0x1000_0800;
const T1_MODE: u32 = 0x1000_0810;
const T1_COMP: u32 = 0x1000_0820;
/// BUSCLK/256 = 576 kHz: 576 cuentas por milisegundo.
const T1_TICKS_PER_MS: u32 = 576;
const MODE_RUN: u32 = 2 | 1 << 6 | 1 << 7 | 1 << 8; // BUSCLK/256, ZRET, CUE, CMPE
const MODE_EQUF: u32 = 1 << 10;

const MAX_THREADS: usize = 256;

static mut READY: bool = false;
/// Ciclos acumulados hasta `LAST_COUNT`.
static mut BASE: u64 = 0;
static mut LAST_COUNT: u32 = 0;

static mut WAIT_ADDR: [usize; MAX_THREADS] = [0; MAX_THREADS];
static mut DEADLINE: [u64; MAX_THREADS] = [0; MAX_THREADS];
static mut TIMED_OUT: [bool; MAX_THREADS] = [false; MAX_THREADS];
static mut NEXT_DEADLINE: u64 = u64::MAX;

/// Actualiza el reloj de 64 bits. Llamar con interrupciones desactivadas.
fn advance() -> u64 {
    unsafe {
        let now = kernel::cpu_count();
        let delta = now.wrapping_sub(LAST_COUNT);
        LAST_COUNT = now;
        BASE += delta as u64;
        BASE
    }
}

extern "C" fn timer_handler(_cause: i32, _arg: usize, _addr: usize) -> i32 {
    unsafe {
        kernel::write32(T1_MODE, MODE_RUN | MODE_EQUF);
        let now = advance();
        if now >= NEXT_DEADLINE {
            let mut next = u64::MAX;
            for tid in 0..MAX_THREADS {
                let d = DEADLINE[tid];
                if d == 0 {
                    continue;
                }
                if d <= now {
                    DEADLINE[tid] = 0;
                    WAIT_ADDR[tid] = 0;
                    TIMED_OUT[tid] = true;
                    kernel::i_wakeup_thread(tid as i32);
                } else if d < next {
                    next = d;
                }
            }
            NEXT_DEADLINE = next;
        }
    }
    kernel::exit_handler();
    0
}

/// Arranca el temporizador. Idempotente.
pub fn init() {
    let first = kernel::critical(|| unsafe {
        if READY {
            return false;
        }
        READY = true;
        LAST_COUNT = kernel::cpu_count();
        true
    });
    if !first {
        return;
    }
    unsafe {
        kernel::write32(T1_MODE, 0);
        kernel::write32(T1_COUNT, 0);
        kernel::write32(T1_COMP, T1_TICKS_PER_MS);
    }
    kernel::add_intc_handler(kernel::INTC_TIM1, timer_handler);
    kernel::enable_intc(kernel::INTC_TIM1);
    unsafe { kernel::write32(T1_MODE, MODE_RUN | MODE_EQUF) };
}

/// Ciclos de CPU desde el arranque del reloj (monótono, 64 bits).
pub fn now() -> u64 {
    init();
    kernel::critical(advance)
}

fn slot() -> usize {
    (kernel::thread_id() as usize) % MAX_THREADS
}

/// Bloquea el hilo actual hasta que lo despierte `wake(addr)` o venza
/// `deadline` (en ciclos de [`now`]). Si `check` devuelve `false` antes de
/// dormir, no espera. Devuelve `false` si venció el plazo.
///
/// Puede volver antes de tiempo (despertar espurio): el llamador decide.
pub fn block(addr: usize, deadline: Option<u64>, check: impl FnOnce() -> bool) -> bool {
    init();
    let tid = slot();
    let should_sleep = kernel::critical(|| unsafe {
        if !check() {
            return false;
        }
        WAIT_ADDR[tid] = addr;
        TIMED_OUT[tid] = false;
        if let Some(d) = deadline {
            let d = d.max(1);
            DEADLINE[tid] = d;
            if d < NEXT_DEADLINE {
                NEXT_DEADLINE = d;
            }
        }
        true
    });
    if !should_sleep {
        return true;
    }
    kernel::sleep_thread();
    kernel::critical(|| unsafe {
        let timed_out = TIMED_OUT[tid];
        WAIT_ADDR[tid] = 0;
        DEADLINE[tid] = 0;
        TIMED_OUT[tid] = false;
        !timed_out
    })
}

/// Despierta hasta `count` hilos que esperan en `addr`; devuelve cuántos.
pub fn wake(addr: usize, count: usize) -> usize {
    let mut woken = [0u8; MAX_THREADS];
    let n = kernel::critical(|| unsafe {
        let mut n = 0;
        for tid in 0..MAX_THREADS {
            if n == count {
                break;
            }
            if WAIT_ADDR[tid] == addr && addr != 0 {
                WAIT_ADDR[tid] = 0;
                DEADLINE[tid] = 0;
                woken[n] = tid as u8;
                n += 1;
            }
        }
        n
    });
    for &tid in &woken[..n] {
        kernel::wakeup_thread(tid as i32);
    }
    n
}

/// Duerme hasta el ciclo `deadline`.
pub fn sleep_until(deadline: u64) {
    while now() < deadline {
        block(0, Some(deadline), || true);
    }
}

/// Convierte nanosegundos a ciclos (redondeando hacia arriba).
pub fn nanos_to_cycles(nanos: u128) -> u64 {
    let c = (nanos * CPU_HZ as u128).div_ceil(1_000_000_000);
    c.min(u64::MAX as u128) as u64
}

/// Convierte ciclos a nanosegundos.
pub fn cycles_to_nanos(cycles: u64) -> u128 {
    cycles as u128 * 1_000_000_000 / CPU_HZ as u128
}

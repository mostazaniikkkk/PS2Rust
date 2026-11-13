//! Futex sobre el planificador del kernel del EE.
//!
//! Con él, la std construye `Mutex`, `Condvar`, `RwLock`, `Once` y el
//! aparcamiento de hilos (`thread::park`) con sus implementaciones genéricas.

use super::os::sched;
use crate::sync::atomic::Atomic;
use crate::sync::atomic::Ordering::Relaxed;
use crate::time::Duration;

pub type Futex = Atomic<Primitive>;
pub type Primitive = u32;
pub type SmallFutex = Atomic<SmallPrimitive>;
pub type SmallPrimitive = u32;

/// Espera mientras `*futex == expected`. Devuelve `false` si venció el plazo.
pub fn futex_wait(futex: &Atomic<u32>, expected: u32, timeout: Option<Duration>) -> bool {
    let deadline = timeout.map(|d| sched::now().saturating_add(sched::nanos_to_cycles(d.as_nanos())));
    sched::block(futex.as_ptr() as usize, deadline, || futex.load(Relaxed) == expected)
}

#[inline]
pub fn futex_wake(futex: &Atomic<u32>) -> bool {
    sched::wake(futex.as_ptr() as usize, 1) > 0
}

#[inline]
pub fn futex_wake_all(futex: &Atomic<u32>) {
    sched::wake(futex.as_ptr() as usize, usize::MAX);
}

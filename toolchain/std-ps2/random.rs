//! Bytes aleatorios para `HashMap` y `std::random`.
//!
//! La PS2 no tiene generador por hardware: se mezclan el contador de ciclos,
//! el reloj de 64 bits y un contador con splitmix64. Sirve para dispersar
//! tablas hash; no es criptográficamente seguro.

use crate::sync::atomic::{AtomicU32, Ordering};
use crate::sys::os::{kernel, sched};

static COUNTER: AtomicU32 = AtomicU32::new(0x9e37_79b9);

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

pub fn fill_bytes(bytes: &mut [u8]) {
    let mut state = sched::now() ^ (kernel::cpu_count() as u64) << 32;
    for chunk in bytes.chunks_mut(8) {
        let c = COUNTER.fetch_add(0x6d2b_79f5, Ordering::Relaxed) as u64;
        state = splitmix(state ^ c ^ kernel::cpu_count() as u64);
        let v = state.to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
}

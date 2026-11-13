//! Carga de módulos en el IOP (servidor LOADFILE de la BIOS, RPC 0x80000006).

use super::kernel::uncached;
use super::sif::{self, Client};
use core::ptr::{addr_of, addr_of_mut, copy_nonoverlapping, read_volatile, write_bytes, write_volatile};

const SID_LOADFILE: u32 = 0x8000_0006;
const F_MOD_LOAD: u32 = 0;
const PATH_MAX: usize = 252;

#[repr(C, align(64))]
struct Arg([u8; 512]);

static mut CLIENT: Client = Client::new();
static mut ARG: Arg = Arg([0; 512]);
static mut MEMCARD_STATE: u8 = 0; // 0 = sin intentar, 1 = listo, 2 = falló

/// Carga un módulo IRX (por ejemplo `rom0:MCMAN`). Devuelve el resultado del
/// cargador (>= 0 si fue bien).
pub fn load_module(path: &[u8]) -> i32 {
    if path.len() >= PATH_MAX {
        return -22;
    }
    unsafe {
        let client = &mut *addr_of_mut!(CLIENT);
        if !client.is_bound() && !sif::bind(client, SID_LOADFILE, 200) {
            return -1;
        }
        let a = uncached(addr_of!(ARG.0) as *const u8);
        write_bytes(a, 0, 512);
        copy_nonoverlapping(path.as_ptr(), a.add(8), path.len());
        sif::call(client, F_MOD_LOAD, a, 512, a, 16);
        read_volatile(a as *const i32)
    }
}

/// Olvida lo que se sabía del IOP (tras reiniciarlo).
pub fn forget() {
    unsafe {
        *addr_of_mut!(CLIENT) = Client::new();
        write_volatile(addr_of_mut!(MEMCARD_STATE), 0);
    }
}

/// Carga (una vez) los módulos de la BIOS que dan acceso a `mc0:` y `mc1:`.
pub fn ensure_memory_card() -> bool {
    unsafe {
        match read_volatile(addr_of!(MEMCARD_STATE)) {
            1 => return true,
            2 => return false,
            _ => {}
        }
        let ok = [&b"rom0:SIO2MAN"[..], b"rom0:MCMAN", b"rom0:MCSERV"].iter().all(|m| load_module(m) >= 0);
        write_volatile(addr_of_mut!(MEMCARD_STATE), if ok { 1 } else { 2 });
        ok
    }
}

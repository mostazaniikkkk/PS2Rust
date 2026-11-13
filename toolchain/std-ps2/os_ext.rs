//! Extensiones de la std para PlayStation 2 (`std::os::ps2`).

#![stable(feature = "rust1", since = "1.0.0")]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod ffi;

use crate::io;

#[stable(feature = "rust1", since = "1.0.0")]
pub mod prelude {
    #[doc(no_inline)]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use super::ffi::{OsStrExt, OsStringExt};
}

/// Carga un módulo IRX en el IOP, por ejemplo `rom0:PADMAN`. Devuelve el
/// resultado del cargador.
#[stable(feature = "rust1", since = "1.0.0")]
pub fn load_iop_module(path: &str) -> io::Result<i32> {
    let r = crate::sys::os::loadfile::load_module(path.as_bytes());
    if r < 0 { Err(io::Error::from_raw_os_error(-r)) } else { Ok(r) }
}

/// Reinicia el IOP y deja cargados sólo los módulos de la BIOS.
///
/// Un programa lanzado desde otro (uLaunchELF, Free McBoot) hereda el IOP
/// como lo dejó el lanzador, con sus propios controladores. Reiniciarlo al
/// empezar da siempre el mismo punto de partida. Los archivos abiertos y los
/// [`SifRpcClient`] ligados antes dejan de valer.
#[stable(feature = "rust1", since = "1.0.0")]
pub fn reset_iop() {
    crate::sys::os::sif::reset_iop(b"rom0:UDNL rom0:EELOADCNF");
    crate::sys::os::loadfile::forget();
    crate::sys::os::cdvd::forget();
    crate::sys::os::fileio::forget();
}

/// Ciclos de CPU desde el arranque (294,912 MHz), sin dar la vuelta.
#[stable(feature = "rust1", since = "1.0.0")]
pub fn cpu_cycles() -> u64 {
    crate::sys::os::sched::now()
}

/// Bytes libres en el heap.
#[stable(feature = "rust1", since = "1.0.0")]
pub fn heap_free_bytes() -> usize {
    crate::sys::os::heap::free_bytes()
}

/// Tamaño total del heap.
#[stable(feature = "rust1", since = "1.0.0")]
pub fn heap_total_bytes() -> usize {
    crate::sys::os::heap::total_bytes()
}

/// Cliente de un servidor RPC del IOP (mandos, sonido y demás módulos que la
/// std no cubre). Se liga con [`bind`](SifRpcClient::bind) al identificador
/// del servidor y después se le llama con [`call`](SifRpcClient::call).
///
/// El IOP guarda la dirección del cliente mientras atiende una llamada, así
/// que el valor no debe moverse entre `bind` y la última `call`: guárdalo en
/// un `Box` o en un `static`.
#[stable(feature = "rust1", since = "1.0.0")]
pub struct SifRpcClient {
    inner: crate::sys::os::sif::Client,
}

#[stable(feature = "rust1", since = "1.0.0")]
impl SifRpcClient {
    /// Un cliente sin ligar.
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn new() -> SifRpcClient {
        SifRpcClient { inner: crate::sys::os::sif::Client::new() }
    }

    /// Liga el cliente al servidor `sid`. Reintenta hasta `attempts` veces
    /// mientras el servidor no exista (por ejemplo, si su módulo se acaba de
    /// cargar) y devuelve si lo consiguió.
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn bind(&mut self, sid: u32, attempts: u32) -> bool {
        crate::sys::os::sif::bind(&mut self.inner, sid, attempts)
    }

    /// ¿Está ligado a un servidor?
    #[stable(feature = "rust1", since = "1.0.0")]
    pub fn is_bound(&self) -> bool {
        self.inner.is_bound()
    }

    /// Llama a la función `fno` del servidor y espera a que termine.
    ///
    /// Los `send_size` bytes de `send` se copian al servidor por DMA y éste
    /// escribe `recv_size` bytes de respuesta en `recv`, también por DMA.
    ///
    /// # Safety
    ///
    /// `send` debe estar alineado a 16 bytes y `recv` a 64, y ambos deben
    /// apuntar a memoria válida durante la llamada. El DMA no pasa por la
    /// caché de datos: escribe `send` y lee `recv` a través del espejo sin
    /// caché de la RAM (`dirección | 0x2000_0000`).
    #[stable(feature = "rust1", since = "1.0.0")]
    pub unsafe fn call(&mut self, fno: u32, send: *const u8, send_size: u32, recv: *mut u8, recv_size: u32) {
        crate::sys::os::sif::call(&mut self.inner, fno, send, send_size, recv, recv_size)
    }
}

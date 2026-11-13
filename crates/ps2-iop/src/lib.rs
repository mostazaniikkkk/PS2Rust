//! Runtime de Rust para módulos IRX del IOP, el procesador de E/S de la
//! PlayStation 2 (un R3000A con 2 MiB de RAM).
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//! use ps2_iop::println;
//!
//! ps2_iop::module!(name: "hola", version: (1, 0), main: main);
//!
//! fn main() {
//!     println!("Hola desde Rust en el IOP");
//! }
//! ```
//!
//! Se compila con `cargo ps2 build --iop` (target `mipsel-sony-ps2iop`) y da
//! un `.irx` que el cargador de la BIOS reubica y enlaza con el kernel.
//!
//! `main` corre en un hilo propio y el módulo queda residente. No hay std ni
//! atómicos (el R3000A no tiene con qué): para compartir datos entre hilos
//! están [`sync::Mutex`] y [`kernel::critical`].

#![no_std]
// `allow_internal_unstable`: para que `import_table!` funcione en otros
// crates sin que tengan que activar el ensamblador en línea para MIPS.
#![allow(internal_features)]
#![feature(asm_experimental_arch, allow_internal_unstable)]

extern crate alloc;

pub mod console;
pub mod heap;
pub mod kernel;
pub mod math;
pub mod rpc;
pub mod sync;
pub mod thread;
pub mod time;

/// Identidad del módulo: el cargador la lee para su lista de módulos.
#[repr(C)]
pub struct ModuleId {
    pub name: *const u8,
    pub version: u16,
}

unsafe impl Sync for ModuleId {}

/// Lo que devuelve `_start` al cargador.
const RESIDENT: i32 = 0;
const NO_RESIDENT: i32 = 1;

static mut MAIN: Option<fn()> = None;

extern "C" fn main_thread(_arg: *mut u8) {
    if let Some(main) = unsafe { MAIN } {
        main();
    }
    // Si `main` vuelve, su hilo termina; el módulo sigue residente.
    unsafe { kernel::ExitDeleteThread() };
}

/// Lo que hace `_start`. Corre en la pila del hilo cargador de la BIOS, que
/// es muy pequeña: aquí no se formatea texto ni se llama a nada profundo.
/// Sólo se reserva el heap y se lanza el hilo principal, con su propia pila.
#[doc(hidden)]
pub fn start(heap_bytes: usize, stack_bytes: usize, priority: u32, main: fn()) -> i32 {
    if !heap::init(heap_bytes) {
        unsafe { kernel::printf(c"[iop] sin memoria para el heap del modulo\n".as_ptr().cast()) };
        return NO_RESIDENT;
    }
    let param = kernel::ThreadParam {
        attr: kernel::TH_C,
        option: 0,
        entry: main_thread,
        stack_size: stack_bytes as u32,
        priority,
    };
    unsafe {
        MAIN = Some(main);
        let id = kernel::CreateThread(&param);
        if id <= 0 || kernel::StartThread(id, core::ptr::null_mut()) < 0 {
            kernel::printf(c"[iop] no se pudo crear el hilo principal\n".as_ptr().cast());
            return NO_RESIDENT;
        }
    }
    RESIDENT
}

/// Declara el módulo: nombre, versión y función principal.
///
/// Opcionales: `heap` (bytes del heap, por defecto 256 KiB), `stack` (pila
/// del hilo principal, por defecto 16 KiB) y `priority` (por defecto 40).
#[macro_export]
macro_rules! module {
    (name: $name:literal, version: ($major:literal, $minor:literal), main: $main:path $(, heap: $heap:expr)? $(, stack: $stack:expr)? $(, priority: $prio:expr)? $(,)?) => {
        #[doc(hidden)]
        #[used]
        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".data.irx_id")]
        pub static _irx_id: $crate::ModuleId =
            $crate::ModuleId { name: concat!($name, "\0").as_ptr(), version: ($major << 8) | $minor };

        #[doc(hidden)]
        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".text.start")]
        pub extern "C" fn _start(_argc: i32, _argv: *const *const u8) -> i32 {
            #[allow(unused_mut, unused_assignments)]
            let (mut heap, mut stack, mut priority) = (256 * 1024usize, 16 * 1024usize, 40u32);
            $( heap = $heap; )?
            $( stack = $stack; )?
            $( priority = $prio; )?
            $crate::start(heap, stack, priority, $main)
        }
    };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("[iop] {}", info);
    // El módulo no puede descargarse solo: el hilo se queda dormido.
    loop {
        unsafe { kernel::SleepThread() };
    }
}

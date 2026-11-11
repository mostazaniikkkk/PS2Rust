//! Runtime de Rust para PlayStation 2 (Emotion Engine), sin C ni PS2SDK.
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//! use ps2::println;
//!
//! ps2::entry!(main);
//!
//! fn main() {
//!     println!("Hola desde Rust en una PlayStation 2");
//! }
//! ```
//!
//! Se compila con `cargo ps2 build` (target `mipsel-sony-ps2`).

#![no_std]
#![feature(asm_experimental_arch)]
#![feature(core_intrinsics)]
#![allow(internal_features)]

extern crate alloc;

pub mod console;
pub mod font;
pub mod gs;
pub mod heap;
pub mod kernel;
pub mod math;
pub mod time;

mod start;

/// Define el punto de entrada del programa: `ps2::entry!(main);`
///
/// `main` puede devolver `()` o un `i32` (código de salida).
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[doc(hidden)]
        #[unsafe(export_name = "__ps2_main")]
        pub extern "Rust" fn __ps2_main() -> i32 {
            $crate::Termination::report($main())
        }
    };
}

/// Valores que `main` puede devolver.
pub trait Termination {
    fn report(self) -> i32;
}

impl Termination for () {
    fn report(self) -> i32 {
        0
    }
}

impl Termination for i32 {
    fn report(self) -> i32 {
        self
    }
}

/// Termina el programa y devuelve el control al sistema.
pub fn exit(code: i32) -> ! {
    // El driver (`cargo ps2 run`) reconoce esta línea para saber que terminamos.
    println!("[ps2rt] exit {}", code);
    kernel::exit(code)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("[ps2rt] {}", info);
    // Fondo rojo: en hardware real es la única pista si no hay consola serie.
    gs::set_background(gs::Color::rgb(160, 0, 0));
    exit(101)
}

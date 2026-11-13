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
//!
//! Dos formas de uso:
//!
//! * **Con la std** (recomendado): el programa es un `fn main()` normal y la
//!   std pone el arranque, el heap, los hilos y los archivos. Este crate
//!   aporta el hardware: gráficos (`gs`), fuente, matemáticas `f32`.
//!   `ps2 = { path = "...", default-features = false }`
//! * **Sin std** (`#![no_std]`): la feature `rt` (por defecto) añade el
//!   arranque, el asignador, el manejador de pánicos y `ps2::entry!`.

#![no_std]
#![feature(asm_experimental_arch)]
#![feature(core_intrinsics)]
#![allow(internal_features)]

#[cfg(feature = "rt")]
extern crate alloc;

pub mod console;
pub mod font;
pub mod gs;
#[cfg(feature = "rt")]
pub mod heap;
pub mod kernel;
pub mod math;
pub mod time;

#[cfg(feature = "rt")]
mod start;

/// Define el punto de entrada de un programa `no_std`: `ps2::entry!(main);`
///
/// `main` puede devolver `()` o un `i32` (código de salida).
#[cfg(feature = "rt")]
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
#[cfg(feature = "rt")]
pub trait Termination {
    fn report(self) -> i32;
}

#[cfg(feature = "rt")]
impl Termination for () {
    fn report(self) -> i32 {
        0
    }
}

#[cfg(feature = "rt")]
impl Termination for i32 {
    fn report(self) -> i32 {
        self
    }
}

/// Termina el programa y devuelve el control al sistema (en programas con
/// std, usa `std::process::exit`).
#[cfg(feature = "rt")]
pub fn exit(code: i32) -> ! {
    // El driver (`cargo ps2 run`) reconoce esta línea para saber que terminamos.
    println!("[ps2rt] exit {}", code);
    kernel::exit(code)
}

#[cfg(feature = "rt")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("[ps2rt] {}", info);
    // Fondo rojo: en hardware real es la única pista si no hay consola serie.
    gs::set_background(gs::Color::rgb(160, 0, 0));
    exit(101)
}

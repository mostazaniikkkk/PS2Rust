//! Ejecuta la batería de OpenGL ES 1.0 en la consola: dibuja con el Graphics
//! Synthesizer, lee los píxeles de vuelta y los comprueba. La salida debe ser
//! idéntica a la del PC (`toolchain/selftest-host --gles`), donde rasteriza la
//! referencia por software. El código de salida es el número de fallos.

#![no_std]
#![no_main]

extern crate alloc;

mod gl_suite;

use ps2::console::Console;

ps2::entry!(main);

fn main() -> i32 {
    gl_suite::run(&mut Console) as i32
}

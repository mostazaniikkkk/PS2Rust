//! Ejecuta la batería de pruebas en la consola e imprime el resultado por la
//! consola del EE. El código de salida es el número de fallos.

#![no_std]
#![no_main]

extern crate alloc;

mod suite;

use ps2::console::Console;

ps2::entry!(main);

/// Sin inline a propósito: así el argumento llega en `$f12` y la instrucción
/// `sqrt.s` necesita la corrección de campos del legalizador.
#[inline(never)]
fn native_sqrt(x: f32) -> f32 {
    ps2::math::sqrt(x)
}

fn main() -> i32 {
    let env = suite::Env { sqrt: native_sqrt };
    // `Escaped`: transporte sin pérdida por la consola del emulador.
    suite::run(&mut suite::Escaped(Console), &env) as i32
}

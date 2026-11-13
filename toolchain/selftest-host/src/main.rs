//! Ejecuta en el PC las mismas baterías que corren en la PlayStation 2. Su
//! salida es la referencia contra la que se compara, línea por línea, la de
//! la consola.
//!
//! * sin argumentos: batería de `core`/`alloc` en texto normal.
//! * `--escaped`: la misma, con la codificación `%XX` de la consola.
//! * `--iop`: la variante que corre en el IOP (sin atómicos, bloques más
//!   pequeños), codificada, como `iop-selftest`.
//! * `--std`: `core`/`alloc` + std, codificada, como `std-selftest`.
//!   Usa el directorio actual para las pruebas de archivos.

extern crate alloc;

#[path = "../../../examples/selftest/src/suite.rs"]
mod suite;
#[path = "../../../examples/std-selftest/src/std_suite.rs"]
mod std_suite;

use std::fmt::Write as _;

fn main() {
    let iop = std::env::args().any(|a| a == "--iop");
    let env = if iop { suite::Env::iop(f32::sqrt) } else { suite::Env::ee(f32::sqrt) };
    let escaped = std::env::args().any(|a| a == "--escaped");
    let with_std = std::env::args().any(|a| a == "--std");
    let mut core_out = String::new();
    let mut failed = suite::run(&mut core_out, &env);
    let mut raw = core_out;
    if with_std {
        let (std_out, std_failed) = std_suite::run();
        raw.push_str(&std_out);
        failed += std_failed;
    }
    let out = if escaped || with_std || iop {
        let mut s = String::new();
        let _ = suite::Escaped(&mut s).write_str(&raw);
        s
    } else {
        raw
    };
    print!("{out}");
    std::process::exit(failed as i32);
}

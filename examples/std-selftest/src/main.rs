//! Batería completa con std: primero la de `core`/`alloc` (la misma del
//! ejemplo `selftest`) y luego la de la biblioteca estándar.
//!
//! El mismo código se compila para el PC (`toolchain/selftest-host`) y las
//! dos salidas deben coincidir byte a byte. El código de salida es el número
//! de fallos.

extern crate alloc;

#[path = "../../selftest/src/suite.rs"]
mod suite;
mod std_suite;

use std::fmt::Write as _;

/// Adaptador de `io::Write` a `fmt::Write` para la batería de `core`.
struct Out(String);

impl std::fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.push_str(s);
        Ok(())
    }
}

fn main() {
    let env = suite::Env { sqrt: f32::sqrt };
    let mut core_out = Out(String::new());
    let core_failed = suite::run(&mut core_out, &env);
    let (std_out, std_failed) = std_suite::run();

    // Todo de una vez al final: así nada se intercala con los mensajes que
    // el IOP imprime al tocar archivos. `Escaped` hace el texto ASCII puro
    // (la consola de PCSX2 no conserva UTF-8).
    let mut text = String::new();
    let mut esc = suite::Escaped(&mut text);
    let _ = esc.write_str(&core_out.0);
    let _ = esc.write_str(&std_out);
    print!("{text}");
    std::process::exit((core_failed + std_failed) as i32);
}

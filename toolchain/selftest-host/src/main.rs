//! Ejecuta en el PC la misma batería que corre en la PlayStation 2. Su salida
//! es la referencia contra la que se compara, línea por línea, la de la consola.

extern crate alloc;

#[path = "../../../examples/selftest/src/suite.rs"]
mod suite;

fn main() {
    let env = suite::Env { sqrt: f32::sqrt };
    // Con --escaped la salida usa la misma codificación %XX que la consola.
    let escaped = std::env::args().any(|a| a == "--escaped");
    let mut out = String::new();
    let failed = if escaped {
        suite::run(&mut suite::Escaped(&mut out), &env)
    } else {
        suite::run(&mut out, &env)
    };
    print!("{out}");
    std::process::exit(failed as i32);
}

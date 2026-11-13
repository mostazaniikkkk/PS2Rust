//! La batería de pruebas del lenguaje, `core` y `alloc` (la misma del ejemplo
//! `selftest`) corriendo en el IOP.
//!
//!     cargo ps2 build --iop -p iop-selftest   ->  target/ps2/iop-selftest.irx
//!
//! El módulo espera a que el EE se lo pida por RPC (`iop-selftest-runner`),
//! ejecuta la batería imprimiendo por la consola del IOP y devuelve el número
//! de fallos. La salida debe ser idéntica a la del PC
//! (`toolchain/selftest-host --iop`).

#![no_std]
#![no_main]

extern crate alloc;

#[path = "../../selftest/src/suite.rs"]
mod suite;

use ps2_iop::console::Console;
use ps2_iop::{math, rpc};

// La batería pide más heap y pila que un módulo corriente.
ps2_iop::module!(name: "iopsuite", version: (1, 0), main: main, heap: 640 * 1024, stack: 48 * 1024);

/// Identificador del servidor RPC: "RSTT".
const SERVER_ID: u32 = 0x5253_5454;
const FN_RUN: u32 = 1;

fn main() {
    rpc::serve(SERVER_ID, 64, |function, data, _| {
        let failed = if function == FN_RUN {
            let env = suite::Env::iop(math::sqrt);
            // `Escaped`: transporte sin pérdida por la consola del emulador.
            suite::run(&mut suite::Escaped(Console::new()), &env)
        } else {
            u32::MAX
        };
        data[..4].copy_from_slice(&failed.to_le_bytes());
    });
}

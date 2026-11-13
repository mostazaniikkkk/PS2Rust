//! Lanza desde el EE la batería de pruebas del IOP: carga `iop-selftest.irx`,
//! le pide por RPC que la ejecute y sale con el número de fallos.
//!
//!     cargo ps2 build --iop -p iop-selftest
//!     cargo ps2 run -p iop-selftest-runner
//!
//! No imprime nada si todo va bien: lo que se ve por consola es la salida de
//! la batería, que escribe el propio IOP.

use std::os::ps2::{load_iop_module, SifRpcClient};

/// Identificador del servidor RPC del módulo: "RSTT".
const SERVER_ID: u32 = 0x5253_5454;
const FN_RUN: u32 = 1;

#[repr(C, align(64))]
struct Aligned([u8; 64]);

fn main() {
    if let Err(e) = load_iop_module("host:iop-selftest.irx") {
        println!("no se pudo cargar host:iop-selftest.irx: {e}");
        println!("compilalo antes con: cargo ps2 build --iop -p iop-selftest");
        std::process::exit(200);
    }
    let mut client = Box::new(SifRpcClient::new());
    if !client.bind(SERVER_ID, 200) {
        println!("el servidor RPC de iop-selftest no responde");
        std::process::exit(201);
    }
    let mut send = Box::new(Aligned([0; 64]));
    let mut recv = Box::new(Aligned([0xff; 64]));
    // El DMA no pasa por la caché: se usa el espejo sin caché de la RAM.
    let send_ptr = (send.0.as_mut_ptr() as usize | 0x2000_0000) as *mut u8;
    let recv_ptr = (recv.0.as_mut_ptr() as usize | 0x2000_0000) as *mut u32;
    let failed = unsafe {
        recv_ptr.write_volatile(u32::MAX);
        client.call(FN_RUN, send_ptr, 16, recv_ptr as *mut u8, 64);
        recv_ptr.read_volatile()
    };
    std::process::exit(failed.min(199) as i32);
}

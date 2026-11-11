//! Comprueba la ruta de pánico: el mensaje sale por la consola del EE y el
//! programa termina con código 101.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::hint::black_box;
use ps2::println;

ps2::entry!(main);

fn main() {
    println!("antes del panico");
    let data: Vec<u32> = (0..4).collect();
    let index = black_box(7);
    // Acceso fuera de rango: Rust lo detecta también en la PS2.
    println!("esto no se imprime: {}", data[index]);
}

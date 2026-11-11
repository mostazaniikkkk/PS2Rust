//! Hola mundo: consola del EE, heap y texto en pantalla.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use ps2::gs::{Color, Gs, VideoMode};
use ps2::println;

ps2::entry!(main);

fn main() {
    println!("Hola desde Rust en una PlayStation 2!");

    // `alloc` funciona: el heap es toda la RAM libre de la consola.
    let squares: Vec<u32> = (1..=10).map(|n| n * n).collect();
    let total: u32 = squares.iter().sum();
    println!("cuadrados = {:?}, suma = {}", squares, total);
    println!("heap: {} KiB libres de {} KiB", ps2::heap::free_bytes() / 1024, ps2::heap::total_bytes() / 1024);

    // Flotantes: f32 en la FPU de la consola, f64 por software.
    let radius = 2.5f32;
    println!("area del circulo (f32) = {}", ps2::math::PI * radius * radius);
    println!("1/3 (f64) = {}", 1.0f64 / 3.0);

    let mut gs = Gs::init(VideoMode::Ntsc);
    let lines: [String; 3] = [
        format!("{}x{} NTSC", gs.width(), gs.height()),
        format!("heap libre: {} KiB", ps2::heap::free_bytes() / 1024),
        format!("suma de cuadrados: {}", total),
    ];
    for frame in 0..300 {
        gs.clear(Color::rgb(12, 16, 48));
        gs.rect(40, 60, 560, 4, Color::rgb(222, 88, 28));
        gs.text(40, 80, 5, Color::WHITE, "¡Hola, PS2!");
        gs.text(40, 136, 3, Color::rgb(222, 165, 132), "Rust en el Emotion Engine");
        for (i, line) in lines.iter().enumerate() {
            gs.text(40, 190 + i as i32 * 26, 2, Color::rgb(170, 190, 230), line);
        }
        gs.text(40, 400, 2, Color::rgb(120, 130, 160), &format!("frame {frame}"));
        gs.present();
        if frame == 0 {
            println!("texto en pantalla");
        }
    }
    println!("fin del programa");
}

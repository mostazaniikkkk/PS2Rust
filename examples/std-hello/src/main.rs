//! La biblioteca estándar de Rust en una PlayStation 2: hilos, canales,
//! `HashMap`, archivos, reloj, y el resultado dibujado en pantalla con el
//! Graphics Synthesizer.

use ps2::gs::{Color, Gs, VideoMode};
use std::collections::HashMap;
use std::fs;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Fecha civil (UTC) a partir de días desde 1970.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn main() {
    let start = Instant::now();
    println!("Hola desde la std de Rust en una PlayStation 2!");

    // Cuatro hilos cuentan primos en rangos distintos y mandan el resultado
    // por un canal.
    let (tx, rx) = mpsc::channel();
    for k in 0..4u32 {
        let tx = tx.clone();
        thread::spawn(move || {
            let (lo, hi) = (k * 25_000, (k + 1) * 25_000);
            let primes = (lo.max(2)..hi).filter(|&n| (2..).take_while(|d| d * d <= n).all(|d| n % d != 0)).count();
            tx.send((k, primes)).unwrap();
        });
    }
    drop(tx);
    let mut per_thread: Vec<(u32, usize)> = rx.iter().collect();
    per_thread.sort();
    let total: usize = per_thread.iter().map(|p| p.1).sum();
    println!("primos < 100000 por hilo: {per_thread:?} -> {total}");

    // Un HashMap y un archivo en el PC (host:) o donde esté el ELF.
    let mut inventory = HashMap::new();
    inventory.insert("pociones", 3);
    inventory.insert("llaves", 1);
    inventory.insert("mapas", 2);
    let save: String = {
        let mut items: Vec<_> = inventory.iter().collect();
        items.sort();
        items.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
    };
    fs::write("partida.txt", &save).expect("no pude guardar");
    let loaded = fs::read_to_string("partida.txt").expect("no pude leer");
    let bytes = fs::metadata("partida.txt").map(|m| m.len()).unwrap_or(0);
    fs::remove_file("partida.txt").ok();
    println!("guardado y leído ({bytes} bytes): {:?}", loaded.lines().collect::<Vec<_>>());

    let unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let (y, m, d) = civil(unix.div_euclid(86_400));
    let secs = unix.rem_euclid(86_400);
    let date = format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3600, secs / 60 % 60);
    println!("reloj de la consola: {date}");

    let mut gs = Gs::init(VideoMode::Ntsc);
    let elapsed = start.elapsed();
    let lines = [
        format!("primos < 100000: {total}"),
        format!("4 hilos: {:?}", per_thread.iter().map(|p| p.1).collect::<Vec<_>>()),
        format!("archivo: {} lineas, {bytes} bytes", loaded.lines().count()),
        format!("fecha: {date}"),
        format!("tiempo de calculo: {} ms", elapsed.as_millis()),
    ];
    for frame in 0..240u32 {
        gs.clear(Color::rgb(10, 24, 30));
        gs.rect(40, 56, 560, 4, Color::rgb(80, 200, 160));
        gs.text(40, 74, 5, Color::WHITE, "std::thread");
        gs.text(40, 124, 3, Color::rgb(140, 220, 190), "Rust con biblioteca estandar");
        for (i, line) in lines.iter().enumerate() {
            gs.text(40, 184 + i as i32 * 26, 2, Color::rgb(200, 220, 230), line);
        }
        let bar = (frame as i32 * 560 / 239).max(1);
        gs.rect(40, 360, bar, 8, Color::rgb(80, 200, 160));
        gs.present();
        if frame == 0 {
            println!("pantalla lista");
        }
        thread::sleep(Duration::from_millis(1));
    }
}

//! Los dos procesadores de la PlayStation 2 hablando en Rust.
//!
//! Este programa corre en el Emotion Engine (con std). Carga en el IOP el
//! módulo `rustiop.irx` —también escrito en Rust, ver `examples/iop-module`—
//! y le pide trabajo por RPC. Cada respuesta se compara con el mismo cálculo
//! hecho aquí, en el EE. El código de salida es el número de discrepancias.
//!
//!     cargo ps2 build --iop -p rustiop
//!     cargo ps2 run -p iop-demo

use ps2::gs::{Color, Gs, VideoMode};
use std::os::ps2::{load_iop_module, SifRpcClient};
use std::thread;
use std::time::{Duration, Instant};

/// Identificador del servidor RPC del módulo: "RUST".
const SERVER_ID: u32 = 0x5255_5354;
/// Máximo de bytes por mensaje (igual que en el módulo).
const BUFFER: usize = 8192;

const FN_HELLO: u32 = 1;
const FN_TEXT: u32 = 2;
const FN_PRIMES: u32 = 3;
const FN_SORT: u32 = 4;
const FN_CRC32: u32 = 5;
const FN_STATS: u32 = 6;
const FN_STATUS: u32 = 7;

/// El DMA entre procesadores pide búferes alineados a 64 bytes.
#[repr(C, align(64))]
struct Aligned([u8; BUFFER]);

/// Conexión con el módulo del IOP.
struct Iop {
    client: Box<SifRpcClient>,
    send: Box<Aligned>,
    recv: Box<Aligned>,
}

impl Iop {
    fn connect() -> Option<Iop> {
        let mut client = Box::new(SifRpcClient::new());
        if !client.bind(SERVER_ID, 200) {
            return None;
        }
        Some(Iop { client, send: Box::new(Aligned([0; BUFFER])), recv: Box::new(Aligned([0; BUFFER])) })
    }

    /// Llama a la función `function` del módulo con `payload` y devuelve la
    /// respuesta. Protocolo: un `u32` con el tamaño y después los datos.
    fn call(&mut self, function: u32, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() + 4 <= BUFFER, "mensaje demasiado grande");
        // El DMA no pasa por la caché: los búferes se tocan por el espejo
        // sin caché de la RAM.
        let send = (self.send.0.as_mut_ptr() as usize | 0x2000_0000) as *mut u8;
        let recv = (self.recv.0.as_mut_ptr() as usize | 0x2000_0000) as *mut u8;
        unsafe {
            std::ptr::copy_nonoverlapping((payload.len() as u32).to_le_bytes().as_ptr(), send, 4);
            std::ptr::copy_nonoverlapping(payload.as_ptr(), send.add(4), payload.len());
            let size = (payload.len() + 4 + 15) & !15;
            self.client.call(function, send, size as u32, recv, BUFFER as u32);
            let mut length = [0u8; 4];
            std::ptr::copy_nonoverlapping(recv, length.as_mut_ptr(), 4);
            let length = (u32::from_le_bytes(length) as usize).min(BUFFER - 4);
            let mut reply = vec![0u8; length];
            std::ptr::copy_nonoverlapping(recv.add(4), reply.as_mut_ptr(), length);
            reply
        }
    }
}

fn u32_at(bytes: &[u8], index: usize) -> u32 {
    bytes.get(index * 4..index * 4 + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    bytes.get(offset..offset + 8).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap()))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Lleva la cuenta de las comprobaciones y de lo que se mostrará en pantalla.
struct Report {
    lines: Vec<(bool, String)>,
}

impl Report {
    fn check(&mut self, ok: bool, text: String) {
        println!("[{}] {text}", if ok { "ok" } else { "FALLO" });
        self.lines.push((ok, text));
    }
}

fn main() {
    let start = Instant::now();
    println!("EE: cargando rustiop.irx en el IOP");
    match load_iop_module("host:rustiop.irx") {
        Ok(id) => println!("EE: modulo cargado (id {id})"),
        Err(e) => {
            println!("EE: no se pudo cargar host:rustiop.irx: {e}");
            println!("    compilalo antes con: cargo ps2 build --iop -p rustiop");
            std::process::exit(100);
        }
    }
    let Some(mut iop) = Iop::connect() else {
        println!("EE: el servidor RPC del modulo no responde");
        std::process::exit(101);
    };
    let mut report = Report { lines: Vec::new() };

    // 1. Presentación.
    let hello = String::from_utf8_lossy(&iop.call(FN_HELLO, &[])).into_owned();
    println!("IOP: {hello}");
    report.check(hello.starts_with("rustiop 1.0"), "el modulo se presenta".to_string());

    // 2. Texto UTF-8: String, chars y tablas Unicode en el IOP.
    let text = "Rust en los dos procesadores: ñandú, straße, 日本";
    let expected: String = text.chars().rev().collect::<String>().to_uppercase();
    let reply = String::from_utf8_lossy(&iop.call(FN_TEXT, text.as_bytes())).into_owned();
    report.check(reply == expected, format!("texto al reves en mayusculas ({} bytes)", reply.len()));

    // 3. Criba de primos: Vec y enteros de 64 bits.
    let limit = 100_000u32;
    let reply = iop.call(FN_PRIMES, &limit.to_le_bytes());
    let primes: Vec<u64> = (2..limit as u64).filter(|&n| (2..).take_while(|d| d * d <= n).all(|d| n % d != 0)).collect();
    let (count, sum) = (primes.len() as u32, primes.iter().sum::<u64>());
    report.check(
        u32_at(&reply, 0) == count && u64_at(&reply, 4) == sum,
        format!("primos < {limit}: {} (suma {})", u32_at(&reply, 0), u64_at(&reply, 4)),
    );

    // 4. Ordenación de 2000 enteros.
    let mut seed = 0x1234_5678u32;
    let mut values: Vec<u32> = (0..2000)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        })
        .collect();
    let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    let reply = iop.call(FN_SORT, &raw);
    values.sort_unstable();
    let sorted: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    report.check(reply == sorted, format!("ordenar {} enteros (minimo {:#010x})", values.len(), u32_at(&reply, 0)));

    // 5. CRC-32 de los mismos 8000 bytes.
    let reply = iop.call(FN_CRC32, &raw);
    report.check(u32_at(&reply, 0) == crc32(&raw), format!("crc32 de {} bytes: {:#010x}", raw.len(), u32_at(&reply, 0)));

    // 6. Media y varianza en f64: en el IOP, coma flotante por software.
    let reply = iop.call(FN_STATS, &raw);
    let n = values.len() as f64;
    // Mismo orden de operaciones que el módulo, con los datos sin ordenar.
    let unsorted: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let mean = unsorted.iter().map(|&v| v as f64).sum::<f64>() / n;
    let variance = unsorted.iter().map(|&v| (v as f64 - mean) * (v as f64 - mean)).sum::<f64>() / n;
    report.check(
        u64_at(&reply, 0) == mean.to_bits() && u64_at(&reply, 8) == variance.to_bits(),
        format!("media f64 sin FPU: {:.3}", f64::from_bits(u64_at(&reply, 0))),
    );

    // 7. Estado: el segundo hilo del módulo cuenta tics mientras el EE duerme.
    let before = iop.call(FN_STATUS, &[]);
    thread::sleep(Duration::from_millis(300));
    let after = iop.call(FN_STATUS, &[]);
    let ticks = u32_at(&after, 2).wrapping_sub(u32_at(&before, 2));
    let calls = u32_at(&after, 0);
    report.check(
        calls == 8 && (5..=60).contains(&ticks) && u64_at(&after, 12) > u64_at(&before, 12),
        format!("hilo del reloj: {ticks} tics en 300 ms; {calls} llamadas atendidas"),
    );

    let good = report.lines.iter().filter(|l| l.0).count();
    let total = report.lines.len();
    let elapsed = start.elapsed().as_millis();
    println!("demo IOP: {good}/{total} comprobaciones correctas en {elapsed} ms");

    let mut gs = Gs::init(VideoMode::Ntsc);
    for frame in 0..240u32 {
        gs.clear(Color::rgb(16, 18, 34));
        gs.rect(40, 50, 560, 4, Color::rgb(240, 150, 60));
        gs.text(40, 66, 5, Color::WHITE, "Rust en el IOP");
        gs.text(40, 116, 2, Color::rgb(250, 190, 120), "EE (R5900, std)  <-- RPC -->  IOP (R3000A, no_std)");
        for (i, (ok, line)) in report.lines.iter().enumerate() {
            let y = 156 + i as i32 * 24;
            let color = if *ok { Color::rgb(120, 220, 140) } else { Color::rgb(240, 90, 90) };
            gs.text(40, y, 2, color, if *ok { "ok" } else { "MAL" });
            gs.text(90, y, 2, Color::rgb(210, 215, 235), line);
        }
        gs.text(40, 340, 2, Color::WHITE, &format!("{good}/{total} correctas en {elapsed} ms"));
        let bar = (frame as i32 * 560 / 239).max(1);
        gs.rect(40, 372, bar, 8, Color::rgb(240, 150, 60));
        gs.present();
        if frame == 0 {
            println!("pantalla lista");
        }
        thread::sleep(Duration::from_millis(1));
    }
    std::process::exit((total - good) as i32);
}

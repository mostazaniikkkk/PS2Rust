//! Un módulo IRX escrito en Rust: corre en el IOP (el procesador de E/S de la
//! PlayStation 2) y ofrece servicios por RPC a los programas del EE.
//!
//!     cargo ps2 build --iop -p rustiop      ->  target/ps2/rustiop.irx
//!
//! El programa `iop-demo` lo carga desde el EE y llama a cada función.
//!
//! Protocolo: petición y respuesta empiezan con un `u32` (bytes de carga
//! útil) seguido de la carga útil.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use ps2_iop::sync::Mutex;
use ps2_iop::{heap, println, rpc, thread, time};

ps2_iop::module!(name: "rustiop", version: (1, 0), main: main);

// Un servicio de la BIOS que `ps2-iop` no trae: se importa con la misma
// macro que usa el crate. `sysclib` es la libc mínima del kernel del IOP.
ps2_iop::import_table!("sysclib", 1 . 1, { 27 => bios_strlen });

extern "C" {
    fn bios_strlen(s: *const u8) -> usize;
}

/// Identificador del servidor RPC: "RUST".
const SERVER_ID: u32 = 0x5255_5354;
/// Máximo de bytes por mensaje.
const BUFFER: usize = 8192;

const FN_HELLO: u32 = 1;
const FN_TEXT: u32 = 2;
const FN_PRIMES: u32 = 3;
const FN_SORT: u32 = 4;
const FN_CRC32: u32 = 5;
const FN_STATS: u32 = 6;
const FN_STATUS: u32 = 7;

/// Estado compartido entre el hilo que atiende llamadas y el del reloj.
struct Status {
    calls: u32,
    bytes_in: u32,
    ticks: u32,
}

static STATUS: Mutex<Status> = Mutex::new(Status { calls: 0, bytes_in: 0, ticks: 0 });

fn main() {
    println!(
        "rustiop: Rust en el IOP (hilo {}, heap {} KiB, IOP libre {} KiB)",
        thread::current().0,
        heap::free_bytes() / 1024,
        heap::system_free_bytes() / 1024
    );

    println!("rustiop: sysclib de la BIOS dice strlen = {}", unsafe { bios_strlen(c"rustiop".as_ptr().cast()) });

    // Un segundo hilo, con más prioridad que el servidor: cada 10 ms lo
    // interrumpe para contar un tic.
    let ticker = thread::Builder::new().priority(30).stack_size(4096).spawn(|| loop {
        thread::sleep_ms(10);
        STATUS.lock().ticks += 1;
    });
    if ticker.is_none() {
        println!("rustiop: no se pudo crear el hilo del reloj");
    }

    rpc::serve(SERVER_ID, BUFFER, |function, data, received| {
        let length = (u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize).min(received.saturating_sub(4));
        {
            let mut status = STATUS.lock();
            status.calls += 1;
            status.bytes_in += length as u32;
        }
        let reply = handle(function, &data[4..4 + length]);
        let reply = &reply[..reply.len().min(data.len() - 4)];
        data[..4].copy_from_slice(&(reply.len() as u32).to_le_bytes());
        data[4..4 + reply.len()].copy_from_slice(reply);
    });
}

fn handle(function: u32, input: &[u8]) -> Vec<u8> {
    match function {
        FN_HELLO => hello().into_bytes(),
        FN_TEXT => text(input).into_bytes(),
        FN_PRIMES => primes(input),
        FN_SORT => sort(input),
        FN_CRC32 => crc32(input).to_le_bytes().to_vec(),
        FN_STATS => stats(input),
        FN_STATUS => status(),
        _ => Vec::new(),
    }
}

fn words(input: &[u8]) -> Vec<u32> {
    input.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Presentación del módulo.
fn hello() -> String {
    format!(
        "rustiop 1.0 en el hilo {}: heap {}/{} KiB, IOP libre {} KiB",
        thread::current().0,
        heap::free_bytes() / 1024,
        heap::total_bytes() / 1024,
        heap::system_free_bytes() / 1024
    )
}

/// El texto recibido, al revés y en mayúsculas (UTF-8 completo).
fn text(input: &[u8]) -> String {
    match core::str::from_utf8(input) {
        Ok(s) => s.chars().rev().collect::<String>().to_uppercase(),
        Err(e) => format!("UTF-8 invalido: {e}"),
    }
}

/// Cuántos primos hay por debajo de `n` y cuánto suman (criba).
fn primes(input: &[u8]) -> Vec<u8> {
    let n = words(input).first().copied().unwrap_or(0) as usize;
    let mut sieve = vec![true; n.max(2)];
    sieve[0] = false;
    sieve[1] = false;
    let mut i = 2;
    while i * i < n {
        if sieve[i] {
            let mut j = i * i;
            while j < n {
                sieve[j] = false;
                j += i;
            }
        }
        i += 1;
    }
    let (mut count, mut sum) = (0u32, 0u64);
    for (value, _) in sieve.iter().enumerate().take(n).filter(|(_, &prime)| prime) {
        count += 1;
        sum += value as u64;
    }
    let mut out = count.to_le_bytes().to_vec();
    out.extend_from_slice(&sum.to_le_bytes());
    out
}

/// Los `u32` recibidos, ordenados.
fn sort(input: &[u8]) -> Vec<u8> {
    let mut values = words(input);
    values.sort_unstable();
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
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

/// Media y varianza en `f64` de los `u32` recibidos. El IOP no tiene FPU:
/// toda esta aritmética es IEEE 754 por software.
fn stats(input: &[u8]) -> Vec<u8> {
    let values = words(input);
    let n = values.len().max(1) as f64;
    let mean = values.iter().map(|&v| v as f64).sum::<f64>() / n;
    let variance = values.iter().map(|&v| (v as f64 - mean) * (v as f64 - mean)).sum::<f64>() / n;
    let mut out = mean.to_bits().to_le_bytes().to_vec();
    out.extend_from_slice(&variance.to_bits().to_le_bytes());
    out
}

/// Llamadas atendidas, bytes recibidos, tics del segundo hilo y reloj.
fn status() -> Vec<u8> {
    let (calls, bytes_in, ticks) = {
        let status = STATUS.lock();
        (status.calls, status.bytes_in, status.ticks)
    };
    let mut out = Vec::new();
    for v in [calls, bytes_in, ticks] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&time::uptime_usec().to_le_bytes());
    out
}

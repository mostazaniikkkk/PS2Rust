//! `ps2snd.irx`: el sonido de la PlayStation 2, en Rust.
//!
//! El chip de sonido (SPU2) cuelga del IOP. Este módulo es la mitad del IOP
//! del crate `ps2-audio`: usa la biblioteca de sonido de la BIOS
//! (`rom0:LIBSD`, que el EE carga antes) y ofrece por RPC:
//!
//! * Un **flujo PCM** estéreo de 16 bits a 48 kHz. El EE envía muestras, que
//!   esperan en una cola, y un hilo las va pasando al SPU2 en bloques de 512
//!   cada vez que el chip termina la mitad de su búfer (93,75 veces por
//!   segundo). Si la cola se vacía suena silencio.
//! * Las **24 voces** del SPU2 para muestras ADPCM (efectos): subir datos a
//!   la memoria de sonido, y tocar, parar y consultar voces.
//!
//! Protocolo (servidor "PSSN"): la petición y la respuesta son palabras de 32
//! bits; el número de función dice qué se pide. Ver las constantes `FN_*`.

#![no_std]
#![no_main]

use core::ptr::addr_of_mut;
use ps2_iop::kernel::{self, critical};
use ps2_iop::{println, rpc, thread};

ps2_iop::module!(name: "ps2snd", version: (1, 0), main: main, heap: 64 * 1024);

ps2_iop::import_table!("libsd", 1 . 1, {
    4 => sceSdInit,
    5 => sceSdSetParam,
    6 => sceSdGetParam,
    7 => sceSdSetSwitch,
    9 => sceSdSetAddr,
    17 => sceSdVoiceTrans,
    18 => sceSdBlockTrans,
    19 => sceSdVoiceTransStatus,
    20 => sceSdBlockTransStatus,
    21 => sceSdSetTransCallback,
});

#[allow(non_snake_case)]
extern "C" {
    fn sceSdInit(flag: i32) -> i32;
    fn sceSdSetParam(entry: u32, value: u32);
    fn sceSdGetParam(entry: u32) -> u32;
    fn sceSdSetSwitch(entry: u32, value: u32);
    fn sceSdSetAddr(entry: u32, value: u32);
    fn sceSdVoiceTrans(channel: i32, mode: u32, iop_address: *mut u8, spu_address: u32, size: u32) -> i32;
    fn sceSdBlockTrans(channel: i32, mode: u32, iop_address: *mut u8, size: u32, start: u32) -> i32;
    fn sceSdVoiceTransStatus(channel: i32, flag: i32) -> u32;
    fn sceSdBlockTransStatus(channel: i32, flag: i32) -> u32;
    fn sceSdSetTransCallback(core: i32, callback: Option<extern "C" fn(*mut u8) -> i32>) -> usize;
}

const SERVER_ID: u32 = 0x5053_534E;
/// Tamaño de los mensajes RPC.
const BUFFER: usize = 16 * 1024;

/// Arranca el SPU2 y el flujo. Respuesta: `[estado]` (0 = bien).
const FN_INIT: u32 = 1;
/// `[n, muestras…]`: encola `n` muestras estéreo (I, D intercaladas, `i16`).
/// Respuesta: estado del flujo (ver [`status`]) con las aceptadas en `[0]`.
const FN_WRITE: u32 = 2;
/// Respuesta: estado del flujo.
const FN_STATUS: u32 = 3;
/// `[izquierda, derecha]` (0..=0x3FFF): volumen del flujo.
const FN_STREAM_VOLUME: u32 = 4;
/// `[dirección, bytes, datos…]`: copia datos a la memoria de sonido.
const FN_UPLOAD: u32 = 5;
/// `[voz, dirección, tono, izquierda, derecha]`: toca una muestra ADPCM.
const FN_PLAY: u32 = 6;
/// `[máscara]`: suelta esas voces (un bit por voz).
const FN_STOP: u32 = 7;
/// `[dirección, bytes]`: lee la memoria de sonido. Respuesta: `[bytes, datos…]`.
const FN_READ: u32 = 8;
/// `[izquierda, derecha]` (0..=0x3FFF): volumen general.
const FN_MASTER_VOLUME: u32 = 9;

// Registros de libsd: `registro << 8 | voz << 1 | núcleo`. Todo el sonido
// sale por el núcleo 1, que es la etapa final de mezcla.
const CORE: u32 = 1;
const VOICE_VOLL: u32 = 0x0000;
const VOICE_VOLR: u32 = 0x0100;
const VOICE_PITCH: u32 = 0x0200;
const VOICE_ADSR1: u32 = 0x0300;
const VOICE_ADSR2: u32 = 0x0400;
const VOICE_ENVELOPE: u32 = 0x0500;
const MASTER_VOLL: u32 = 0x0980;
const MASTER_VOLR: u32 = 0x0A80;
const INPUT_VOLL: u32 = 0x0D80;
const INPUT_VOLR: u32 = 0x0E80;
const STREAM_VOLL: u32 = 0x0F80;
const STREAM_VOLR: u32 = 0x1080;
const KEY_ON: u32 = 0x1500;
const KEY_OFF: u32 = 0x1600;
const VOICE_START: u32 = 0x2000;
const TRANSFER_LOOP: u32 = 0x10;
const TRANSFER_READ: u32 = 1;

/// Muestras estéreo por medio búfer del SPU2.
const BLOCK: usize = 512;
/// Búfer que el SPU2 recorre en bucle: dos mitades de 512 muestras estéreo,
/// cada una como I[256] D[256] I[256] D[256].
#[repr(C, align(16))]
struct Transfer([i16; BLOCK * 4]);
static mut TRANSFER: Transfer = Transfer([0; BLOCK * 4]);

/// Cola de muestras del flujo (unos 340 ms).
const RING: usize = 16384;
struct Stream {
    ring: [[i16; 2]; RING],
    read: usize,
    queued: usize,
    /// Medios búferes entregados al SPU2 desde el arranque.
    blocks: u32,
    /// Veces que la cola se quedó sin datos con el flujo sonando.
    underruns: u32,
    playing: bool,
}
static mut STREAM: Stream = Stream { ring: [[0; 2]; RING], read: 0, queued: 0, blocks: 0, underruns: 0, playing: false };

/// Semáforo que despierta al hilo del flujo en cada medio búfer.
static mut HALF_DONE: i32 = -1;

/// Se ejecuta dentro de la interrupción del SPU2: sólo avisa al hilo.
extern "C" fn half_done(_data: *mut u8) -> i32 {
    unsafe { kernel::iSignalSema(HALF_DONE) };
    1
}

/// Hilo del flujo: en cada aviso rellena la mitad que el SPU2 acaba de soltar.
fn feeder() {
    let mut next = [[0i16; 2]; BLOCK];
    loop {
        // El bloque siguiente se prepara antes de esperar.
        critical(|| unsafe {
            let s = &mut *addr_of_mut!(STREAM);
            let take = s.queued.min(BLOCK);
            for (i, slot) in next.iter_mut().enumerate() {
                *slot = if i < take { s.ring[(s.read + i) % RING] } else { [0; 2] };
            }
            s.read = (s.read + take) % RING;
            s.queued -= take;
            if s.playing && take < BLOCK {
                s.underruns += 1;
                s.playing = false;
            }
        });
        unsafe {
            kernel::WaitSema(HALF_DONE);
            critical(|| {
                let half = 1 - (sceSdBlockTransStatus(CORE as i32, 0) >> 24) as usize;
                let transfer = &mut *addr_of_mut!(TRANSFER);
                let out = &mut transfer.0[(half & 1) * BLOCK * 2..][..BLOCK * 2];
                for (i, frame) in next.iter().enumerate() {
                    let base = (i / 256) * 512 + i % 256;
                    out[base] = frame[0];
                    out[base + 256] = frame[1];
                }
                (*addr_of_mut!(STREAM)).blocks += 1;
            });
        }
    }
}

unsafe fn param(register: u32, value: u32) {
    sceSdSetParam(register | CORE, value);
}

/// Arranca el SPU2, las voces y el flujo.
unsafe fn init() -> u32 {
    if sceSdInit(0) < 0 {
        return 1;
    }
    let semaphore = kernel::SemaParam { attr: 0, option: 0, initial: 0, max: 1 };
    HALF_DONE = kernel::CreateSema(&semaphore);
    if HALF_DONE < 0 {
        return 2;
    }
    // El núcleo 0 calla; el 1 mezcla sus voces, el flujo y lo que venga del 0.
    for register in [INPUT_VOLL, INPUT_VOLR] {
        param(register, 0x7FFF);
    }
    for register in [STREAM_VOLL, STREAM_VOLR, MASTER_VOLL, MASTER_VOLR] {
        sceSdSetParam(register, 0);
        param(register, 0x3FFF);
    }
    // Voces: a todo volumen al instante y hasta que acabe la muestra.
    sceSdSetSwitch(KEY_OFF | CORE, 0xFF_FFFF);
    for voice in 0..24 {
        param(VOICE_ADSR1 | voice << 1, 0x00FF);
        param(VOICE_ADSR2 | voice << 1, 0x1FC0);
    }
    sceSdSetTransCallback(CORE as i32, Some(half_done));
    let transfer = addr_of_mut!(TRANSFER.0) as *mut u8;
    if sceSdBlockTrans(CORE as i32, TRANSFER_LOOP, transfer, (BLOCK * 8) as u32, 0) < 0 {
        return 3;
    }
    // Más prioridad que el hilo del RPC (40): el sonido no espera.
    if thread::Builder::new().priority(39).stack_size(8 * 1024).spawn(feeder).is_none() {
        return 4;
    }
    println!("ps2snd: SPU2 listo, flujo PCM de 48 kHz en marcha");
    0
}

fn word(data: &[u8], index: usize) -> u32 {
    u32::from_le_bytes([data[index * 4], data[index * 4 + 1], data[index * 4 + 2], data[index * 4 + 3]])
}

fn put(data: &mut [u8], index: usize, value: u32) {
    data[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
}

/// Estado del flujo: `[aceptadas, en cola, bloques, cortes, voces calladas, hueco]`.
unsafe fn status(data: &mut [u8], accepted: u32) {
    let (queued, blocks, underruns) = critical(|| {
        let s = &*addr_of_mut!(STREAM);
        (s.queued as u32, s.blocks, s.underruns)
    });
    put(data, 0, accepted);
    put(data, 1, queued);
    put(data, 2, blocks);
    put(data, 3, underruns);
    // Una voz está callada cuando su envolvente ha llegado a cero.
    let mut idle = 0u32;
    for voice in 0..24 {
        if sceSdGetParam(VOICE_ENVELOPE | voice << 1 | CORE) & 0xFFFF == 0 {
            idle |= 1 << voice;
        }
    }
    put(data, 4, idle);
    put(data, 5, RING as u32 - queued);
}

fn main() {
    let mut ready = false;
    rpc::serve(SERVER_ID, BUFFER, move |function, data, received| unsafe {
        if function == FN_INIT {
            let result = if ready { 0 } else { init() };
            ready = result == 0;
            put(data, 0, result);
            return;
        }
        if !ready {
            put(data, 0, u32::MAX);
            return;
        }
        match function {
            FN_WRITE => {
                let wanted = (word(data, 0) as usize).min(received.saturating_sub(4) / 4);
                let accepted = critical(|| {
                    let s = &mut *addr_of_mut!(STREAM);
                    let count = wanted.min(RING - s.queued);
                    for i in 0..count {
                        let at = 4 + i * 4;
                        let frame = [i16::from_le_bytes([data[at], data[at + 1]]), i16::from_le_bytes([data[at + 2], data[at + 3]])];
                        s.ring[(s.read + s.queued + i) % RING] = frame;
                    }
                    s.queued += count;
                    if count > 0 {
                        s.playing = true;
                    }
                    count
                });
                status(data, accepted as u32);
            }
            FN_STATUS => status(data, 0),
            FN_STREAM_VOLUME | FN_MASTER_VOLUME => {
                let (left, right) = if function == FN_STREAM_VOLUME { (STREAM_VOLL, STREAM_VOLR) } else { (MASTER_VOLL, MASTER_VOLR) };
                param(left, word(data, 0).min(0x3FFF));
                param(right, word(data, 1).min(0x3FFF));
                put(data, 0, 0);
            }
            FN_UPLOAD => {
                let (address, size) = (word(data, 0), word(data, 1).min((data.len() - 64) as u32));
                // Por el canal 0: el 1 lo ocupa el flujo.
                let sent = sceSdVoiceTrans(0, 0, data.as_mut_ptr().add(8), address, size);
                if sent >= 0 {
                    sceSdVoiceTransStatus(0, 1);
                }
                put(data, 0, sent as u32);
            }
            FN_PLAY => {
                let voice = word(data, 0).min(23);
                param(VOICE_PITCH | voice << 1, word(data, 2).min(0x3FFF));
                param(VOICE_VOLL | voice << 1, word(data, 3).min(0x3FFF));
                param(VOICE_VOLR | voice << 1, word(data, 4).min(0x3FFF));
                sceSdSetAddr(VOICE_START | voice << 1 | CORE, word(data, 1));
                sceSdSetSwitch(KEY_ON | CORE, 1 << voice);
                put(data, 0, 0);
            }
            FN_STOP => {
                sceSdSetSwitch(KEY_OFF | CORE, word(data, 0) & 0xFF_FFFF);
                put(data, 0, 0);
            }
            FN_READ => {
                let (address, size) = (word(data, 0), (word(data, 1) as usize).min(data.len() - 64) & !63);
                let got = sceSdVoiceTrans(0, TRANSFER_READ, data.as_mut_ptr().add(64), address, size as u32);
                if got >= 0 {
                    sceSdVoiceTransStatus(0, 1);
                }
                put(data, 0, got as u32);
            }
            _ => put(data, 0, u32::MAX),
        }
    });
}

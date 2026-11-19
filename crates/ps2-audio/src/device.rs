//! La conexión con `ps2snd.irx`, el módulo de sonido del IOP.

use crate::{adpcm, pitch, MAX_VOLUME, VOICES};
use std::io;
use std::os::ps2::{exe_dir, flush_data_cache, load_iop_module, SifRpcClient};
use std::thread;
use std::time::Duration;

const SERVER_ID: u32 = 0x5053_534E;
const BUFFER: usize = 16 * 1024;
const FN_INIT: u32 = 1;
const FN_WRITE: u32 = 2;
const FN_STATUS: u32 = 3;
const FN_STREAM_VOLUME: u32 = 4;
const FN_UPLOAD: u32 = 5;
const FN_PLAY: u32 = 6;
const FN_STOP: u32 = 7;
const FN_READ: u32 = 8;
const FN_MASTER_VOLUME: u32 = 9;

/// Memoria de sonido para muestras: desde detrás de la zona reservada del
/// chip hasta dejar libre el final, donde trabajan los efectos.
const SAMPLE_MEMORY: core::ops::Range<u32> = 0x5010..0x1C_0000;
/// Muestras estéreo que caben en un mensaje.
const FRAMES_PER_CALL: usize = (BUFFER - 64) / 4;
/// Bytes de memoria de sonido por mensaje (múltiplo de 64).
const BYTES_PER_CALL: usize = 8192;

#[repr(C, align(64))]
struct Aligned([u8; BUFFER]);

/// Estado del sonido.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// Muestras estéreo del flujo que aún esperan en la cola.
    pub queued: usize,
    /// Muestras que caben ahora mismo en la cola.
    pub free: usize,
    /// Bloques de 512 muestras entregados al chip desde el arranque (93,75
    /// por segundo, haya datos o no).
    pub blocks: u32,
    /// Veces que el flujo se quedó sin datos (también cuenta cuando termina
    /// lo que se envió y no llega más).
    pub underruns: u32,
    /// Voces que no están sonando (un bit por voz).
    pub idle_voices: u32,
}

/// Una muestra ADPCM ya subida a la memoria de sonido.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    address: u32,
    bytes: u32,
    rate: u32,
}

impl Sample {
    /// Dirección en la memoria de sonido.
    pub fn address(&self) -> u32 {
        self.address
    }

    /// Bytes que ocupa.
    pub fn len(&self) -> usize {
        self.bytes as usize
    }

    pub fn is_empty(&self) -> bool {
        self.bytes == 0
    }
}

/// El sonido de la consola. Sólo puede haber uno.
pub struct Audio {
    client: Box<SifRpcClient>,
    send: Box<Aligned>,
    recv: Box<Aligned>,
    /// Siguiente dirección libre para muestras.
    next: u32,
}

fn volume(v: f32) -> u32 {
    (v.clamp(0.0, 1.0) * MAX_VOLUME as f32) as u32
}

impl Audio {
    /// Arranca el sonido: carga `rom0:LIBSD` y `ps2snd.irx` (de la carpeta
    /// del ejecutable) si hace falta, e inicia el chip y el flujo PCM.
    pub fn init() -> io::Result<Audio> {
        let mut client = Box::new(SifRpcClient::new());
        // Puede que ya esté cargado (de una llamada anterior).
        if !client.bind(SERVER_ID, 1) {
            // La biblioteca de la BIOS; si ya estaba, el error no importa.
            let _ = load_iop_module("rom0:LIBSD");
            let dir = exe_dir();
            let dir = dir.to_string_lossy();
            let loaded = ["ps2snd.irx", "PS2SND.IRX", "PS2SND.IRX;1"].iter().any(|name| load_iop_module(&format!("{dir}{name}")).is_ok_and(|r| r >= 0));
            if !loaded {
                return Err(io::Error::new(io::ErrorKind::NotFound, "ps2snd.irx not found next to the executable"));
            }
            if !client.bind(SERVER_ID, 200) {
                return Err(io::Error::other("ps2snd.irx does not answer"));
            }
        }
        let mut audio = Audio { client, send: Box::new(Aligned([0; BUFFER])), recv: Box::new(Aligned([0; BUFFER])), next: SAMPLE_MEMORY.start };
        flush_data_cache();
        match audio.call(FN_INIT, &[], &[], 16)[0] {
            0 => Ok(audio),
            code => Err(io::Error::other(format!("the sound chip did not start (error {code})"))),
        }
    }

    /// Una llamada al módulo: palabras de cabecera, datos, y la respuesta
    /// como palabras (las primeras ocho).
    fn call(&mut self, function: u32, words: &[u32], data: &[u8], reply_bytes: usize) -> [u32; 8] {
        let size = words.len() * 4 + data.len();
        assert!(size <= BUFFER, "mensaje demasiado grande");
        // El DMA no pasa por la caché: se usa el espejo sin caché de la RAM.
        let send = (self.send.0.as_mut_ptr() as usize | 0x2000_0000) as *mut u8;
        let recv = (self.recv.0.as_mut_ptr() as usize | 0x2000_0000) as *mut u8;
        let mut reply = [0u32; 8];
        unsafe {
            for (i, word) in words.iter().enumerate() {
                send.add(i * 4).copy_from_nonoverlapping(word.to_le_bytes().as_ptr(), 4);
            }
            send.add(words.len() * 4).copy_from_nonoverlapping(data.as_ptr(), data.len());
            let reply_size = (reply_bytes.max(32) + 63) & !63;
            self.client.call(function, send, ((size.max(16) + 15) & !15) as u32, recv, reply_size.min(BUFFER) as u32);
            for (i, word) in reply.iter_mut().enumerate() {
                let mut bytes = [0u8; 4];
                recv.add(i * 4).copy_to_nonoverlapping(bytes.as_mut_ptr(), 4);
                *word = u32::from_le_bytes(bytes);
            }
        }
        reply
    }

    fn to_status(reply: &[u32; 8]) -> Status {
        Status { queued: reply[1] as usize, blocks: reply[2], underruns: reply[3], idle_voices: reply[4], free: reply[5] as usize }
    }

    pub fn status(&mut self) -> Status {
        Audio::to_status(&self.call(FN_STATUS, &[], &[], 32))
    }

    /// Encola muestras estéreo de 48 kHz en el flujo, sin esperar. Devuelve
    /// cuántas cupieron.
    pub fn write(&mut self, frames: &[[i16; 2]]) -> usize {
        let mut done = 0;
        while done < frames.len() {
            let chunk = &frames[done..(done + FRAMES_PER_CALL).min(frames.len())];
            // `[i16; 2]` ya es el formato del mensaje: I, D, I, D…
            let bytes = unsafe { core::slice::from_raw_parts(chunk.as_ptr() as *const u8, chunk.len() * 4) };
            let accepted = self.call(FN_WRITE, &[chunk.len() as u32], bytes, 32)[0] as usize;
            done += accepted;
            if accepted < chunk.len() {
                break;
            }
        }
        done
    }

    /// Encola todas las muestras, esperando a que haya sitio en la cola.
    pub fn write_all(&mut self, mut frames: &[[i16; 2]]) {
        while !frames.is_empty() {
            let accepted = self.write(frames);
            frames = &frames[accepted..];
            if !frames.is_empty() {
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Espera a que el flujo haya sonado entero.
    pub fn drain(&mut self) {
        while self.status().queued > 0 {
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Volumen del flujo PCM, de 0.0 a 1.0 por canal.
    pub fn set_stream_volume(&mut self, left: f32, right: f32) {
        self.call(FN_STREAM_VOLUME, &[volume(left), volume(right)], &[], 16);
    }

    /// Volumen general (flujo y voces), de 0.0 a 1.0 por canal.
    pub fn set_master_volume(&mut self, left: f32, right: f32) {
        self.call(FN_MASTER_VOLUME, &[volume(left), volume(right)], &[], 16);
    }

    /// Bytes de memoria de sonido libres para muestras.
    pub fn sample_memory_free(&self) -> usize {
        (SAMPLE_MEMORY.end - self.next) as usize
    }

    /// Sube datos ADPCM ya codificados (bloques de 16 bytes, ver [`adpcm`])
    /// grabados a `rate` Hz.
    pub fn load_adpcm(&mut self, data: &[u8], rate: u32) -> io::Result<Sample> {
        if data.is_empty() || data.len() % 16 != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "ADPCM data must be whole 16-byte blocks"));
        }
        // El chip recibe de 64 en 64 bytes: cada muestra ocupa un múltiplo.
        let reserved = ((data.len() + 63) & !63) as u32;
        if reserved as usize > self.sample_memory_free() {
            return Err(io::Error::new(io::ErrorKind::OutOfMemory, "sound memory is full"));
        }
        let address = (self.next + 15) & !15;
        let mut padded = data.to_vec();
        padded.resize(reserved as usize, 0);
        for (i, chunk) in padded.chunks(BYTES_PER_CALL).enumerate() {
            let at = address + (i * BYTES_PER_CALL) as u32;
            if (self.call(FN_UPLOAD, &[at, chunk.len() as u32], chunk, 16)[0] as i32) < 0 {
                return Err(io::Error::other("the sound chip rejected the sample"));
            }
        }
        self.next = address + reserved;
        Ok(Sample { address, bytes: data.len() as u32, rate })
    }

    /// Codifica PCM mono de 16 bits grabado a `rate` Hz y lo sube. Con
    /// `looped` la muestra se repite hasta que se pare la voz.
    pub fn load_sample(&mut self, pcm: &[i16], rate: u32, looped: bool) -> io::Result<Sample> {
        self.load_adpcm(&adpcm::encode(pcm, looped), rate)
    }

    /// Olvida todas las muestras subidas: su memoria queda libre. Las
    /// [`Sample`] anteriores dejan de valer.
    pub fn free_samples(&mut self) {
        self.next = SAMPLE_MEMORY.start;
    }

    /// Toca una muestra en la voz `voice` (0..24) a su tono original, con
    /// volumen de 0.0 a 1.0 por canal. Si la voz estaba sonando, la corta.
    pub fn play(&mut self, voice: u8, sample: &Sample, left: f32, right: f32) {
        self.play_at(voice, sample, left, right, 1.0);
    }

    /// Como [`play`](Audio::play), `speed` veces más rápido (y más agudo).
    pub fn play_at(&mut self, voice: u8, sample: &Sample, left: f32, right: f32, speed: f32) {
        assert!(voice < VOICES, "el SPU2 tiene 24 voces");
        let words = [voice as u32, sample.address, pitch(sample.rate, speed) as u32, volume(left), volume(right)];
        self.call(FN_PLAY, &words, &[], 16);
    }

    /// Para una voz.
    pub fn stop(&mut self, voice: u8) {
        self.call(FN_STOP, &[1 << (voice % VOICES)], &[], 16);
    }

    /// Para todas las voces.
    pub fn stop_all(&mut self) {
        self.call(FN_STOP, &[0xFF_FFFF], &[], 16);
    }

    /// ¿Sigue sonando la voz?
    pub fn is_playing(&mut self, voice: u8) -> bool {
        self.status().idle_voices & (1 << (voice % VOICES)) == 0
    }

    /// Una voz que no esté sonando, si la hay.
    pub fn free_voice(&mut self) -> Option<u8> {
        let idle = self.status().idle_voices;
        (0..VOICES).find(|v| idle & (1 << v) != 0)
    }

    /// Lee la memoria de sonido del chip (diagnóstico).
    pub fn read_memory(&mut self, address: u32, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        let mut at = address;
        while out.len() < len {
            let chunk = (len - out.len()).div_ceil(64).min(BYTES_PER_CALL / 64) * 64;
            if (self.call(FN_READ, &[at, chunk as u32], &[], 64 + chunk)[0] as i32) < 0 {
                break;
            }
            let recv = (self.recv.0.as_ptr() as usize | 0x2000_0000) as *const u8;
            let start = out.len();
            out.resize(start + chunk, 0);
            unsafe { recv.add(64).copy_to_nonoverlapping(out[start..].as_mut_ptr(), chunk) };
            at += chunk as u32;
        }
        out.truncate(len);
        out
    }
}

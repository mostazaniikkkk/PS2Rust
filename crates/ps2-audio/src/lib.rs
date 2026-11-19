//! Sonido para la PlayStation 2.
//!
//! ```ignore
//! use ps2_audio::{Audio, to_stream};
//!
//! let mut audio = Audio::init()?;
//!
//! // Flujo PCM (música, o la mezcla de tu motor): estéreo, 16 bits, 48 kHz.
//! let frames: Vec<[i16; 2]> = to_stream(&[0i16; 44100], 1, 44100);
//! audio.write_all(&frames);
//!
//! // Efectos: muestras ADPCM en cualquiera de las 24 voces del chip.
//! let beep = audio.load_sample(&[0i16; 4000], 22050, false)?;
//! audio.play(0, &beep, 1.0, 1.0);
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! El chip de sonido (SPU2) está en el IOP. [`Audio::init`] carga la
//! biblioteca de sonido de la BIOS (`rom0:LIBSD`) y `ps2snd.irx`, un módulo
//! en Rust que `cargo ps2` deja junto al ejecutable, y habla con él por RPC.
//!
//! [`adpcm`] y [`to_stream`] no dependen de la consola: también compilan en
//! el PC.

pub mod adpcm;

#[cfg(target_os = "ps2")]
mod device;
#[cfg(target_os = "ps2")]
pub use device::{Audio, Sample, Status};

/// Frecuencia del flujo PCM y de referencia de las voces.
pub const RATE: u32 = 48000;
/// Volumen máximo de los registros del SPU2.
pub const MAX_VOLUME: u16 = 0x3FFF;
/// Voces del SPU2 disponibles para muestras.
pub const VOICES: u8 = 24;

/// Convierte PCM de 16 bits con `channels` canales intercalados (1 o 2) y
/// `rate` Hz al formato del flujo: estéreo a 48 kHz. Interpola linealmente.
pub fn to_stream(pcm: &[i16], channels: usize, rate: u32) -> Vec<[i16; 2]> {
    assert!(channels == 1 || channels == 2, "sólo mono o estéreo");
    assert!(rate > 0, "frecuencia no válida");
    let frames = pcm.len() / channels;
    let frame = |i: usize| -> [i32; 2] {
        let i = i.min(frames.saturating_sub(1));
        if channels == 1 {
            [pcm[i] as i32; 2]
        } else {
            [pcm[i * 2] as i32, pcm[i * 2 + 1] as i32]
        }
    };
    if frames == 0 {
        return Vec::new();
    }
    let out_len = (frames as u64 * RATE as u64 / rate as u64) as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        // Posición en la entrada, en coma fija con 16 bits de fracción.
        let position = n as u64 * ((rate as u64) << 16) / RATE as u64;
        let (index, fraction) = ((position >> 16) as usize, (position & 0xffff) as i32);
        let (a, b) = (frame(index), frame(index + 1));
        out.push([(a[0] + (((b[0] - a[0]) * fraction) >> 16)) as i16, (a[1] + (((b[1] - a[1]) * fraction) >> 16)) as i16]);
    }
    out
}

/// Valor del registro de tono de una voz para una muestra grabada a `rate`
/// Hz, tocada `speed` veces más rápido (1.0 = tono original).
pub fn pitch(rate: u32, speed: f32) -> u16 {
    ((rate as f32 * speed * 4096.0 / RATE as f32) as u32).clamp(1, 0x3FFF) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling() {
        // A 48 kHz estéreo no cambia nada.
        assert_eq!(to_stream(&[1, 2, 3, 4], 2, 48000), [[1, 2], [3, 4]]);
        // Mono se duplica.
        assert_eq!(to_stream(&[5, -6], 1, 48000), [[5, 5], [-6, -6]]);
        // De 24 kHz al doble de muestras, interpolando.
        assert_eq!(to_stream(&[0, 100, 200], 1, 24000), [[0, 0], [50, 50], [100, 100], [150, 150], [200, 200], [200, 200]]);
        // 44,1 kHz: un segundo sigue durando un segundo.
        let second: Vec<i16> = (0..44100).map(|i| (i % 100) as i16).collect();
        assert_eq!(to_stream(&second, 1, 44100).len(), 48000);
        assert!(to_stream(&[], 2, 44100).is_empty());
    }

    #[test]
    fn pitch_register() {
        assert_eq!(pitch(48000, 1.0), 0x1000);
        assert_eq!(pitch(24000, 1.0), 0x0800);
        assert_eq!(pitch(24000, 2.0), 0x1000);
        assert_eq!(pitch(48000, 100.0), 0x3FFF);
    }
}

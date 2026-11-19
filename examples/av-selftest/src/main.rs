//! Batería de mando y sonido (crates `ps2-pad` y `ps2-audio`), sin tocar el
//! mando: comprueba lo que la consola dice por sí sola. El código de salida
//! es el número de fallos.
//!
//! Supone un DualShock 2 en el puerto 1 y nada en el puerto 2 (la
//! configuración habitual de PCSX2). Hace sonar tonos cortos y bajos.
//!
//! Los botones se prueban aparte, con teclas simuladas: ver `src/bin/pad-keys.rs`.

use ps2_audio::{adpcm, Audio};
use ps2_pad::{Driver, Input, Kind, Pad, Pads, State};
use std::thread::sleep;
use std::time::{Duration, Instant};

struct T {
    total: u32,
    failed: u32,
}

impl T {
    fn section(&mut self, name: &str) {
        println!("== {name}");
    }

    fn check(&mut self, name: &str, ok: bool) {
        self.total += 1;
        self.failed += !ok as u32;
        println!("[{}] {name}", if ok { "ok" } else { "FALLO" });
    }

    fn expect<V: std::fmt::Debug>(&mut self, name: &str, ok: bool, got: V) {
        self.check(name, ok);
        if !ok {
            println!("     se obtuvo {got:?}");
        }
    }
}

/// Sondea el mando una vez por frame hasta que entregue datos.
fn wait_input(pad: &mut Pad, limit: Duration) -> Option<Input> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(input) = pad.poll() {
            return Some(input);
        }
        sleep(Duration::from_millis(16));
    }
    None
}

fn pads(t: &mut T, driver: Driver, name: &str) {
    t.section(name);
    let pads = Pads::init_with(driver);
    t.expect("los modulos de la BIOS cargan y responden", pads.is_ok(), pads.as_ref().err());
    let Ok(mut pads) = pads else { return };
    t.check("driver()", pads.driver() == driver);
    let pad = pads.open(0);
    t.expect("abrir el puerto 1", pad.is_ok(), pad.as_ref().err());
    let Ok(mut pad) = pad else { return };
    t.check("no se puede abrir dos veces", pads.open(0).is_err());
    t.check("no existe el puerto 3", pads.open(2).is_err());
    t.expect("recien abierto: ocupado", pad.state() == State::Busy, pad.state());
    let input = wait_input(&mut pad, Duration::from_secs(4));
    t.expect("el mando queda listo solo", input.is_some() && pad.state() == State::Ready, pad.state());
    if let Some(input) = input {
        t.expect("DualShock 2 con presion", input.kind == Kind::Pressure, input.kind);
        t.expect("ningun boton pulsado", input.buttons == 0 && input.pressure == [0; 12], input);
        let centered = |v: u8| (126..=129).contains(&v);
        t.expect("sticks en el centro", [input.left.0, input.left.1, input.right.0, input.right.1].iter().all(|&v| centered(v)), (input.left, input.right));
        let (x, y) = input.left_stick();
        t.check("left_stick() cerca de (0, 0)", x.abs() < 0.02 && y.abs() < 0.02);
    }
    t.expect("tiene motores de vibracion", pad.has_rumble() == Some(true), pad.has_rumble());
    t.check("vibracion: motor grande", pad.rumble(false, 180));
    sleep(Duration::from_millis(60));
    t.check("vibracion: los dos motores", pad.rumble(true, 255));
    sleep(Duration::from_millis(60));
    t.check("vibracion: parar", pad.rumble(false, 0));
    t.check("tras vibrar sigue entregando datos", wait_input(&mut pad, Duration::from_secs(1)).is_some());
    // Puerto 2: abierto, pero sin mando.
    let second = pads.open(1);
    t.expect("abrir el puerto 2", second.is_ok(), second.as_ref().err());
    if let Ok(mut second) = second {
        let mut seen = None;
        for _ in 0..40 {
            seen = second.poll();
            sleep(Duration::from_millis(16));
        }
        t.expect("puerto 2 sin mando: desconectado", seen.is_none() && second.state() == State::Disconnected, second.state());
        t.check("sin mando no hay vibracion", !second.rumble(true, 255));
    }
    // Cerrar y volver a abrir.
    drop(pad);
    let again = pads.open(0);
    t.check("cerrar y reabrir el puerto 1", again.is_ok_and(|mut p| wait_input(&mut p, Duration::from_secs(4)).is_some()));
}

/// Amplitud máxima y cruces por cero de un trozo de memoria de sonido,
/// leído como muestras de 16 bits.
fn measure(bytes: &[u8]) -> (u16, usize) {
    let samples: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    let crossings = samples.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
    (peak, crossings)
}

fn sine(samples: usize, frequency: f32, rate: f32, amplitude: f32) -> Vec<i16> {
    (0..samples).map(|i| ((i as f32 * frequency * std::f32::consts::TAU / rate).sin() * amplitude) as i16).collect()
}

// Zonas de la memoria de sonido donde el chip va dejando, en vueltas de 512
// muestras, lo que le entra y lo que mezcla (núcleo 1).
const STREAM_INPUT_LEFT: u32 = 0x4800;
const STREAM_INPUT_RIGHT: u32 = 0x4C00;
const VOICE_1_OUTPUT: u32 = 0x1800;
const VOICE_MIX_LEFT: u32 = 0x3000;

fn audio(t: &mut T) {
    t.section("sonido: flujo PCM");
    let audio = Audio::init();
    t.expect("Audio::init (rom0:LIBSD + ps2snd.irx)", audio.is_ok(), audio.as_ref().err());
    let Ok(mut audio) = audio else { return };
    let before = audio.status();
    sleep(Duration::from_millis(640));
    let after = audio.status();
    // 93,75 bloques de 512 muestras por segundo.
    let blocks = after.blocks - before.blocks;
    t.expect("el chip consume 48000 muestras por segundo", (54..=66).contains(&blocks), blocks);
    t.expect("cola vacia al empezar", after.queued == 0 && after.free >= 8192, after);
    // 0,4 s: 1 kHz por la izquierda y 3 kHz por la derecha, flojito.
    let (left, right) = (sine(19200, 1000.0, 48000.0, 1500.0), sine(19200, 3000.0, 48000.0, 1500.0));
    let frames: Vec<[i16; 2]> = left.iter().zip(&right).map(|(&l, &r)| [l, r]).collect();
    let accepted = audio.write(&frames);
    // Caben las 16384 de la cola y alguna más que el chip va gastando entretanto.
    t.expect("write llena la cola y dice cuanto cupo", (16384..19200).contains(&accepted), accepted);
    audio.write_all(&frames[accepted..]);
    let queued = audio.status().queued;
    t.expect("las muestras esperan en la cola", queued > 4000 && queued <= 16384, queued);
    sleep(Duration::from_millis(60));
    let (peak_l, crossings_l) = measure(&audio.read_memory(STREAM_INPUT_LEFT, 1024));
    let (peak_r, crossings_r) = measure(&audio.read_memory(STREAM_INPUT_RIGHT, 1024));
    // 512 muestras de 1 kHz a 48 kHz son 10,7 ciclos: 21 cruces; de 3 kHz, 64.
    t.expect("al chip le llega el tono de 1 kHz por la izquierda", (1400..=1500).contains(&peak_l) && (20..=23).contains(&crossings_l), (peak_l, crossings_l));
    t.expect("y el de 3 kHz por la derecha", (1400..=1500).contains(&peak_r) && (62..=66).contains(&crossings_r), (peak_r, crossings_r));
    audio.set_stream_volume(0.5, 0.5);
    audio.set_master_volume(0.8, 0.8);
    let start = Instant::now();
    audio.drain();
    let took = start.elapsed();
    t.expect("drain espera a que suene todo", audio.status().queued == 0 && took < Duration::from_millis(600), took);
    sleep(Duration::from_millis(40));
    let (silence, _) = measure(&audio.read_memory(STREAM_INPUT_LEFT, 1024));
    t.expect("sin datos, al chip le llega silencio", silence == 0, silence);
    t.check("el fin del flujo queda anotado", audio.status().underruns == 1);
    audio.set_stream_volume(1.0, 1.0);
    audio.set_master_volume(1.0, 1.0);

    t.section("sonido: muestras ADPCM en las voces");
    t.expect("las 24 voces calladas", audio.status().idle_voices == 0xFF_FFFF && audio.free_voice() == Some(0), audio.status());
    // 0,3 s de 440 Hz grabados a 22050 Hz.
    let pcm = sine(6615, 440.0, 22050.0, 4000.0);
    let encoded = adpcm::encode(&pcm, false);
    let free = audio.sample_memory_free();
    let sample = audio.load_sample(&pcm, 22050, false);
    t.expect("load_sample", sample.is_ok(), sample.as_ref().err());
    let Ok(sample) = sample else { return };
    t.check("ocupa memoria de sonido", sample.len() == encoded.len() && audio.sample_memory_free() < free);
    t.check("lo subido se lee igual del chip", audio.read_memory(sample.address(), encoded.len()) == encoded);
    audio.play(1, &sample, 0.25, 0.25);
    sleep(Duration::from_millis(100));
    t.check("la voz 1 suena", audio.is_playing(1) && !audio.is_playing(0));
    t.expect("free_voice se salta la que suena", audio.status().idle_voices == 0xFF_FFFD, audio.status().idle_voices);
    // El chip toca a 48 kHz: 512 muestras de 440 Hz son 4,7 ciclos.
    let (peak, crossings) = measure(&audio.read_memory(VOICE_1_OUTPUT, 1024));
    t.expect("la voz 1 da 440 Hz con la amplitud original", (3300..=4400).contains(&peak) && (8..=11).contains(&crossings), (peak, crossings));
    let (mixed, _) = measure(&audio.read_memory(VOICE_MIX_LEFT, 1024));
    t.expect("y entra en la mezcla al 25% de volumen", (800..=1150).contains(&mixed), mixed);
    sleep(Duration::from_millis(350));
    t.check("al acabar la muestra, la voz calla sola", !audio.is_playing(1));
    // Al doble de velocidad: una octava más arriba.
    audio.play_at(1, &sample, 0.25, 0.25, 2.0);
    sleep(Duration::from_millis(60));
    let (_, crossings) = measure(&audio.read_memory(VOICE_1_OUTPUT, 1024));
    t.expect("play_at(2.0) suena una octava mas alta", (17..=21).contains(&crossings), crossings);
    sleep(Duration::from_millis(200));
    // En bucle: suena hasta que se pare.
    let looped = audio.load_sample(&sine(2205, 441.0, 22050.0, 4000.0), 22050, true);
    if let Ok(looped) = looped {
        t.check("la segunda muestra va detras de la primera", looped.address() >= sample.address() + sample.len() as u32);
        audio.play(3, &looped, 0.2, 0.2);
        audio.play(5, &sample, 0.1, 0.2);
        sleep(Duration::from_millis(250));
        t.check("una muestra en bucle sigue sonando mas alla de su duracion", audio.is_playing(3));
        t.check("dos voces a la vez", audio.is_playing(5));
        audio.stop(3);
        sleep(Duration::from_millis(60));
        t.check("stop la calla", !audio.is_playing(3));
        audio.play(3, &looped, 0.2, 0.2);
        audio.stop_all();
        sleep(Duration::from_millis(60));
        t.expect("stop_all las calla todas", audio.status().idle_voices == 0xFF_FFFF, audio.status().idle_voices);
    } else {
        t.expect("load_sample en bucle", false, looped.err());
    }
    t.check("datos ADPCM a medias se rechazan", audio.load_adpcm(&[0; 20], 22050).is_err());
    audio.free_samples();
    t.check("free_samples devuelve la memoria", audio.sample_memory_free() == free);
    // El flujo sigue en marcha después de todo.
    let before = audio.status().blocks;
    sleep(Duration::from_millis(320));
    let blocks = audio.status().blocks - before;
    t.expect("el flujo no se ha parado", (26..=34).contains(&blocks), blocks);
}

fn main() {
    let mut t = T { total: 0, failed: 0 };
    pads(&mut t, Driver::New, "mando: rom0:XPADMAN");
    // Las dos generaciones de módulos no conviven: se reinicia el IOP.
    std::os::ps2::reset_iop();
    pads(&mut t, Driver::Old, "mando: rom0:PADMAN (el de las primeras consolas)");
    std::os::ps2::reset_iop();
    audio(&mut t);
    println!("== resultado: {} comprobaciones, {} fallos", t.total, t.failed);
    std::process::exit(t.failed as i32);
}

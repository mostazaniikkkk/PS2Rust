//! Mando y sonido: lo que le falta a un motor para ser un juego.
//!
//!     cargo ps2 run -p av-demo --keep-open
//!
//! En pantalla se ve el mando en vivo (botones con su presión y los dos
//! sticks) y el estado del chip de sonido. Con el mando:
//!
//! * **Cruz, Círculo, Cuadrado, Triángulo**: cuatro notas, cada una en una
//!   voz del chip; cuanto más fuerte se pulsa, más alto suena.
//! * **Cruceta izquierda / derecha**: lleva las notas a un altavoz o al otro.
//! * **Start**: una melodía por el flujo PCM.
//! * **L2 / R2**: vibración (motor grande según la presión de L2, pequeño con R2).
//! * **Select + Start**: salir.
//!
//! Al arrancar no suena nada: todo el sonido lo dispara el mando.

use ps2::gs::{Color, Gs, VideoMode};
use ps2_audio::{to_stream, Audio, Sample};
use ps2_pad::{Button, Input, Pads, State};
use std::thread::sleep;
use std::time::Duration;

const BACKGROUND: Color = Color::rgb(14, 16, 30);
const IDLE: Color = Color::rgb(52, 58, 84);
const TEXT: Color = Color::rgb(205, 212, 232);
const ACCENT: Color = Color::rgb(250, 190, 90);

/// Una nota con ataque rápido y caída suave, para que no chasquee.
fn note(frequency: f32, rate: u32, seconds: f32, amplitude: f32) -> Vec<i16> {
    let samples = (rate as f32 * seconds) as usize;
    (0..samples)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let envelope = (t * 200.0).min(1.0) * (1.0 - i as f32 / samples as f32);
            let phase = t * frequency * std::f32::consts::TAU;
            // Fundamental y un poco de armónico, para que no sea un pitido.
            ((ps2::math::sin(phase) + 0.3 * ps2::math::sin(phase * 2.0)) * envelope * amplitude) as i16
        })
        .collect()
}

/// Una melodía corta, mono a 22050 Hz.
fn melody() -> Vec<i16> {
    let notes = [(523.25, 0.18), (659.25, 0.18), (783.99, 0.18), (1046.5, 0.30), (783.99, 0.18), (1046.5, 0.45)];
    notes.iter().flat_map(|&(frequency, seconds)| note(frequency, 22050, seconds, 9000.0)).collect()
}

/// Color de un botón: gris en reposo, más vivo cuanto más fuerte se pulsa.
fn lit(input: Option<&Input>, button: Button, color: (u8, u8, u8)) -> Color {
    match input.map(|i| i.pressure(button)) {
        Some(p) if p > 0 => {
            let k = 90 + p as u32 * 165 / 255;
            Color::rgb((color.0 as u32 * k / 255) as u8, (color.1 as u32 * k / 255) as u8, (color.2 as u32 * k / 255) as u8)
        }
        _ => IDLE,
    }
}

fn draw_pad(gs: &mut Gs, input: Option<&Input>) {
    let white = (235, 238, 250);
    // Gatillos y botones superiores.
    for (button, x, y, label) in [(Button::L2, 70, 150, "L2"), (Button::L1, 70, 176, "L1"), (Button::R2, 250, 150, "R2"), (Button::R1, 250, 176, "R1")] {
        gs.rect(x, y, 60, 20, lit(input, button, white));
        gs.text(x + 18, y + 3, 2, BACKGROUND, label);
    }
    // Cruceta.
    for (button, x, y) in [(Button::Up, 88, 214), (Button::Down, 88, 262), (Button::Left, 64, 238), (Button::Right, 112, 238)] {
        gs.rect(x, y, 22, 22, lit(input, button, white));
    }
    // Botones de acción, con sus colores.
    for (button, x, y, color) in [
        (Button::Triangle, 268, 214, (80, 220, 170)),
        (Button::Cross, 268, 262, (110, 150, 250)),
        (Button::Square, 244, 238, (240, 130, 200)),
        (Button::Circle, 292, 238, (250, 100, 90)),
    ] {
        gs.rect(x, y, 22, 22, lit(input, button, color));
    }
    for (button, x, label) in [(Button::Select, 150, "SEL"), (Button::Start, 196, "STA")] {
        gs.rect(x, 244, 38, 14, lit(input, button, white));
        gs.text(x + 3, 246, 1, BACKGROUND, label);
    }
    // Sticks: un cuadro con el punto donde está cada uno; L3/R3 lo encienden.
    for (button, x, stick) in [(Button::L3, 120, input.map(|i| i.left)), (Button::R3, 204, input.map(|i| i.right))] {
        gs.rect(x, 300, 60, 60, lit(input, button, (120, 130, 170)));
        gs.rect(x + 2, 302, 56, 56, BACKGROUND);
        let (sx, sy) = stick.unwrap_or((128, 128));
        gs.rect(x + 2 + sx as i32 * 48 / 255, 302 + sy as i32 * 48 / 255, 8, 8, ACCENT);
    }
}

fn main() {
    let mut gs = Gs::init(VideoMode::Ntsc);
    let mut pads = Pads::init().expect("no se pudieron cargar los modulos de mandos");
    let mut pad = pads.open(0).expect("no se pudo abrir el puerto 1");
    let mut audio = Audio::init().expect("no se pudo arrancar el sonido");
    println!("av-demo: mando ({:?}) y sonido listos", pads.driver());

    // Una nota de 440 Hz grabada a 24 kHz; las otras salen de tocarla más deprisa.
    let tone: Sample = audio.load_sample(&note(440.0, 24000, 0.5, 14000.0), 24000, false).expect("muestra");
    let tune = to_stream(&melody(), 1, 22050);
    let notes = [(Button::Cross, 0u8, 1.0f32), (Button::Square, 1, 1.25), (Button::Circle, 2, 1.5), (Button::Triangle, 3, 2.0)];

    let mut previous = 0u16;
    let mut frame = 0u32;
    loop {
        let input = pad.poll();
        let state = pad.state();
        if let Some(input) = &input {
            let new = input.buttons & !previous;
            previous = input.buttons;
            // Notas: volumen según la presión, reparto según la cruceta.
            let (left, right) = match (input.pressed(Button::Left), input.pressed(Button::Right)) {
                (true, false) => (1.0, 0.1),
                (false, true) => (0.1, 1.0),
                _ => (0.7, 0.7),
            };
            for (button, voice, speed) in notes {
                if new & button as u16 != 0 {
                    let force = 0.35 + 0.65 * input.pressure(button) as f32 / 255.0;
                    audio.play_at(voice, &tone, left * force, right * force, speed);
                }
            }
            if new & Button::Start as u16 != 0 && !input.pressed(Button::Select) {
                audio.write(&tune);
            }
            pad.rumble(input.pressed(Button::R2), input.pressure(Button::L2));
            if input.pressed(Button::Select) && input.pressed(Button::Start) {
                break;
            }
        } else {
            previous = 0;
        }
        let status = audio.status();

        gs.clear(BACKGROUND);
        gs.rect(40, 50, 560, 4, ACCENT);
        gs.text(40, 66, 5, Color::WHITE, "Mando y sonido");
        let message = match state {
            State::Ready => "DualShock listo: pulsa botones",
            State::Busy => "configurando el mando...",
            State::Disconnected => "conecta un mando en el puerto 1",
        };
        gs.text(40, 116, 2, ACCENT, message);
        draw_pad(&mut gs, input.as_ref());

        gs.text(400, 150, 2, TEXT, "SPU2");
        gs.text(400, 176, 2, TEXT, &format!("bloques {}", status.blocks));
        gs.text(400, 198, 2, TEXT, &format!("cola {:5}", status.queued));
        // Una barra por voz: encendida si suena.
        gs.text(400, 232, 2, TEXT, "voces");
        for voice in 0..24 {
            let playing = status.idle_voices & (1 << voice) == 0;
            gs.rect(400 + voice * 8, 256, 6, 18, if playing { ACCENT } else { IDLE });
        }
        // La cola del flujo, como barra.
        gs.rect(400, 290, 190, 10, IDLE);
        gs.rect(400, 290, (status.queued * 190 / (status.queued + status.free).max(1)) as i32, 10, Color::rgb(110, 200, 140));
        gs.text(400, 312, 1, TEXT, "X O [] /\\ : notas");
        gs.text(400, 326, 1, TEXT, "cruceta: izquierda / derecha");
        gs.text(400, 340, 1, TEXT, "START: melodia   L2 R2: vibrar");
        gs.text(400, 354, 1, TEXT, "SELECT + START: salir");
        gs.present();
        // Para la captura de la documentación: con `cargo ps2 run --keys` y
        // un PCSX2 con el mando en el teclado (ver av-selftest), estas líneas
        // pulsan botones que no suenan. Sin `--keys` son sólo texto.
        if frame == 90 {
            for key in ["Q", "Up", "4", "D", "T"] {
                println!("[tecla] pulsa {key}");
            }
        }
        if frame == 120 {
            println!("pantalla lista");
        }
        frame += 1;
        sleep(Duration::from_millis(1));
    }
    audio.stop_all();
    pad.rumble(false, 0);
    println!("av-demo: fin tras {frame} frames");
}

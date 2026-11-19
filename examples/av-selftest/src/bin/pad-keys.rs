//! Prueba de los botones y sticks del mando con teclas simuladas.
//!
//!     cargo ps2 run -p av-selftest --bin pad-keys --keys
//!
//! El programa pide cada tecla por la consola (`[tecla] pulsa X`) y
//! `cargo ps2 --keys` se la envía a la ventana del emulador. Necesita un
//! PCSX2 con el mando 1 asignado al teclado así:
//!
//! | Mando | Tecla | Mando | Tecla |
//! |---|---|---|---|
//! | Cruceta | flechas | Triángulo, Círculo, Cruz, Cuadrado | I, L, K, J |
//! | Select, Start | Retroceso, Intro | L1, L2, L3 | Q, 1, 2 |
//! | Stick izquierdo | W, A, S, D | R1, R2, R3 | E, 3, 4 |
//! | Stick derecho | T, F, G, H | | |
//!
//! El código de salida es el número de fallos.

use ps2_pad::{Button, Input, Pad, Pads};
use std::thread::sleep;
use std::time::{Duration, Instant};

const BUTTONS: [(Button, &str, &str); 16] = [
    (Button::Up, "Up", "arriba"),
    (Button::Down, "Down", "abajo"),
    (Button::Left, "Left", "izquierda"),
    (Button::Right, "Right", "derecha"),
    (Button::Triangle, "I", "triangulo"),
    (Button::Circle, "L", "circulo"),
    (Button::Cross, "K", "cruz"),
    (Button::Square, "J", "cuadrado"),
    (Button::Select, "Backspace", "select"),
    (Button::Start, "Return", "start"),
    (Button::L1, "Q", "L1"),
    (Button::L2, "1", "L2"),
    (Button::L3, "2", "L3"),
    (Button::R1, "E", "R1"),
    (Button::R2, "3", "R2"),
    (Button::R3, "4", "R3"),
];

/// (tecla, nombre, stick derecho, eje y, hacia el máximo).
const STICKS: [(&str, &str, bool, bool, bool); 8] = [
    ("W", "stick izquierdo arriba", false, true, false),
    ("S", "stick izquierdo abajo", false, true, true),
    ("A", "stick izquierdo a la izquierda", false, false, false),
    ("D", "stick izquierdo a la derecha", false, false, true),
    ("T", "stick derecho arriba", true, true, false),
    ("G", "stick derecho abajo", true, true, true),
    ("F", "stick derecho a la izquierda", true, false, false),
    ("H", "stick derecho a la derecha", true, false, true),
];

/// Sondea el mando hasta que `done` se cumpla (o pase medio segundo) y
/// devuelve la última lectura.
fn wait_for(pad: &mut Pad, done: impl Fn(&Input) -> bool) -> Option<Input> {
    let start = Instant::now();
    let mut last = None;
    while start.elapsed() < Duration::from_millis(500) {
        if let Some(input) = pad.poll() {
            last = Some(input);
            if done(&input) {
                break;
            }
        }
        sleep(Duration::from_millis(8));
    }
    last
}

fn main() {
    let mut pads = Pads::init().expect("modulos de mandos");
    let mut pad = pads.open(0).expect("puerto 1");
    let start = Instant::now();
    while pad.poll().is_none() && start.elapsed() < Duration::from_secs(4) {
        sleep(Duration::from_millis(16));
    }
    let (mut total, mut failed) = (0, 0);
    let mut check = |name: &str, ok: bool, got: Option<Input>| {
        total += 1;
        failed += !ok as u32;
        println!("[{}] {name}", if ok { "ok" } else { "FALLO" });
        if !ok {
            println!("     ultima lectura: {got:?}");
        }
    };

    println!("== botones");
    for (button, key, name) in BUTTONS {
        println!("[tecla] pulsa {key}");
        let down = wait_for(&mut pad, |i| i.buttons == button as u16);
        // Sólo ese botón, a fondo, y los sticks quietos.
        let ok = down.is_some_and(|i| i.buttons == button as u16 && i.pressed(button) && i.pressure(button) == 255 && i.left == (127, 127) && i.right == (127, 127));
        check(&format!("{name}: pulsado"), ok, down);
        println!("[tecla] suelta {key}");
        let up = wait_for(&mut pad, |i| i.buttons == 0);
        check(&format!("{name}: soltado"), up.is_some_and(|i| i.buttons == 0 && i.pressure(button) == 0), up);
    }

    println!("== sticks");
    for (key, name, right, vertical, high) in STICKS {
        let axis = move |i: &Input| {
            let stick = if right { i.right } else { i.left };
            if vertical {
                stick.1
            } else {
                stick.0
            }
        };
        println!("[tecla] pulsa {key}");
        let moved = wait_for(&mut pad, |i| if high { axis(i) > 250 } else { axis(i) < 5 });
        // Sólo se mueve ese eje, hasta el tope, y no se pulsa ningún botón.
        let ok = moved.is_some_and(|i| {
            let all = [i.left.0, i.left.1, i.right.0, i.right.1];
            let centered = all.iter().filter(|&&v| v == 127).count();
            (if high { axis(&i) > 250 } else { axis(&i) < 5 }) && centered == 3 && i.buttons == 0
        });
        check(name, ok, moved);
        println!("[tecla] suelta {key}");
        let back = wait_for(&mut pad, |i| axis(i) == 127);
        check(&format!("{name}: vuelve al centro"), back.is_some_and(|i| axis(&i) == 127), back);
    }

    println!("== combinaciones");
    println!("[tecla] pulsa K");
    println!("[tecla] pulsa Up");
    println!("[tecla] pulsa D");
    let combo = wait_for(&mut pad, |i| i.buttons == Button::Cross as u16 | Button::Up as u16 && i.left.0 > 250);
    check("cruz + arriba + stick a la derecha, a la vez", combo.is_some_and(|i| i.pressed(Button::Cross) && i.pressed(Button::Up) && i.left_stick().0 > 0.95), combo);
    println!("[tecla] suelta K");
    let partial = wait_for(&mut pad, |i| i.buttons == Button::Up as u16);
    check("al soltar cruz sigue arriba", partial.is_some_and(|i| i.buttons == Button::Up as u16 && i.left.0 > 250), partial);
    println!("[tecla] suelta Up");
    println!("[tecla] suelta D");
    let rest = wait_for(&mut pad, |i| i.buttons == 0 && i.left.0 == 127);
    check("todo suelto", rest.is_some_and(|i| i.buttons == 0 && i.left == (127, 127)), rest);

    println!("== resultado: {total} comprobaciones, {failed} fallos");
    std::process::exit(failed as i32);
}

//! Demo gráfica: cubo de colores girando con sombreado Gouraud, satélites en
//! órbita, campo de estrellas y texto. Toda la geometría se calcula en `f32`
//! sobre la FPU del Emotion Engine y se dibuja con el Graphics Synthesizer.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::vec::Vec;
use ps2::gs::{Color, Gs, Vertex, VideoMode};
use ps2::math::{cos, sin};
use ps2::println;

ps2::entry!(main);

/// Caras del cubo: vértices en orden alrededor del perímetro. El vértice `i`
/// está en (±1, ±1, ±1) según los bits 0, 1 y 2 de `i`.
const FACES: [[usize; 4]; 6] = [
    [0, 1, 3, 2], // z-
    [4, 6, 7, 5], // z+
    [0, 2, 6, 4], // x-
    [1, 5, 7, 3], // x+
    [0, 4, 5, 1], // y-
    [2, 3, 7, 6], // y+
];

const CAMERA_DISTANCE: f32 = 4.2;
const FOCAL: f32 = 430.0;

#[derive(Clone, Copy)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

impl Vec3 {
    fn rotate(self, ax: f32, ay: f32, az: f32) -> Vec3 {
        let (sx, cx) = (sin(ax), cos(ax));
        let (sy, cy) = (sin(ay), cos(ay));
        let (sz, cz) = (sin(az), cos(az));
        // Rotación en Y, luego X, luego Z.
        let (x, z) = (self.x * cy + self.z * sy, -self.x * sy + self.z * cy);
        let (y, z) = (self.y * cx - z * sx, self.y * sx + z * cx);
        let (x, y) = (x * cz - y * sz, x * sz + y * cz);
        Vec3 { x, y, z }
    }

    /// Proyección en perspectiva a coordenadas de pantalla.
    fn project(self, cx: f32, cy: f32) -> (f32, f32) {
        let k = FOCAL / (self.z + CAMERA_DISTANCE);
        (cx + self.x * k, cy - self.y * k * 0.92)
    }
}

struct Star {
    x: i32,
    y: i32,
    phase: u32,
}

struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0 >> 8
    }
}

fn main() {
    let mut gs = Gs::init(VideoMode::Ntsc);
    let (w, h) = (gs.width(), gs.height());
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0 + 10.0);
    println!("cube: {}x{}, heap libre {} KiB", w, h, ps2::heap::free_bytes() / 1024);

    let mut rng = Lcg(2026);
    let stars: Vec<Star> = (0..90)
        .map(|_| Star { x: (rng.next() % w as u32) as i32, y: (rng.next() % h as u32) as i32, phase: rng.next() % 64 })
        .collect();

    let corners: Vec<Vec3> = (0..8)
        .map(|i| Vec3 {
            x: if i & 1 != 0 { 1.0 } else { -1.0 },
            y: if i & 2 != 0 { 1.0 } else { -1.0 },
            z: if i & 4 != 0 { 1.0 } else { -1.0 },
        })
        .collect();
    let corner_colors: Vec<Color> = (0..8)
        .map(|i| {
            let channel = |bit: usize| if i & bit != 0 { 250 } else { 35 };
            Color::rgb(channel(1), channel(2), channel(4))
        })
        .collect();

    let title = "RUST EN PLAYSTATION 2";
    let mut frame = 0u32;
    loop {
        let time = frame as f32 / 60.0;

        // Fondo: degradado vertical con dos triángulos Gouraud.
        let top = Color::rgb(6, 8, 30);
        let bottom = Color::rgb(60, 22, 78);
        gs.quad(
            Vertex::new(0.0, 0.0, top),
            Vertex::new(w as f32, 0.0, top),
            Vertex::new(w as f32, h as f32, bottom),
            Vertex::new(0.0, h as f32, bottom),
        );

        for star in &stars {
            let twinkle = (frame + star.phase) % 64;
            let level = (if twinkle < 32 { 90 + twinkle * 5 } else { 90 + (63 - twinkle) * 5 }) as u8;
            let size = if star.phase % 7 == 0 { 2 } else { 1 };
            gs.rect(star.x, star.y, size, size, Color::rgb(level, level, level));
        }

        // Geometría del cubo.
        let (ax, ay, az) = (time * 0.9, time * 1.3, time * 0.4);
        let rotated: Vec<Vec3> = corners.iter().map(|c| c.rotate(ax, ay, az)).collect();
        let screen: Vec<(f32, f32)> = rotated.iter().map(|v| v.project(cx, cy)).collect();

        // Satélites: los que están detrás del cubo se dibujan antes que él.
        let satellites: Vec<(Vec3, Color)> = (0..6)
            .map(|i| {
                let angle = time * 1.7 + i as f32 * (ps2::math::TAU / 6.0);
                let pos = Vec3 { x: cos(angle) * 2.5, y: sin(angle * 2.0) * 0.5, z: sin(angle) * 2.5 };
                let color = corner_colors[1 + i % 6];
                (pos, color)
            })
            .collect();
        let draw_satellite = |gs: &mut Gs, pos: Vec3, color: Color| {
            let (sx, sy) = pos.project(cx, cy);
            let half = 40.0 / (pos.z + CAMERA_DISTANCE);
            let dark = Color::rgb(color.r / 3, color.g / 3, color.b / 3);
            gs.quad(
                Vertex::new(sx, sy - half, color),
                Vertex::new(sx + half, sy, dark),
                Vertex::new(sx, sy + half, color),
                Vertex::new(sx - half, sy, Color::WHITE),
            );
        };
        for &(pos, color) in satellites.iter().filter(|(p, _)| p.z > 0.0) {
            draw_satellite(&mut gs, pos, color);
        }

        let mut visible_faces = 0;
        for face in &FACES {
            // La normal de una cara de un cubo centrado es su centro; la cara
            // mira a la cámara si normal · (centro - cámara) < 0.
            let nz = face.iter().map(|&i| rotated[i].z).sum::<f32>() / 4.0;
            if 1.0 + nz * CAMERA_DISTANCE >= 0.0 {
                continue;
            }
            visible_faces += 1;
            let v = |k: usize| {
                let i = face[k];
                Vertex::new(screen[i].0, screen[i].1, corner_colors[i])
            };
            gs.quad(v(0), v(1), v(2), v(3));
            for k in 0..4 {
                let (a, b) = (screen[face[k]], screen[face[(k + 1) % 4]]);
                gs.line(a.0, a.1, b.0, b.1, Color::rgb(255, 255, 255));
            }
        }

        for &(pos, color) in satellites.iter().filter(|(p, _)| p.z <= 0.0) {
            draw_satellite(&mut gs, pos, color);
        }

        // Paneles translúcidos (mezcla alfa) y texto.
        gs.set_blend(true);
        gs.rect(0, 22, w, 46, Color::rgba(0, 0, 0, 0x48));
        gs.rect(0, h - 50, w, 30, Color::rgba(0, 0, 0, 0x48));
        gs.set_blend(false);
        let title_x = (w - Gs::text_width(title, 4)) / 2;
        gs.text(title_x + 2, 32, 4, Color::rgb(90, 30, 10), title);
        gs.text(title_x, 30, 4, Color::rgb(247, 130, 60), title);
        let status = format!("frame {:5}   caras visibles: {}   FPU: f32 nativo", frame, visible_faces);
        gs.text((w - Gs::text_width(&status, 2)) / 2, h - 42, 2, Color::rgb(200, 210, 235), &status);

        gs.present();
        frame += 1;
        if frame == 1 {
            println!("cube: primer frame presentado");
        }
        if frame % 240 == 0 {
            println!("cube: {} frames, {} ms", frame, ps2::time::micros() / 1000);
        }
    }
}

//! Rasterizador de pruebas para el PC: el de referencia, pero con la
//! geometría de los triángulos hecha por el microprograma de la VU1,
//! interpretado ([`crate::vuinterp`]).
//!
//! Sirve para comprobar el microprograma sin consola: lo que éste manda al
//! GS se decodifica aquí y se rasteriza con [`SoftRasterizer`]. Se elige con
//! la variable de entorno `PS2_GLES_VU`.

use crate::raster::*;
use crate::soft::SoftRasterizer;
use crate::vu1prog::{self, Key, Target, CONSTANTS, C_COUNT, PALETTE};
use std::collections::BTreeMap;
use crate::vuinterp::Vu;

const ORIGIN: f32 = 1024.0;

/// Triángulos que han pasado por el microprograma en todo el proceso.
pub static FAST_TRIANGLES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Vértices que el núcleo de GL ha tenido que leer y entregar (los de la
/// geometría retenida sólo cuentan la primera vez).
pub static FILLED_VERTICES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub struct VuSoftRasterizer {
    soft: SoftRasterizer,
    state: State,
    vu: Vu,
    program: Option<(Key, Vec<u64>)>,
    /// Valores de K (nivel de detalle) de los últimos triángulos con textura.
    pub last_lod: Vec<i32>,
    /// Geometría retenida: (matrices por vértice, vértices).
    retained: BTreeMap<u32, (usize, Vec<f32>)>,
    next_retained: u32,
}

impl VuSoftRasterizer {
    pub fn new(width: u32, height: u32, depth_bits: u32) -> VuSoftRasterizer {
        VuSoftRasterizer { soft: SoftRasterizer::new(width, height, depth_bits), state: State::DEFAULT, vu: Vu::new(), program: None, last_lod: Vec::new(), retained: BTreeMap::new(), next_retained: 1 }
    }

    /// Instrucciones de VU ejecutadas hasta ahora.
    pub fn executed(&self) -> u64 {
        self.vu.executed
    }
}

impl Rasterizer for VuSoftRasterizer {
    fn size(&self) -> (u32, u32) {
        self.soft.size()
    }
    fn depth_bits(&self) -> u32 {
        self.soft.depth_bits()
    }
    fn set_state(&mut self, state: &State) {
        self.state = *state;
        self.soft.set_state(state);
    }
    fn point(&mut self, v: &Vertex) {
        self.soft.point(v)
    }
    fn line(&mut self, a: &Vertex, b: &Vertex) {
        self.soft.line(a, b)
    }
    fn triangle(&mut self, a: &Vertex, b: &Vertex, c: &Vertex) {
        self.soft.triangle(a, b, c)
    }
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, v: &Vertex) {
        self.soft.rect(x0, y0, x1, y1, v)
    }
    fn clear(&mut self, color: Option<[f32; 4]>, depth: Option<f32>, color_mask: [bool; 4], scissor: Option<[i32; 4]>) {
        self.soft.clear(color, depth, color_mask, scissor)
    }
    fn read_pixels(&mut self, x: u32, y: u32, w: u32, h: u32, out: &mut [u8]) {
        self.soft.read_pixels(x, y, w, h, out)
    }
    fn tex_image(&mut self, id: u32, level: u32, w: u32, h: u32, rgba: &[u8]) -> bool {
        self.soft.tex_image(id, level, w, h, rgba)
    }
    fn tex_sub_image(&mut self, id: u32, level: u32, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        self.soft.tex_sub_image(id, level, x, y, w, h, rgba)
    }
    fn tex_delete(&mut self, id: u32) {
        self.soft.tex_delete(id)
    }
    fn finish(&mut self) {
        self.soft.finish()
    }
    fn swap(&mut self) {
        self.soft.swap()
    }

    fn fast_triangles(&mut self, setup: &FastSetup, count: usize, fill: &mut dyn FnMut(&mut [f32])) -> bool {
        let mut vertices = vec![0.0f32; count * fast_vertex_floats(setup.skin)];
        fill(&mut vertices);
        FILLED_VERTICES.fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
        self.run(setup, &vertices);
        true
    }

    fn fast_retain(&mut self, units: usize, count: usize, fill: &mut dyn FnMut(&mut [f32])) -> Option<u32> {
        let mut vertices = vec![0.0f32; count * fast_vertex_floats(units)];
        fill(&mut vertices);
        FILLED_VERTICES.fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
        let id = self.next_retained;
        self.next_retained += 1;
        self.retained.insert(id, (units, vertices));
        Some(id)
    }

    fn fast_retained(&mut self, setup: &FastSetup, id: u32) -> bool {
        let Some((units, vertices)) = self.retained.remove(&id) else { return false };
        assert_eq!(units, setup.skin, "geometría retenida con otro formato de vértice");
        self.run(setup, &vertices);
        self.retained.insert(id, (units, vertices));
        true
    }

    fn fast_release(&mut self, id: u32) {
        self.retained.remove(&id);
    }
}

impl VuSoftRasterizer {
    /// Geometrías retenidas ahora mismo.
    pub fn retained_count(&self) -> usize {
        self.retained.len()
    }

    /// Pasa `vertices` (triángulos sueltos) por el microprograma.
    fn run(&mut self, setup: &FastSetup, vertices: &[f32]) {
        let (_, height) = self.soft.size();
        let depth_max = ((1u64 << self.soft.depth_bits()) - 1) as f32;
        let texture = self.state.texture;
        let color_scale = if texture.is_some() { 128.0 } else { 255.0 };
        // Como en el GS: el nivel de detalle hace falta con mipmaps o con
        // filtros distintos para reducir y ampliar.
        let lod = texture.filter(|t| t.levels > 1 || t.min != t.mag).map(|t| (0, (t.width * t.height) as f32));
        let target = Target { height: height as f32, origin: ORIGIN, depth_max, inverted: false, color_scale, prim: 0, lod };
        let key = Key::of(setup, &target);
        if self.program.as_ref().map(|p| p.0) != Some(key) {
            self.program = Some((key, vu1prog::build(&key)));
        }
        let code = &self.program.as_ref().unwrap().1;
        assert!(code.len() <= 2048, "el microprograma no cabe en la VU1");
        self.vu.mem[..CONSTANTS].copy_from_slice(&vu1prog::constants(setup, &target));
        if key.skin > 0 {
            let palette = vu1prog::palette(setup);
            self.vu.mem[PALETTE..PALETTE + palette.len()].copy_from_slice(&palette);
        }
        self.last_lod.clear();
        let words = key.vertex_words();
        for batch in vertices.chunks(key.batch() * 3 * words * 4) {
            for (i, word) in batch.chunks(4).enumerate() {
                self.vu.mem[key.input() + i] = [0, 1, 2, 3].map(|c| word[c].to_bits());
            }
            let triangles = batch.len() / (12 * words);
            self.vu.mem[C_COUNT] = vu1prog::batch_header(triangles);
            self.vu.run(code, 0);
            FAST_TRIANGLES.fetch_add(triangles as u64, std::sync::atomic::Ordering::Relaxed);
            // Lo enviado al GS, de vuelta a vértices para el rasterizador.
            for packet in core::mem::take(&mut self.vu.kicked) {
                let record = if lod.is_some() { 10 } else { 9 };
                assert_eq!(packet[0][1] >> 28, record as u32);
                for triangle in packet[1..].chunks(record) {
                    let mut out = [Vertex::default(); 3];
                    if lod.is_some() {
                        assert_eq!(triangle[0][2], 0x14, "falta el registro TEX1");
                        self.last_lod.push(((triangle[0][1] as i32) << 20) >> 20);
                    }
                    for (k, v) in triangle[record - 9..].chunks(3).enumerate() {
                        let (st, rgba, xyzf) = (v[0].map(f32::from_bits), v[1], v[2]);
                        let q = st[2];
                        out[k] = Vertex {
                            x: xyzf[0] as f32 / 16.0 - ORIGIN + 0.5,
                            y: xyzf[1] as f32 / 16.0 - ORIGIN + 0.5,
                            z: (xyzf[2] >> 4) as f32 / depth_max,
                            q,
                            s: st[0] / q,
                            t: st[1] / q,
                            color: [rgba[0] as f32 / color_scale, rgba[1] as f32 / color_scale, rgba[2] as f32 / color_scale, rgba[3] as f32 / 128.0],
                            fog: (xyzf[3] >> 4) as f32 / 255.0,
                        };
                    }
                    self.soft.triangle(&out[0], &out[1], &out[2]);
                }
            }
        }
    }
}

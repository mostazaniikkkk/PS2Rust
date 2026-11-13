//! Rasterizador sobre el Graphics Synthesizer de la PlayStation 2.
//!
//! El GS es de función fija y casi todo GLES 1.0 tiene registro propio. Lo
//! que no encaja se documenta aquí:
//!
//! * **Mezcla.** La fórmula del GS es `(A - B)·C + D`, con A, B, D entre el
//!   color de origen, el de destino o cero, y C entre el alfa de origen, el
//!   de destino o una constante. Se resuelven todas las parejas de
//!   `glBlendFunc` que caben (las de alfa, uno y cero); las que multiplican
//!   por un color (`GL_DST_COLOR`…), `GL_SRC_ALPHA_SATURATE` y las que piden
//!   sumar los dos colores con pesos distintos de alfa no existen: se dibuja
//!   sin mezcla. El alfa que queda en el framebuffer es el del fragmento.
//! * **Profundidad.** El GS sólo compara con "mayor" y "mayor o igual", así
//!   que la profundidad se guarda invertida (cerca = valor alto) y
//!   `GL_LESS`/`GL_LEQUAL` salen exactos. Con `GL_GREATER`/`GL_GEQUAL` se
//!   guarda sin invertir: funciona si no se mezclan los dos sentidos en un
//!   mismo frame. `GL_EQUAL` se trata como `GL_LEQUAL` y `GL_NOTEQUAL` como
//!   `GL_ALWAYS`.
//! * **Entorno de textura.** `GL_MODULATE` y `GL_REPLACE` son exactos, y
//!   `GL_DECAL` con texturas RGB. `GL_DECAL` con RGBA ignora el alfa de la
//!   textura; `GL_BLEND` y `GL_ADD` se dibujan como `GL_MODULATE`.
//! * **Mipmaps.** Hasta 7 niveles, con el nivel de detalle calculado una vez
//!   por triángulo.
//!
//! Escalas del GS: en el alfa, 0x80 es 1,0; al modular con una textura, el
//! color del vértice 0x80 es 1,0.

use crate::blend::solve_blend;
use crate::raster::*;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use ps2::gs::{Gs, VideoMode};

const REG_PRIM: u8 = 0x00;
const REG_RGBAQ: u8 = 0x01;
const REG_ST: u8 = 0x02;
const REG_XYZF2: u8 = 0x04;
const REG_TEX0_1: u8 = 0x06;
const REG_CLAMP_1: u8 = 0x08;
const REG_TEX1_1: u8 = 0x14;
const REG_MIPTBP1_1: u8 = 0x34;
const REG_MIPTBP2_1: u8 = 0x36;
const REG_FOGCOL: u8 = 0x3D;
const REG_TEXFLUSH: u8 = 0x3F;
const REG_SCISSOR_1: u8 = 0x40;
const REG_ALPHA_1: u8 = 0x42;
const REG_TEST_1: u8 = 0x47;
const REG_FRAME_1: u8 = 0x4C;
const REG_ZBUF_1: u8 = 0x4E;

const PRIM_POINT: u64 = 0;
const PRIM_LINE: u64 = 1;
const PRIM_TRIANGLE: u64 = 3;
const PRIM_SPRITE: u64 = 6;

/// Origen de la ventana en el espacio de coordenadas del GS (lo fija `Gs`).
const ORIGIN: f32 = 1024.0;
/// Páginas de 8 KiB de la memoria del GS (4 MiB).
const VRAM_PAGES: u32 = 512;

/// Memoria de vídeo de un nivel de textura.
struct Allocation {
    page: u32,
    pages: u32,
    buffer_width: u32,
}

pub struct GsRasterizer {
    gs: Gs,
    width: u32,
    height: u32,
    depth_bits: u32,
    /// Página donde empieza el búfer de profundidad.
    depth_page: u32,
    /// Huecos libres de memoria de vídeo: (página, páginas).
    free: Vec<(u32, u32)>,
    textures: BTreeMap<(u32, u32), Allocation>,
    state: State,
    /// ¿Están los registros del GS al día con `state`?
    applied: bool,
    /// Último tipo de primitiva escrito en PRIM.
    prim: Option<u64>,
    /// Bits de PRIM que dependen del estado (textura, niebla, mezcla).
    prim_flags: u64,
    /// Profundidad invertida: cerca = valor alto.
    inverted: bool,
    textured: bool,
    /// `GL_REPLACE` con textura sólo de alfa: el alfa del vértice no cuenta.
    opaque_vertex_alpha: bool,
    /// El filtro depende del nivel de detalle de cada triángulo.
    needs_lod: bool,
    /// Valor de TEX1 sin el nivel de detalle, y el último K escrito.
    tex1: u64,
    lod_k: i32,
    /// Rectángulo de recorte vacío: no se dibuja nada.
    nothing_visible: bool,
}

impl GsRasterizer {
    pub fn new(mode: VideoMode, depth_bits: u32) -> GsRasterizer {
        let gs = Gs::init(mode);
        let (width, height) = (gs.width() as u32, gs.height() as u32);
        let depth_page = gs.first_free_page();
        let bytes_per_pixel = if depth_bits <= 16 { 2 } else { 4 };
        let depth_pages = (width * height * bytes_per_pixel + 8191) / 8192;
        let first = depth_page + depth_pages;
        GsRasterizer {
            gs,
            width,
            height,
            depth_bits,
            depth_page,
            free: alloc::vec![(first, VRAM_PAGES - first)],
            textures: BTreeMap::new(),
            state: State::DEFAULT,
            applied: false,
            prim: None,
            prim_flags: 0,
            inverted: true,
            textured: false,
            opaque_vertex_alpha: false,
            needs_lod: false,
            tex1: 0,
            lod_k: 0,
            nothing_visible: false,
        }
    }

    fn depth_value(&self, z: f32) -> u64 {
        let max = ((1u32 << self.depth_bits) - 1) as f32;
        let z = if self.inverted { 1.0 - z } else { z };
        (z * max + 0.5) as u64
    }

    fn frame_register(&self, mask: [bool; 4]) -> u64 {
        let mut fbmsk = 0u64;
        for (i, write) in mask.iter().enumerate() {
            if !write {
                fbmsk |= 0xff << (8 * i);
            }
        }
        self.gs.draw_page() as u64 | ((self.width / 64) as u64) << 16 | fbmsk << 32
    }

    fn zbuf_register(&self, write: bool) -> u64 {
        // Z24 con 24 bits; Z16S (compatible con un framebuffer de 32 bits) con 16.
        let psm = if self.depth_bits <= 16 { 0xA } else { 1 };
        self.depth_page as u64 | psm << 24 | ((!write) as u64) << 32
    }

    fn scissor_register(&self, scissor: Option<[i32; 4]>) -> u64 {
        let [x0, y0, x1, y1] = scissor.unwrap_or([0, 0, self.width as i32, self.height as i32]);
        x0 as u64 | ((x1 - 1).max(0) as u64) << 16 | (y0 as u64) << 32 | ((y1 - 1).max(0) as u64) << 48
    }

    /// Escribe en el GS los registros que corresponden a `self.state`.
    fn apply(&mut self) {
        let s = self.state;
        self.nothing_visible = matches!(s.scissor, Some([x0, y0, x1, y1]) if x1 <= x0 || y1 <= y0);
        if let Some(func) = s.depth_test {
            self.inverted = !matches!(func, Compare::Greater | Compare::Gequal);
        }
        // Pruebas de alfa y profundidad.
        let mut test = 1 << 16; // la prueba de profundidad debe estar siempre encendida
        if let Some((func, reference)) = s.alpha_test {
            let atst = match func {
                Compare::Never => 0,
                Compare::Always => 1,
                Compare::Less => 2,
                Compare::Lequal => 3,
                Compare::Equal => 4,
                Compare::Gequal => 5,
                Compare::Greater => 6,
                Compare::Notequal => 7,
            };
            test |= 1 | atst << 1 | ((reference * 128.0 + 0.5) as u64) << 4;
        }
        let ztst = match s.depth_test {
            None | Some(Compare::Always) | Some(Compare::Notequal) => 1,
            Some(Compare::Never) => 0,
            Some(Compare::Less) | Some(Compare::Greater) => 3,
            Some(Compare::Lequal) | Some(Compare::Gequal) | Some(Compare::Equal) => 2,
        };
        self.gs.register(REG_TEST_1, test | ztst << 17);
        let zbuf = self.zbuf_register(s.depth_test.is_some() && s.depth_write);
        self.gs.register(REG_ZBUF_1, zbuf);
        let frame = self.frame_register(s.color_mask);
        self.gs.register(REG_FRAME_1, frame);
        let scissor = self.scissor_register(s.scissor);
        self.gs.register(REG_SCISSOR_1, scissor);

        self.prim_flags = 1 << 3; // Gouraud: el color va siempre por vértice
        if let Some(alpha) = s.blend.and_then(|(src, dst)| solve_blend(src, dst)) {
            self.gs.register(REG_ALPHA_1, alpha);
            self.prim_flags |= 1 << 6;
        }
        if let Some(color) = s.fog_color {
            let byte = |c: f32| (c * 255.0 + 0.5) as u64;
            self.gs.register(REG_FOGCOL, byte(color[0]) | byte(color[1]) << 8 | byte(color[2]) << 16);
            self.prim_flags |= 1 << 5;
        }

        self.textured = false;
        self.needs_lod = false;
        self.opaque_vertex_alpha = false;
        if let Some(t) = s.texture {
            if let Some(base) = self.textures.get(&(t.id, 0)) {
                let has_alpha = matches!(t.format, TexFormat::Alpha | TexFormat::LuminanceAlpha | TexFormat::Rgba);
                let has_color = t.format != TexFormat::Alpha;
                // (función del GS: 0 modular, 1 calcar; ¿usar el alfa de la textura?)
                let (tfx, tcc) = match t.env {
                    TexEnv::Replace if !has_color => {
                        self.opaque_vertex_alpha = true;
                        (0, 1)
                    }
                    TexEnv::Replace => (1, has_alpha as u64),
                    TexEnv::Decal if t.format == TexFormat::Rgba => (1, 0),
                    TexEnv::Decal if has_color => (1, has_alpha as u64),
                    _ => (0, has_alpha as u64),
                };
                let log2 = |v: u32| (31 - v.leading_zeros()) as u64;
                let tex0 = (base.page * 32) as u64 | ((base.buffer_width / 64) as u64) << 14 | log2(t.width) << 26 | log2(t.height) << 30 | tcc << 34 | tfx << 35;
                self.gs.register(REG_TEX0_1, tex0);
                // Niveles de mipmap presentes en memoria de vídeo.
                let mut levels = 1;
                let mut mip = [0u64; 2];
                while levels < t.levels.min(7) {
                    let Some(a) = self.textures.get(&(t.id, levels)) else { break };
                    let slot = ((levels - 1) % 3) as u64 * 20;
                    mip[((levels - 1) / 3) as usize] |= ((a.page * 32) as u64 | ((a.buffer_width / 64) as u64) << 14) << slot;
                    levels += 1;
                }
                self.gs.register(REG_MIPTBP1_1, mip[0]);
                self.gs.register(REG_MIPTBP2_1, mip[1]);
                let filter = |f: Filter| match f {
                    Filter::Nearest => 0u64,
                    Filter::Linear => 1,
                    Filter::NearestMipNearest => 2,
                    Filter::NearestMipLinear => 3,
                    Filter::LinearMipNearest => 4,
                    Filter::LinearMipLinear => 5,
                };
                let min = if levels > 1 {
                    filter(t.min)
                } else {
                    // Sin pirámide, sólo queda el filtro base.
                    matches!(t.min, Filter::Linear | Filter::LinearMipNearest | Filter::LinearMipLinear) as u64
                };
                let mag = (t.mag == Filter::Linear) as u64;
                // LCM = 1: el nivel de detalle es el valor K, que se fija por triángulo.
                self.tex1 = 1 | (levels as u64 - 1) << 2 | mag << 5 | min << 6;
                self.needs_lod = levels > 1 || min != mag;
                self.lod_k = -1;
                self.gs.register(REG_TEX1_1, self.tex1 | (self.lod_k as u64 & 0xfff) << 32);
                let wrap = |w: Wrap| (w == Wrap::ClampToEdge) as u64;
                self.gs.register(REG_CLAMP_1, wrap(t.wrap_s) | wrap(t.wrap_t) << 2);
                self.gs.register(REG_TEXFLUSH, 0);
                self.prim_flags |= 1 << 4;
                self.textured = true;
            }
        }
        self.prim = None;
        self.applied = true;
    }

    /// Prepara el GS para una primitiva del tipo `kind`. Devuelve `false` si
    /// no hay nada que dibujar.
    fn begin(&mut self, kind: u64) -> bool {
        if !self.applied {
            self.apply();
        }
        if self.nothing_visible {
            return false;
        }
        // Los triángulos seguidos comparten un PRIM. Puntos, líneas y
        // rectángulos lo repiten: PCSX2 pierde los que llegan sin él.
        if self.prim != Some(kind) || kind != PRIM_TRIANGLE {
            self.gs.register(REG_PRIM, kind | self.prim_flags);
            self.prim = Some(kind);
        }
        true
    }

    /// Fija el nivel de detalle (K de TEX1) para la primitiva siguiente.
    fn set_lod(&mut self, lod: f32) {
        if !self.needs_lod {
            return;
        }
        // K en coma fija con 4 bits de fracción. Con K < 0 el GS amplía.
        let k = if lod <= 0.0 { -1 } else { ((lod * 16.0) as i32).clamp(1, 7 * 16) };
        if k != self.lod_k {
            self.lod_k = k;
            self.gs.register(REG_TEX1_1, self.tex1 | (k as u64 & 0xfff) << 32);
        }
    }

    fn position(&self, x: f32, y: f32) -> u64 {
        // Coma fija 12.4; los centros de píxel del GS están en los enteros.
        let fixed = |v: f32| (((v - 0.5 + ORIGIN) * 16.0 + 0.5) as i32).clamp(0, 0xffff) as u64;
        fixed(x) | fixed(y) << 16
    }

    fn vertex(&mut self, v: &Vertex, x: f32, y: f32) {
        if self.textured {
            self.gs.register(REG_ST, (v.s * v.q).to_bits() as u64 | ((v.t * v.q).to_bits() as u64) << 32);
        }
        let scale = if self.textured { 128.0 } else { 255.0 };
        let channel = |c: f32| (c * scale + 0.5) as u64;
        let alpha = if self.opaque_vertex_alpha { 128 } else { (v.color[3] * 128.0 + 0.5) as u64 };
        let rgba = channel(v.color[0]) | channel(v.color[1]) << 8 | channel(v.color[2]) << 16 | alpha << 24;
        self.gs.register(REG_RGBAQ, rgba | (v.q.to_bits() as u64) << 32);
        let fog = (v.fog * 255.0 + 0.5) as u64;
        let xyzf = self.position(x, y) | self.depth_value(v.z) << 32 | fog << 56;
        self.gs.register(REG_XYZF2, xyzf);
    }

    /// Texels con el alfa en la escala del GS (0x80 = opaco).
    fn gs_texels(rgba: &[u8]) -> Vec<u8> {
        let mut out = rgba.to_vec();
        for p in out.chunks_exact_mut(4) {
            p[3] = ((p[3] as u32 * 128 + 127) / 255) as u8;
        }
        out
    }

    fn release(&mut self, key: (u32, u32)) {
        if let Some(a) = self.textures.remove(&key) {
            self.free.push((a.page, a.pages));
            // Une los huecos contiguos.
            self.free.sort_unstable();
            let mut merged: Vec<(u32, u32)> = Vec::new();
            for &(page, pages) in &self.free {
                match merged.last_mut() {
                    Some(last) if last.0 + last.1 == page => last.1 += pages,
                    _ => merged.push((page, pages)),
                }
            }
            self.free = merged;
        }
    }
}

impl Rasterizer for GsRasterizer {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn depth_bits(&self) -> u32 {
        self.depth_bits
    }

    fn set_state(&mut self, state: &State) {
        if *state != self.state || !self.applied {
            self.state = *state;
            self.applied = false;
        }
    }

    fn depth_func_hint(&mut self, func: Compare) {
        self.inverted = !matches!(func, Compare::Greater | Compare::Gequal);
    }

    fn point(&mut self, v: &Vertex) {
        if self.begin(PRIM_POINT) {
            self.set_lod(0.0);
            // El píxel que contiene el punto.
            let floor = |c: f32| (c as i32) as f32 + 0.5;
            self.vertex(v, floor(v.x), floor(v.y));
        }
    }

    fn line(&mut self, a: &Vertex, b: &Vertex) {
        if self.begin(PRIM_LINE) {
            self.set_lod(0.0);
            self.vertex(a, a.x, a.y);
            self.vertex(b, b.x, b.y);
        }
    }

    fn triangle(&mut self, a: &Vertex, b: &Vertex, c: &Vertex) {
        if !self.begin(PRIM_TRIANGLE) {
            return;
        }
        if self.needs_lod {
            let t = self.state.texture.unwrap();
            let cross = |ax: f32, ay: f32, bx: f32, by: f32| {
                let v = ax * by - ay * bx;
                if v < 0.0 {
                    -v
                } else {
                    v
                }
            };
            let pixels = cross(b.x - a.x, b.y - a.y, c.x - a.x, c.y - a.y);
            let texels = cross(b.s - a.s, b.t - a.t, c.s - a.s, c.t - a.t) * t.width as f32 * t.height as f32;
            let lod = if pixels > 0.0 && texels > 0.0 { 0.5 * crate::math::log2(texels / pixels) } else { -1.0 };
            self.set_lod(lod);
        }
        self.vertex(a, a.x, a.y);
        self.vertex(b, b.x, b.y);
        self.vertex(c, c.x, c.y);
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, v: &Vertex) {
        if self.begin(PRIM_SPRITE) {
            self.set_lod(0.0);
            self.vertex(v, x0, y0);
            self.vertex(v, x1, y1);
        }
    }

    fn clear(&mut self, color: Option<[f32; 4]>, depth: Option<f32>, color_mask: [bool; 4], scissor: Option<[i32; 4]>) {
        if matches!(scissor, Some([x0, y0, x1, y1]) if x1 <= x0 || y1 <= y0) {
            return;
        }
        // Un sprite que cubre la zona, con las pruebas apagadas.
        self.gs.register(REG_TEST_1, 1 << 16 | 1 << 17);
        let zbuf = self.zbuf_register(depth.is_some());
        self.gs.register(REG_ZBUF_1, zbuf);
        let frame = self.frame_register(if color.is_some() { color_mask } else { [false; 4] });
        self.gs.register(REG_FRAME_1, frame);
        let scissor = self.scissor_register(scissor);
        self.gs.register(REG_SCISSOR_1, scissor);
        self.gs.register(REG_PRIM, PRIM_SPRITE);
        let c = color.unwrap_or([0.0; 4]);
        let byte = |v: f32| (v * 255.0 + 0.5) as u64;
        self.gs.register(REG_RGBAQ, byte(c[0]) | byte(c[1]) << 8 | byte(c[2]) << 16 | ((c[3] * 128.0 + 0.5) as u64) << 24);
        let z = self.depth_value(depth.unwrap_or(1.0));
        let corner = |x: u32, y: u32| ((x as u64 + 1024) << 4) | ((y as u64 + 1024) << 4) << 16;
        self.gs.register(REG_XYZF2, corner(0, 0) | z << 32);
        self.gs.register(REG_XYZF2, corner(self.width, self.height) | z << 32);
        self.applied = false;
        self.prim = None;
    }

    fn read_pixels(&mut self, x: u32, y: u32, w: u32, h: u32, out: &mut [u8]) {
        let block = self.gs.draw_page() * 32;
        self.gs.download(block, self.width, x, y, w, h, out);
        // El alfa del GS va de 0 a 0x80.
        for p in out.chunks_exact_mut(4) {
            p[3] = ((p[3] as u32 * 255 + 64) / 128).min(255) as u8;
        }
    }

    fn tex_image(&mut self, id: u32, level: u32, w: u32, h: u32, rgba: &[u8]) -> bool {
        self.release((id, level));
        // Las texturas de 32 bits ocupan páginas enteras de 64×32 píxeles.
        let buffer_width = w.max(64);
        let pages = (buffer_width / 64) * ((h + 31) / 32);
        let Some(slot) = self.free.iter().position(|&(_, size)| size >= pages) else { return false };
        let (page, size) = self.free[slot];
        if size == pages {
            self.free.remove(slot);
        } else {
            self.free[slot] = (page + pages, size - pages);
        }
        self.gs.upload(page * 32, buffer_width, 0, 0, w, h, &Self::gs_texels(rgba));
        self.textures.insert((id, level), Allocation { page, pages, buffer_width });
        self.applied = false;
        true
    }

    fn tex_sub_image(&mut self, id: u32, level: u32, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        if let Some(a) = self.textures.get(&(id, level)) {
            let (block, buffer_width) = (a.page * 32, a.buffer_width);
            self.gs.upload(block, buffer_width, x, y, w, h, &Self::gs_texels(rgba));
            self.applied = false;
        }
    }

    fn tex_delete(&mut self, id: u32) {
        let keys: Vec<(u32, u32)> = self.textures.range((id, 0)..=(id, u32::MAX)).map(|(k, _)| *k).collect();
        for key in keys {
            self.release(key);
        }
        self.applied = false;
    }

    fn finish(&mut self) {
        self.gs.flush();
    }

    fn swap(&mut self) {
        self.gs.present();
        // `present` cambia de framebuffer y toca registros: se reescriben.
        self.applied = false;
        self.prim = None;
    }
}

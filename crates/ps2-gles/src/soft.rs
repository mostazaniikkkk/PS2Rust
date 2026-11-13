//! Rasterizador por software de referencia. Sigue la especificación de
//! OpenGL ES 1.0 al pie de la letra y sólo se compila en el PC: las pruebas
//! de píxeles comparan contra él lo que dibuja el Graphics Synthesizer.

use crate::raster::*;
use std::collections::BTreeMap;

/// Nivel de mipmap de una textura, en RGBA de 8 bits (fila de `t = 0` primero).
struct Level {
    w: u32,
    h: u32,
    data: Vec<u8>,
}

/// Atributos de un fragmento ya interpolados.
#[derive(Clone, Copy)]
struct Frag {
    z: f32,
    color: [f32; 4],
    fog: f32,
    s: f32,
    t: f32,
}

/// Rasterizador por software con un único búfer de color RGBA8 y un búfer de
/// profundidad entero de `depth_bits` bits.
pub struct SoftRasterizer {
    width: u32,
    height: u32,
    depth_bits: u32,
    /// Valor máximo del búfer de profundidad (2^bits - 1).
    depth_max: u32,
    color: Vec<u8>,
    depth: Vec<u32>,
    state: State,
    textures: BTreeMap<u32, Vec<Level>>,
}

/// Color de 0 a 1 convertido a 8 bits con redondeo.
fn to_u8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Función de comparación de las pruebas alfa y de profundidad:
/// `a` es el valor entrante y `b` el de referencia o el almacenado.
fn compare<T: PartialOrd>(f: Compare, a: T, b: T) -> bool {
    match f {
        Compare::Never => false,
        Compare::Less => a < b,
        Compare::Equal => a == b,
        Compare::Lequal => a <= b,
        Compare::Greater => a > b,
        Compare::Notequal => a != b,
        Compare::Gequal => a >= b,
        Compare::Always => true,
    }
}

/// Factor de mezcla por canal a partir del color de origen y el de destino.
fn factor(f: Factor, s: [f32; 4], d: [f32; 4]) -> [f32; 4] {
    let inv = |c: [f32; 4]| c.map(|v| 1.0 - v);
    match f {
        Factor::Zero => [0.0; 4],
        Factor::One => [1.0; 4],
        Factor::SrcColor => s,
        Factor::OneMinusSrcColor => inv(s),
        Factor::DstColor => d,
        Factor::OneMinusDstColor => inv(d),
        Factor::SrcAlpha => [s[3]; 4],
        Factor::OneMinusSrcAlpha => [1.0 - s[3]; 4],
        Factor::DstAlpha => [d[3]; 4],
        Factor::OneMinusDstAlpha => [1.0 - d[3]; 4],
        Factor::SrcAlphaSaturate => {
            let m = s[3].min(1.0 - d[3]);
            [m, m, m, 1.0]
        }
    }
}

/// Índice de texel envuelto según el modo de repetición.
fn wrap(mode: Wrap, i: i64, n: u32) -> u32 {
    match mode {
        Wrap::Repeat => i.rem_euclid(n as i64) as u32,
        Wrap::ClampToEdge => i.clamp(0, n as i64 - 1) as u32,
    }
}

/// Muestra un nivel con filtro más cercano o bilineal (centros de texel en `.5`).
fn sample_level(l: &Level, linear: bool, ws: Wrap, wt: Wrap, s: f32, t: f32) -> [f32; 4] {
    let fetch = |i: i64, j: i64| -> [f32; 4] {
        let o = ((wrap(wt, j, l.h) * l.w + wrap(ws, i, l.w)) * 4) as usize;
        [0, 1, 2, 3].map(|k| l.data[o + k] as f32 / 255.0)
    };
    let (u, v) = (s * l.w as f32, t * l.h as f32);
    if !linear {
        return fetch(u.floor() as i64, v.floor() as i64);
    }
    let (u, v) = (u - 0.5, v - 0.5);
    let (i, j) = (u.floor(), v.floor());
    let (fu, fv) = (u - i, v - j);
    let (i, j) = (i as i64, j as i64);
    let (c00, c10, c01, c11) = (fetch(i, j), fetch(i + 1, j), fetch(i, j + 1), fetch(i + 1, j + 1));
    [0, 1, 2, 3].map(|k| {
        (c00[k] * (1.0 - fu) + c10[k] * fu) * (1.0 - fv) + (c01[k] * (1.0 - fu) + c11[k] * fu) * fv
    })
}

/// Interpola los atributos de varios vértices con los pesos `w` (que suman 1):
/// `z` lineal en pantalla y el resto con corrección de perspectiva.
fn interpolate(vs: &[&Vertex], w: &[f32]) -> Frag {
    let mut f = Frag { z: 0.0, color: [0.0; 4], fog: 0.0, s: 0.0, t: 0.0 };
    let q: f32 = vs.iter().zip(w).map(|(v, w)| v.q * w).sum();
    for (v, &w) in vs.iter().zip(w) {
        f.z += v.z * w;
        // Peso con perspectiva; si q se anula se interpola linealmente.
        let p = if q != 0.0 { w * v.q / q } else { w };
        for k in 0..4 {
            f.color[k] += v.color[k] * p;
        }
        f.fog += v.fog * p;
        f.s += v.s * p;
        f.t += v.t * p;
    }
    f
}

impl SoftRasterizer {
    /// Crea una ventana de `width`×`height` con el color a cero y la
    /// profundidad al máximo (lejos).
    pub fn new(width: u32, height: u32, depth_bits: u32) -> SoftRasterizer {
        let depth_max = if depth_bits >= 32 { u32::MAX } else { (1u32 << depth_bits) - 1 };
        let n = (width * height) as usize;
        SoftRasterizer {
            width,
            height,
            depth_bits,
            depth_max,
            color: vec![0; n * 4],
            depth: vec![depth_max; n],
            state: State::DEFAULT,
            textures: BTreeMap::new(),
        }
    }

    /// Profundidad de 0 a 1 cuantizada a los bits del búfer.
    fn quantize_depth(&self, z: f32) -> u32 {
        (z.clamp(0.0, 1.0) as f64 * self.depth_max as f64).round() as u32
    }

    /// Rectángulo de píxeles `[x0, y0, x1, y1)` que deja pasar el recorte.
    fn bounds(&self, scissor: Option<[i32; 4]>) -> [i32; 4] {
        let (w, h) = (self.width as i32, self.height as i32);
        match scissor {
            Some(s) => [s[0].max(0), s[1].max(0), s[2].min(w), s[3].min(h)],
            None => [0, 0, w, h],
        }
    }

    /// Texel filtrado de la textura activa, o `None` si no tiene niveles
    /// (textura incompleta: se dibuja como si no hubiera textura).
    fn sample(&self, tex: &TextureState, s: f32, t: f32, lod: f32) -> Option<[f32; 4]> {
        let levels = self.textures.get(&tex.id)?;
        let n = levels.iter().take_while(|l| l.w > 0 && l.h > 0).count().min(tex.levels.max(1) as usize);
        if n == 0 {
            return None;
        }
        let at = |i: usize, f: Filter| {
            let linear = matches!(f, Filter::Linear | Filter::LinearMipNearest | Filter::LinearMipLinear);
            sample_level(&levels[i.min(n - 1)], linear, tex.wrap_s, tex.wrap_t, s, t)
        };
        // Ampliación (lod <= 0, o no calculable): filtro `mag` sobre el nivel 0.
        if !(lod > 0.0) {
            return Some(at(0, tex.mag));
        }
        Some(match tex.min {
            Filter::Nearest | Filter::Linear => at(0, tex.min),
            Filter::NearestMipNearest | Filter::LinearMipNearest => at(((lod + 0.5).ceil() - 1.0).max(0.0) as usize, tex.min),
            Filter::NearestMipLinear | Filter::LinearMipLinear => {
                let base = lod.floor();
                let (f, i) = (lod - base, base as usize);
                // En el último nivel los dos niveles coinciden.
                let f = if i >= n - 1 { 0.0 } else { f };
                let (c0, c1) = (at(i, tex.min), at(i.saturating_add(1), tex.min));
                [0, 1, 2, 3].map(|k| c0[k] * (1.0 - f) + c1[k] * f)
            }
        })
    }

    /// Pasa un fragmento por el pipeline de GL: recorte, textura, niebla,
    /// prueba alfa, prueba de profundidad, mezcla, máscara y escritura.
    fn fragment(&mut self, x: i32, y: i32, f: Frag, lod: f32) {
        let st = self.state;
        let b = self.bounds(st.scissor);
        if x < b[0] || y < b[1] || x >= b[2] || y >= b[3] {
            return;
        }
        let mut c = f.color.map(|v| v.clamp(0.0, 1.0));

        // Entorno de textura (tabla de GL ES 1.0).
        if let Some(tex) = st.texture {
            if let Some(tx) = self.sample(&tex, f.s, f.t, lod) {
                let has_color = tex.format != TexFormat::Alpha;
                let has_alpha = matches!(tex.format, TexFormat::Alpha | TexFormat::LuminanceAlpha | TexFormat::Rgba);
                let decal_rgba = tex.env == TexEnv::Decal && tex.format == TexFormat::Rgba;
                match tex.env {
                    TexEnv::Decal if decal_rgba => {
                        for k in 0..3 {
                            c[k] = c[k] * (1.0 - tx[3]) + tx[k] * tx[3];
                        }
                    }
                    // Decal con otros formatos se trata como Replace.
                    TexEnv::Replace | TexEnv::Decal => {
                        if has_color {
                            c[..3].copy_from_slice(&tx[..3]);
                        }
                        if has_alpha {
                            c[3] = tx[3];
                        }
                    }
                    TexEnv::Modulate | TexEnv::Blend | TexEnv::Add => {
                        if has_color {
                            for k in 0..3 {
                                c[k] = match tex.env {
                                    TexEnv::Modulate => c[k] * tx[k],
                                    TexEnv::Add => (c[k] + tx[k]).min(1.0),
                                    _ => c[k] * (1.0 - tx[k]) + tex.env_color[k].clamp(0.0, 1.0) * tx[k],
                                };
                            }
                        }
                        if has_alpha {
                            c[3] *= tx[3];
                        }
                    }
                }
            }
        }

        // Niebla, sólo sobre RGB.
        if let Some(fc) = st.fog_color {
            let fog = f.fog.clamp(0.0, 1.0);
            for k in 0..3 {
                c[k] = fog * c[k] + (1.0 - fog) * fc[k].clamp(0.0, 1.0);
            }
        }

        // Prueba alfa.
        if let Some((func, reference)) = st.alpha_test {
            if !compare(func, c[3], reference.clamp(0.0, 1.0)) {
                return;
            }
        }

        // Prueba de profundidad: entrante contra almacenado.
        let i = (y as u32 * self.width + x as u32) as usize;
        if let Some(func) = st.depth_test {
            let z = self.quantize_depth(f.z);
            if !compare(func, z, self.depth[i]) {
                return;
            }
            if st.depth_write {
                self.depth[i] = z;
            }
        }

        // Mezcla con lo que ya hay en el búfer de color.
        if let Some((sf, df)) = st.blend {
            let d = [0, 1, 2, 3].map(|k| self.color[i * 4 + k] as f32 / 255.0);
            let (fs, fd) = (factor(sf, c, d), factor(df, c, d));
            c = [0, 1, 2, 3].map(|k| (c[k] * fs[k] + d[k] * fd[k]).clamp(0.0, 1.0));
        }

        for k in 0..4 {
            if st.color_mask[k] {
                self.color[i * 4 + k] = to_u8(c[k]);
            }
        }
    }
}

impl Rasterizer for SoftRasterizer {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn depth_bits(&self) -> u32 {
        self.depth_bits
    }

    fn set_state(&mut self, state: &State) {
        self.state = *state;
    }

    fn point(&mut self, v: &Vertex) {
        if v.x.is_finite() && v.y.is_finite() {
            self.fragment(v.x.floor() as i32, v.y.floor() as i32, interpolate(&[v], &[1.0]), 0.0);
        }
    }

    fn line(&mut self, a: &Vertex, b: &Vertex) {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        if !(dx.is_finite() && dy.is_finite()) {
            return;
        }
        // Se recorre el eje mayor por centros de píxel, con el extremo inicial
        // incluido y el final excluido (aproxima la regla del rombo).
        let x_major = dx.abs() >= dy.abs();
        let (p0, p1, m0, m1, limit) = if x_major {
            (a.x, b.x, a.y, b.y, self.width as i32)
        } else {
            (a.y, b.y, a.x, b.x, self.height as i32)
        };
        let (first, last) = if p0 <= p1 {
            ((p0 - 0.5).ceil() as i32, (p1 - 0.5).ceil() as i32 - 1)
        } else {
            ((p1 - 0.5).floor() as i32 + 1, (p0 - 0.5).floor() as i32)
        };
        for i in first.max(0)..=last.min(limit - 1) {
            let w = (i as f32 + 0.5 - p0) / (p1 - p0);
            let m = (m0 + w * (m1 - m0)).floor() as i32;
            let (x, y) = if x_major { (i, m) } else { (m, i) };
            self.fragment(x, y, interpolate(&[a, b], &[1.0 - w, w]), 0.0);
        }
    }

    fn triangle(&mut self, a: &Vertex, b: &Vertex, c: &Vertex) {
        // Función de arista en f64: positiva a la derecha de p→q (y hacia abajo).
        let edge = |p: &Vertex, q: &Vertex, x: f64, y: f64| {
            (q.x as f64 - p.x as f64) * (y - p.y as f64) - (q.y as f64 - p.y as f64) * (x - p.x as f64)
        };
        // Se ordena para que el área sea positiva (cualquier sentido vale).
        let mut vs = [a, b, c];
        if edge(a, b, c.x as f64, c.y as f64) < 0.0 {
            vs.swap(1, 2);
        }
        let area = edge(vs[0], vs[1], vs[2].x as f64, vs[2].y as f64);
        if !(area > 0.0 && area.is_finite()) {
            return;
        }
        // Arista opuesta a cada vértice y si es superior o izquierda.
        let edges = [(vs[1], vs[2]), (vs[2], vs[0]), (vs[0], vs[1])];
        let top_left = edges.map(|(p, q)| q.y < p.y || (q.y == p.y && q.x > p.x));

        // Nivel de detalle, uno por triángulo: áreas en texels y en píxeles.
        let lod = match self.state.texture {
            Some(t) => {
                let (s0, t0) = (vs[0].s as f64, vs[0].t as f64);
                let texels = ((vs[1].s as f64 - s0) * (vs[2].t as f64 - t0) - (vs[1].t as f64 - t0) * (vs[2].s as f64 - s0)).abs()
                    * t.width as f64
                    * t.height as f64;
                (0.5 * (texels / area).log2()) as f32
            }
            None => 0.0,
        };

        let lo = |f: fn(f32, f32) -> f32, g: fn(&Vertex) -> f32| f(f(g(a), g(b)), g(c));
        let x0 = ((lo(f32::min, |v| v.x) - 0.5).ceil() as i32).max(0);
        let x1 = ((lo(f32::max, |v| v.x) - 0.5).floor() as i32).min(self.width as i32 - 1);
        let y0 = ((lo(f32::min, |v| v.y) - 0.5).ceil() as i32).max(0);
        let y1 = ((lo(f32::max, |v| v.y) - 0.5).floor() as i32).min(self.height as i32 - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let e = [0, 1, 2].map(|k| edge(edges[k].0, edges[k].1, px, py));
                // Regla de relleno: dentro, o sobre una arista superior o izquierda.
                if (0..3).all(|k| e[k] > 0.0 || (e[k] == 0.0 && top_left[k])) {
                    let w = e.map(|v| (v / area) as f32);
                    self.fragment(x, y, interpolate(&vs, &w), lod);
                }
            }
        }
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, v: &Vertex) {
        // Píxeles cuyo centro cae en [x0, x1) × [y0, y1).
        let range = |p0: f32, p1: f32, limit: u32| ((p0 - 0.5).ceil() as i32).max(0)..((p1 - 0.5).ceil() as i32).min(limit as i32);
        let f = interpolate(&[v], &[1.0]);
        for y in range(y0, y1, self.height) {
            for x in range(x0, x1, self.width) {
                self.fragment(x, y, f, 0.0);
            }
        }
    }

    fn clear(&mut self, color: Option<[f32; 4]>, depth: Option<f32>, color_mask: [bool; 4], scissor: Option<[i32; 4]>) {
        let b = self.bounds(scissor);
        let rgba = color.map(|c| c.map(to_u8));
        let z = depth.map(|z| self.quantize_depth(z));
        for y in b[1]..b[3] {
            for x in b[0]..b[2] {
                let i = (y as u32 * self.width + x as u32) as usize;
                if let Some(rgba) = rgba {
                    for k in 0..4 {
                        if color_mask[k] {
                            self.color[i * 4 + k] = rgba[k];
                        }
                    }
                }
                if let Some(z) = z {
                    self.depth[i] = z;
                }
            }
        }
    }

    fn read_pixels(&mut self, x: u32, y: u32, w: u32, h: u32, out: &mut [u8]) {
        let n = w as usize * 4;
        for row in 0..h as usize {
            let src = ((y as usize + row) * self.width as usize + x as usize) * 4;
            out[row * n..(row + 1) * n].copy_from_slice(&self.color[src..src + n]);
        }
    }

    fn tex_image(&mut self, id: u32, level: u32, w: u32, h: u32, rgba: &[u8]) -> bool {
        let levels = self.textures.entry(id).or_default();
        // Los niveles intermedios sin definir quedan vacíos.
        while levels.len() <= level as usize {
            levels.push(Level { w: 0, h: 0, data: Vec::new() });
        }
        let n = (w * h * 4) as usize;
        levels[level as usize] = Level { w, h, data: rgba[..n].to_vec() };
        true
    }

    fn tex_sub_image(&mut self, id: u32, level: u32, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        let Some(l) = self.textures.get_mut(&id).and_then(|t| t.get_mut(level as usize)) else {
            return;
        };
        if x + w > l.w || y + h > l.h {
            return;
        }
        let n = w as usize * 4;
        for row in 0..h as usize {
            let dst = ((y as usize + row) * l.w as usize + x as usize) * 4;
            l.data[dst..dst + n].copy_from_slice(&rgba[row * n..(row + 1) * n]);
        }
    }

    fn tex_delete(&mut self, id: u32) {
        self.textures.remove(&id);
    }

    fn finish(&mut self) {}

    fn swap(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const WHITE: [f32; 4] = [1.0; 4];

    fn vtx(x: f32, y: f32, color: [f32; 4]) -> Vertex {
        Vertex { x, y, z: 0.5, q: 1.0, s: 0.0, t: 0.0, color, fog: 1.0 }
    }

    fn px(r: &mut SoftRasterizer, x: u32, y: u32) -> [u8; 4] {
        let mut out = [0u8; 4];
        r.read_pixels(x, y, 1, 1, &mut out);
        out
    }

    /// Cuadrilátero `[x0, x1) × [y0, y1)` en dos triángulos, con las
    /// coordenadas de textura de 0 a `st` y la profundidad `z`.
    fn quad(r: &mut SoftRasterizer, x0: f32, y0: f32, x1: f32, y1: f32, z: f32, st: f32, color: [f32; 4]) {
        let v = |x, y, s, t| Vertex { z, s, t, ..vtx(x, y, color) };
        let (a, b, c, d) = (v(x0, y0, 0.0, 0.0), v(x1, y0, st, 0.0), v(x1, y1, st, st), v(x0, y1, 0.0, st));
        r.triangle(&a, &b, &c);
        r.triangle(&a, &c, &d);
    }

    fn texture(id: u32, w: u32, h: u32) -> TextureState {
        TextureState {
            id,
            format: TexFormat::Rgba,
            env: TexEnv::Replace,
            env_color: [0.0; 4],
            min: Filter::Nearest,
            mag: Filter::Nearest,
            wrap_s: Wrap::Repeat,
            wrap_t: Wrap::Repeat,
            levels: 1,
            width: w,
            height: h,
        }
    }

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        c.iter().copied().cycle().take((w * h * 4) as usize).collect()
    }

    #[test]
    fn triangle_covers_interior_only() {
        let mut r = SoftRasterizer::new(8, 8, 16);
        // En los dos sentidos de giro.
        r.triangle(&vtx(1.0, 1.0, RED), &vtx(7.0, 1.0, RED), &vtx(1.0, 7.0, RED));
        assert_eq!(px(&mut r, 2, 2), [255, 0, 0, 255]);
        assert_eq!(px(&mut r, 6, 6), [0; 4]);
        assert_eq!(px(&mut r, 0, 0), [0; 4]);
        r.triangle(&vtx(7.0, 7.0, GREEN), &vtx(7.0, 1.0, GREEN), &vtx(1.0, 7.0, GREEN));
        assert_eq!(px(&mut r, 6, 6), [0, 255, 0, 255]);
    }

    #[test]
    fn shared_edge_drawn_once() {
        let gray = [0.25; 4];
        let additive = State { blend: Some((Factor::One, Factor::One)), ..State::DEFAULT };
        // Cuadrado alineado: todos los píxeles exactamente una vez.
        let mut r = SoftRasterizer::new(8, 8, 16);
        r.set_state(&additive);
        quad(&mut r, 0.0, 0.0, 8.0, 8.0, 0.5, 1.0, gray);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(px(&mut r, x, y), [64; 4], "pixel {x},{y}");
            }
        }
        // Abanico inclinado alrededor de un centro: sin dobles ni huecos.
        let mut r = SoftRasterizer::new(16, 16, 16);
        r.set_state(&additive);
        let rim = [(1.5, 3.0), (9.0, 0.5), (15.0, 6.5), (12.5, 14.0), (4.0, 15.5)];
        let center = vtx(8.5, 8.5, gray);
        for i in 0..5 {
            let (p, q) = (rim[i], rim[(i + 1) % 5]);
            r.triangle(&center, &vtx(p.0, p.1, gray), &vtx(q.0, q.1, gray));
        }
        for y in 0..16 {
            for x in 0..16 {
                let p = px(&mut r, x, y);
                assert!(p == [64; 4] || p == [0; 4], "pixel {x},{y} = {p:?}");
            }
        }
        for (x, y) in [(8, 8), (8, 4), (5, 6), (11, 7), (10, 11), (6, 12), (8, 9), (9, 8)] {
            assert_eq!(px(&mut r, x, y), [64; 4], "pixel {x},{y}");
        }
    }

    #[test]
    fn depth_test_less_keeps_nearer() {
        let mut r = SoftRasterizer::new(4, 4, 16);
        r.clear(None, Some(1.0), [true; 4], None);
        r.set_state(&State { depth_test: Some(Compare::Less), ..State::DEFAULT });
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.3, 1.0, RED);
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.6, 1.0, GREEN);
        assert_eq!(px(&mut r, 1, 1), [255, 0, 0, 255]);
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.3, 1.0, GREEN);
        assert_eq!(px(&mut r, 1, 1), [255, 0, 0, 255]);
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.1, 1.0, BLUE);
        assert_eq!(px(&mut r, 1, 1), [0, 0, 255, 255]);
    }

    #[test]
    fn depth_write_off_and_test_disabled_do_not_write() {
        let mut r = SoftRasterizer::new(4, 4, 24);
        let less = State { depth_test: Some(Compare::Less), ..State::DEFAULT };
        r.set_state(&State { depth_write: false, ..less });
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.2, 1.0, RED);
        r.set_state(&State::DEFAULT);
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.1, 1.0, BLUE);
        assert_eq!(px(&mut r, 2, 2), [0, 0, 255, 255]);
        r.set_state(&less);
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 1.0, GREEN);
        assert_eq!(px(&mut r, 2, 2), [0, 255, 0, 255]);
        // La limpieza de profundidad respeta el recorte.
        r.clear(None, Some(0.0), [true; 4], Some([0, 0, 2, 4]));
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.4, 1.0, RED);
        assert_eq!(px(&mut r, 1, 1), [0, 255, 0, 255]);
        assert_eq!(px(&mut r, 2, 1), [255, 0, 0, 255]);
    }

    #[test]
    fn alpha_test_rejects_fragments() {
        let mut r = SoftRasterizer::new(4, 4, 16);
        r.set_state(&State { alpha_test: Some((Compare::Greater, 0.5)), ..State::DEFAULT });
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 1.0, [1.0, 0.0, 0.0, 0.4]);
        assert_eq!(px(&mut r, 1, 1), [0; 4]);
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 1.0, [0.0, 1.0, 0.0, 0.6]);
        assert_eq!(px(&mut r, 1, 1), [0, 255, 0, 153]);
        r.set_state(&State { alpha_test: Some((Compare::Never, 0.0)), ..State::DEFAULT });
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 1.0, WHITE);
        assert_eq!(px(&mut r, 1, 1), [0, 255, 0, 153]);
    }

    #[test]
    fn blend_src_alpha_one_minus_src_alpha() {
        let mut r = SoftRasterizer::new(4, 4, 16);
        r.clear(Some(BLUE), None, [true; 4], None);
        r.set_state(&State { blend: Some((Factor::SrcAlpha, Factor::OneMinusSrcAlpha)), ..State::DEFAULT });
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 1.0, [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(px(&mut r, 1, 1), [128, 0, 128, 191]);
        // SrcAlphaSaturate: min(As, 1 - Ad) en RGB y 1 en alfa.
        r.clear(Some([0.0, 0.0, 0.0, 0.75]), None, [true; 4], None);
        r.set_state(&State { blend: Some((Factor::SrcAlphaSaturate, Factor::One)), ..State::DEFAULT });
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 1.0, [1.0, 1.0, 1.0, 0.5]);
        assert_eq!(px(&mut r, 1, 1), [64, 64, 64, 255]);
    }

    #[test]
    fn scissor_limits_drawing_and_clear() {
        let mut r = SoftRasterizer::new(6, 6, 16);
        r.set_state(&State { scissor: Some([2, 2, 4, 4]), ..State::DEFAULT });
        quad(&mut r, 0.0, 0.0, 6.0, 6.0, 0.5, 1.0, RED);
        r.clear(Some(GREEN), None, [true; 4], Some([4, 0, 6, 1]));
        for y in 0..6 {
            for x in 0..6 {
                let inside = (2..4).contains(&x) && (2..4).contains(&y);
                let cleared = x >= 4 && y == 0;
                let want = if inside { [255, 0, 0, 255] } else if cleared { [0, 255, 0, 255] } else { [0; 4] };
                assert_eq!(px(&mut r, x, y), want, "pixel {x},{y}");
            }
        }
    }

    #[test]
    fn color_mask_protects_channels() {
        let mut r = SoftRasterizer::new(4, 4, 16);
        let mask = [true, false, true, false];
        r.set_state(&State { color_mask: mask, ..State::DEFAULT });
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 1.0, WHITE);
        assert_eq!(px(&mut r, 1, 1), [255, 0, 255, 0]);
        r.clear(Some([0.5; 4]), None, [false, true, false, false], None);
        assert_eq!(px(&mut r, 1, 1), [255, 128, 255, 0]);
    }

    #[test]
    fn nearest_texture_quadrants_and_wrap() {
        let texels: [[u8; 4]; 4] = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255]];
        let mut r = SoftRasterizer::new(4, 4, 16);
        assert!(r.tex_image(7, 0, 2, 2, &texels.concat()));
        r.set_state(&State { texture: Some(texture(7, 2, 2)), ..State::DEFAULT });
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 1.0, WHITE);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(px(&mut r, x, y), texels[((y / 2) * 2 + x / 2) as usize], "pixel {x},{y}");
            }
        }
        // Coordenadas de 0 a 2: Repeat repite el mosaico, ClampToEdge estira el borde.
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 2.0, WHITE);
        assert_eq!(px(&mut r, 2, 0), texels[0]);
        assert_eq!(px(&mut r, 3, 3), texels[3]);
        assert_eq!(px(&mut r, 2, 3), texels[2]);
        let clamp = TextureState { wrap_s: Wrap::ClampToEdge, wrap_t: Wrap::ClampToEdge, ..texture(7, 2, 2) };
        r.set_state(&State { texture: Some(clamp), ..State::DEFAULT });
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 2.0, WHITE);
        assert_eq!(px(&mut r, 0, 0), texels[0]);
        assert_eq!(px(&mut r, 2, 0), texels[1]);
        assert_eq!(px(&mut r, 2, 3), texels[3]);
        assert_eq!(px(&mut r, 0, 3), texels[2]);
        // tex_sub_image sustituye un texel; tex_delete desactiva la textura.
        r.tex_sub_image(7, 0, 1, 0, 1, 1, &[9, 8, 7, 6]);
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 1.0, WHITE);
        assert_eq!(px(&mut r, 3, 0), [9, 8, 7, 6]);
        r.tex_delete(7);
        quad(&mut r, 0.0, 0.0, 4.0, 4.0, 0.5, 1.0, RED);
        assert_eq!(px(&mut r, 3, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn linear_filter_blends_texel_centers() {
        let mut r = SoftRasterizer::new(4, 1, 16);
        r.tex_image(1, 0, 2, 1, &[0, 0, 0, 255, 255, 255, 255, 255]);
        let tex = TextureState { mag: Filter::Linear, wrap_s: Wrap::ClampToEdge, ..texture(1, 2, 1) };
        r.set_state(&State { texture: Some(tex), ..State::DEFAULT });
        // 4 píxeles sobre 2 texels: u - 0.5 = -0.25, 0.25, 0.75, 1.25.
        quad(&mut r, 0.0, 0.0, 4.0, 1.0, 0.5, 1.0, WHITE);
        let got: Vec<u8> = (0..4).map(|x| px(&mut r, x, 0)[0]).collect();
        assert_eq!(got, [0, 64, 191, 255]);
    }

    #[test]
    fn texture_environments() {
        let mut r = SoftRasterizer::new(2, 2, 16);
        r.tex_image(1, 0, 1, 1, &[128, 255, 0, 128]);
        let frag = [0.5, 0.5, 1.0, 0.5];
        let mut draw = |env, format| {
            let tex = TextureState { env, format, env_color: [1.0, 0.0, 1.0, 0.0], ..texture(1, 1, 1) };
            r.set_state(&State { texture: Some(tex), ..State::DEFAULT });
            quad(&mut r, 0.0, 0.0, 2.0, 2.0, 0.5, 1.0, frag);
            px(&mut r, 0, 0)
        };
        assert_eq!(draw(TexEnv::Modulate, TexFormat::Rgba), [64, 128, 0, 64]);
        assert_eq!(draw(TexEnv::Modulate, TexFormat::Rgb), [64, 128, 0, 128]);
        assert_eq!(draw(TexEnv::Modulate, TexFormat::Alpha), [128, 128, 255, 64]);
        assert_eq!(draw(TexEnv::Replace, TexFormat::Rgba), [128, 255, 0, 128]);
        assert_eq!(draw(TexEnv::Replace, TexFormat::Rgb), [128, 255, 0, 128]);
        assert_eq!(draw(TexEnv::Replace, TexFormat::Alpha), [128, 128, 255, 128]);
        // Decal Rgba: Cf*(1-At) + Ct*At con At = 128/255.
        assert_eq!(draw(TexEnv::Decal, TexFormat::Rgba), [128, 192, 127, 128]);
        assert_eq!(draw(TexEnv::Decal, TexFormat::Rgb), [128, 255, 0, 128]);
        // Blend: Cf*(1-Ct) + Cc*Ct.
        assert_eq!(draw(TexEnv::Blend, TexFormat::Rgba), [192, 0, 255, 64]);
        assert_eq!(draw(TexEnv::Blend, TexFormat::Luminance), [192, 0, 255, 128]);
    }

    #[test]
    fn texture_environment_add() {
        let mut r = SoftRasterizer::new(2, 2, 16);
        r.tex_image(1, 0, 1, 1, &[64, 255, 0, 128]);
        let mut draw = |format| {
            let tex = TextureState { env: TexEnv::Add, format, ..texture(1, 1, 1) };
            r.set_state(&State { texture: Some(tex), ..State::DEFAULT });
            quad(&mut r, 0.0, 0.0, 2.0, 2.0, 0.5, 1.0, [0.5, 0.5, 1.0, 0.5]);
            px(&mut r, 0, 0)
        };
        // C = Cf + Ct limitado a 1; el alfa se modula sólo si la textura lo tiene.
        assert_eq!(draw(TexFormat::Rgba), [192, 255, 255, 64]);
        assert_eq!(draw(TexFormat::LuminanceAlpha), [192, 255, 255, 64]);
        assert_eq!(draw(TexFormat::Rgb), [192, 255, 255, 128]);
        assert_eq!(draw(TexFormat::Luminance), [192, 255, 255, 128]);
        assert_eq!(draw(TexFormat::Alpha), [128, 128, 255, 64]);
    }

    #[test]
    fn mip_level_selection() {
        let mut r = SoftRasterizer::new(8, 8, 16);
        r.tex_image(3, 0, 4, 4, &solid(4, 4, [255, 0, 0, 255]));
        r.tex_image(3, 1, 2, 2, &solid(2, 2, [0, 255, 0, 255]));
        r.tex_image(3, 2, 1, 1, &solid(1, 1, [0, 0, 255, 255]));
        let mip = |min, levels| TextureState { min, levels, ..texture(3, 4, 4) };
        let mut draw = |tex, w: f32, h: f32| {
            r.set_state(&State { texture: Some(tex), ..State::DEFAULT });
            quad(&mut r, 0.0, 0.0, w, h, 0.5, 1.0, WHITE);
            px(&mut r, 0, 0)
        };
        let nearest = mip(Filter::NearestMipNearest, 3);
        assert_eq!(draw(nearest, 8.0, 8.0), [255, 0, 0, 255]); // lod = -1
        assert_eq!(draw(nearest, 4.0, 4.0), [255, 0, 0, 255]); // lod = 0
        assert_eq!(draw(nearest, 2.0, 2.0), [0, 255, 0, 255]); // lod = 1
        assert_eq!(draw(nearest, 1.0, 1.0), [0, 0, 255, 255]); // lod = 2
        // El nivel se limita a `levels - 1`, y sin filtro de mipmap es siempre el 0.
        assert_eq!(draw(mip(Filter::NearestMipNearest, 2), 1.0, 1.0), [0, 255, 0, 255]);
        assert_eq!(draw(mip(Filter::Nearest, 3), 1.0, 1.0), [255, 0, 0, 255]);
        // lod = 0.5: mitad del nivel 0 y mitad del 1.
        assert_eq!(draw(mip(Filter::NearestMipLinear, 3), 4.0, 2.0), [128, 128, 0, 255]);
        assert_eq!(draw(mip(Filter::LinearMipLinear, 3), 1.0, 1.0), [0, 0, 255, 255]);
    }

    #[test]
    fn perspective_correct_interpolation() {
        // Lado izquierdo con w = 1 y negro; lado derecho con w = 4 y blanco.
        let v = |x, y, q, c: f32| Vertex { q, s: c, ..vtx(x, y, [c, c, c, 1.0]) };
        let (a, b, c, d) = (v(0.0, 0.0, 1.0, 0.0), v(8.0, 0.0, 0.25, 1.0), v(8.0, 8.0, 0.25, 1.0), v(0.0, 8.0, 1.0, 0.0));
        let mut r = SoftRasterizer::new(8, 8, 16);
        r.triangle(&a, &b, &c);
        r.triangle(&a, &c, &d);
        let want = |x: f32| {
            let w = (x + 0.5) / 8.0;
            to_u8(w * 0.25 / ((1.0 - w) + w * 0.25))
        };
        for (x, y) in [(4, 4), (4, 1), (1, 6), (6, 3)] {
            let got = px(&mut r, x, y)[0] as i32;
            assert!((got - want(x as f32) as i32).abs() <= 1, "pixel {x},{y} = {got}");
        }
        assert_eq!(want(4.0), 62); // lineal sería 143
        // También a lo largo de una línea, y con q constante es lineal.
        r.line(&Vertex { y: 0.5, ..a }, &Vertex { y: 0.5, ..b });
        assert_eq!(px(&mut r, 4, 0)[0], 62);
        r.line(&Vertex { y: 0.5, q: 1.0, ..a }, &Vertex { y: 0.5, q: 1.0, ..b });
        assert_eq!(px(&mut r, 4, 0)[0], 143);
    }

    #[test]
    fn fog_blends_rgb_only() {
        let mut r = SoftRasterizer::new(2, 2, 16);
        r.set_state(&State { fog_color: Some([0.0, 0.0, 1.0]), ..State::DEFAULT });
        r.point(&Vertex { fog: 0.25, ..vtx(0.5, 0.5, [1.0, 1.0, 0.0, 0.5]) });
        assert_eq!(px(&mut r, 0, 0), [64, 64, 191, 128]);
    }

    #[test]
    fn points_lines_and_rects() {
        let mut r = SoftRasterizer::new(6, 6, 16);
        r.point(&vtx(2.9, 1.1, RED));
        assert_eq!(px(&mut r, 2, 1), [255, 0, 0, 255]);
        // Extremo inicial incluido, final excluido; en los dos sentidos.
        r.line(&vtx(0.5, 3.5, GREEN), &vtx(3.5, 3.5, GREEN));
        r.line(&vtx(5.5, 4.5, BLUE), &vtx(5.5, 1.5, BLUE));
        let row: Vec<u8> = (0..6).map(|x| px(&mut r, x, 3)[1]).collect();
        assert_eq!(row, [255, 255, 255, 0, 0, 0]);
        let col: Vec<u8> = (0..6).map(|y| px(&mut r, 5, y)[2]).collect();
        assert_eq!(col, [0, 0, 255, 255, 255, 0]);
        // Diagonal: un píxel por columna.
        let mut r = SoftRasterizer::new(6, 6, 16);
        r.line(&vtx(0.5, 0.5, WHITE), &vtx(5.5, 5.5, WHITE));
        r.rect(3.4, 0.0, 5.4, 1.5,&vtx(0.0, 0.0, RED));
        for y in 0..6 {
            for x in 0..6 {
                let want = if (3..=4).contains(&x) && y == 0 {
                    [255, 0, 0, 255]
                } else if x == y && x < 5 {
                    [255; 4]
                } else {
                    [0; 4]
                };
                assert_eq!(px(&mut r, x, y), want, "pixel {x},{y}");
            }
        }
    }

    #[test]
    fn read_pixels_row_order() {
        let mut r = SoftRasterizer::new(3, 3, 16);
        assert_eq!((r.size(), r.depth_bits()), ((3, 3), 16));
        r.point(&vtx(1.5, 0.5, RED));
        r.point(&vtx(0.5, 1.5, GREEN));
        r.point(&vtx(2.5, 2.5, BLUE));
        let mut out = [0u8; 16];
        r.read_pixels(0, 0, 2, 2, &mut out);
        assert_eq!(out, [0, 0, 0, 0, 255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 0, 0]);
        r.read_pixels(1, 1, 2, 2, &mut out);
        assert_eq!(out, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255]);
    }
}

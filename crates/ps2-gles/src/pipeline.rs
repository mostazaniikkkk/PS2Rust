//! Del array de vértices al rasterizador: lectura de atributos,
//! transformación, iluminación, niebla, ensamblado de primitivas, recorte
//! contra el volumen de visión, descarte de caras y paso a coordenadas de
//! ventana.

use crate::consts::*;
use crate::context::*;
use crate::math::{self, clamp01, Mat4};
use crate::raster::{self, Vertex};
use alloc::vec::Vec;

/// Vértices de un triángulo tras recortarlo con los seis planos, como mucho.
const MAX_POLYGON: usize = 10;

/// Vértice en coordenadas de recorte, con todo lo que se interpola.
#[derive(Clone, Copy)]
struct ClipVertex {
    clip: [f32; 4],
    color: [f32; 4],
    /// Color de la cara trasera (sólo con iluminación a dos caras).
    back: [f32; 4],
    st: [f32; 2],
    fog: f32,
}

fn lerp(a: &ClipVertex, b: &ClipVertex, t: f32) -> ClipVertex {
    let mix = |x: f32, y: f32| x + (y - x) * t;
    let mix4 = |x: [f32; 4], y: [f32; 4]| [mix(x[0], y[0]), mix(x[1], y[1]), mix(x[2], y[2]), mix(x[3], y[3])];
    ClipVertex {
        clip: mix4(a.clip, b.clip),
        color: mix4(a.color, b.color),
        back: mix4(a.back, b.back),
        st: [mix(a.st[0], b.st[0]), mix(a.st[1], b.st[1])],
        fog: mix(a.fog, b.fog),
    }
}

/// Lee el elemento `index` de un array como hasta cuatro `f32`. Los punteros
/// pueden no estar alineados (arrays intercalados).
unsafe fn fetch(array: &Array, index: usize, normalized: bool) -> [f32; 4] {
    let size = array.size as usize;
    let width = match array.kind {
        GL_BYTE | GL_UNSIGNED_BYTE => 1,
        GL_SHORT | GL_UNSIGNED_SHORT => 2,
        _ => 4,
    };
    let stride = if array.stride == 0 { size * width } else { array.stride as usize };
    let base = array.pointer.add(index * stride);
    let mut out = [0.0, 0.0, 0.0, 1.0];
    if array.kind == GL_FLOAT {
        // El caso de casi siempre, sin mirar el tipo componente a componente.
        let source = base as *const f32;
        for (i, slot) in out.iter_mut().enumerate().take(size) {
            *slot = source.add(i).read_unaligned();
        }
        return out;
    }
    for (i, slot) in out.iter_mut().enumerate().take(size) {
        let p = base.add(i * width);
        *slot = match array.kind {
            GL_BYTE => {
                let v = (p as *const i8).read() as f32;
                if normalized {
                    (2.0 * v + 1.0) / 255.0
                } else {
                    v
                }
            }
            GL_UNSIGNED_BYTE => {
                let v = p.read() as f32;
                if normalized {
                    v / 255.0
                } else {
                    v
                }
            }
            GL_SHORT => {
                let v = (p as *const i16).read_unaligned() as f32;
                if normalized {
                    (2.0 * v + 1.0) / 65535.0
                } else {
                    v
                }
            }
            GL_FIXED => (p as *const i32).read_unaligned() as f32 / 65536.0,
            _ => (p as *const f32).read_unaligned(),
        };
    }
    out
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len2 = dot3(v, v);
    if len2 <= 0.0 {
        return v;
    }
    let k = 1.0 / math::sqrt(len2);
    [v[0] * k, v[1] * k, v[2] * k]
}

/// Índices de matriz y pesos del vértice `index` (`GL_OES_matrix_palette`).
unsafe fn skin_of(arrays: &[Array; ARRAYS], index: usize) -> ([usize; 4], [f32; 4]) {
    let i = fetch(&arrays[ARRAY_MATRIX_INDEX], index, false);
    let w = fetch(&arrays[ARRAY_WEIGHT], index, false);
    (i.map(|i| (i as usize).min(MAX_PALETTE_MATRICES - 1)), w)
}

/// Copia un vértice del camino rápido (de 12 o de 20 números) con tamaño
/// conocido al compilar, que es mucho más rápido que una copia genérica.
#[inline(always)]
fn copy_vertex(to: &mut [f32], from: &[f32]) {
    if let (Ok(to), Ok(from)) = (<&mut [f32; 12]>::try_from(&mut *to), <&[f32; 12]>::try_from(from)) {
        *to = *from;
    } else if let (Ok(to), Ok(from)) = (<&mut [f32; 20]>::try_from(to), <&[f32; 20]>::try_from(from)) {
        *to = *from;
    }
}

/// Rellena `out` con vértices de `N` números del camino rápido. Con `first`
/// van seguidos desde ese índice y se leen directamente; si no, el vértice
/// número `n` de la lista es el de índice `index_at(n) + lowest`, que se lee
/// una sola vez y se guarda en `table`.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn expand<const N: usize>(out: &mut [f32], first: Option<usize>, next: &mut usize, table: &mut [f32], known: &mut [bool], lowest: usize, index_at: impl Fn(usize) -> usize, read: impl Fn(usize, &mut [f32])) {
    let (slots, _) = out.as_chunks_mut::<N>();
    if let Some(first) = first {
        for (i, slot) in slots.iter_mut().enumerate() {
            read(first + i, slot);
        }
        *next += slots.len();
        return;
    }
    let (cache, _) = table.as_chunks_mut::<N>();
    for slot in slots {
        let at = index_at(*next);
        *next += 1;
        if !known[at] {
            read(at + lowest, &mut cache[at]);
            known[at] = true;
        }
        *slot = cache[at];
    }
}

/// Matrices y datos que no cambian durante una llamada de dibujo.
struct Frame {
    modelview: Mat4,
    projection: Mat4,
    texture: Mat4,
    texture_identity: bool,
    /// Inversa de la modelview, para llevar las normales a coordenadas de ojo.
    inverse: Mat4,
    /// Factor de `GL_RESCALE_NORMAL`.
    rescale: f32,
}

impl Context {
    fn frame(&self) -> Frame {
        let modelview = *self.modelview.last().unwrap();
        let texture = *self.texture_matrix.last().unwrap();
        let inverse = math::invert(&modelview).unwrap_or(math::IDENTITY);
        let row = [inverse[2], inverse[6], inverse[10]];
        let len2 = dot3(row, row);
        Frame {
            modelview,
            projection: *self.projection.last().unwrap(),
            texture,
            texture_identity: texture == math::IDENTITY,
            inverse,
            rescale: if len2 > 0.0 { 1.0 / math::sqrt(len2) } else { 1.0 },
        }
    }

    /// Ecuación de iluminación de GL para un vértice (en coordenadas de ojo).
    fn light(&self, eye: [f32; 4], normal: [f32; 3], vertex_color: [f32; 4]) -> [f32; 4] {
        let m = &self.material;
        let (ambient, diffuse) = if self.color_material { (vertex_color, vertex_color) } else { (m.ambient, m.diffuse) };
        let mut c = [0.0f32; 3];
        for k in 0..3 {
            c[k] = m.emission[k] + ambient[k] * self.light_model_ambient[k];
        }
        for l in self.lights.iter().filter(|l| l.enabled) {
            // Vector del vértice a la luz y atenuación por distancia.
            let (to_light, attenuation) = if l.position[3] == 0.0 {
                (normalize3([l.position[0], l.position[1], l.position[2]]), 1.0)
            } else {
                let iw = if eye[3] != 0.0 { 1.0 / eye[3] } else { 1.0 };
                let lw = 1.0 / l.position[3];
                let d = [l.position[0] * lw - eye[0] * iw, l.position[1] * lw - eye[1] * iw, l.position[2] * lw - eye[2] * iw];
                let dist = math::sqrt(dot3(d, d));
                let att = 1.0 / (l.attenuation[0] + l.attenuation[1] * dist + l.attenuation[2] * dist * dist);
                (normalize3(d), att)
            };
            let spot = if l.spot_cutoff == 180.0 {
                1.0
            } else {
                let cos_angle = -dot3(to_light, normalize3(l.spot_direction));
                if cos_angle < math::cos(l.spot_cutoff * (core::f32::consts::PI / 180.0)) {
                    0.0
                } else {
                    math::pow(cos_angle.max(0.0), l.spot_exponent)
                }
            };
            let n_dot_l = dot3(normal, to_light).max(0.0);
            let shiny = m.specular[0] != 0.0 || m.specular[1] != 0.0 || m.specular[2] != 0.0;
            let specular = if n_dot_l > 0.0 && shiny {
                // Observador en el infinito: vector medio con (0, 0, 1).
                let half = normalize3([to_light[0], to_light[1], to_light[2] + 1.0]);
                math::pow(dot3(normal, half).max(0.0), m.shininess)
            } else {
                0.0
            };
            let k = attenuation * spot;
            for i in 0..3 {
                c[i] += k * (ambient[i] * l.ambient[i] + n_dot_l * diffuse[i] * l.diffuse[i] + specular * m.specular[i] * l.specular[i]);
            }
        }
        [clamp01(c[0]), clamp01(c[1]), clamp01(c[2]), clamp01(diffuse[3])]
    }

    /// Matrices que se mezclan en cada vértice (`GL_OES_matrix_palette`), o 0
    /// si todos usan la modelview.
    fn skin_units(&self) -> usize {
        let (indices, weights) = (&self.arrays[ARRAY_MATRIX_INDEX], &self.arrays[ARRAY_WEIGHT]);
        if self.matrix_palette && indices.enabled && weights.enabled {
            indices.size.min(weights.size).max(0) as usize
        } else {
            0
        }
    }

    /// Transforma e ilumina el vértice `index` de los arrays.
    unsafe fn process(&self, f: &Frame, index: usize) -> ClipVertex {
        let object = fetch(&self.arrays[ARRAY_VERTEX], index, false);
        let units = self.skin_units();
        // Con paleta, la modelview del vértice es la mezcla de las suyas.
        let mut blended = [0.0f32; 16];
        if units > 0 {
            let (indices, weights) = skin_of(&self.arrays, index);
            for u in 0..units {
                let matrix = &self.palette[indices[u]][0];
                for (b, m) in blended.iter_mut().zip(matrix) {
                    *b += weights[u] * m;
                }
            }
        }
        let eye = math::transform(if units > 0 { &blended } else { &f.modelview }, object);
        let clip = math::transform(&f.projection, eye);
        let color = if self.arrays[ARRAY_COLOR].enabled { fetch(&self.arrays[ARRAY_COLOR], index, true) } else { self.color };
        let (color, back) = if self.lighting {
            let n = if self.arrays[ARRAY_NORMAL].enabled {
                let n = fetch(&self.arrays[ARRAY_NORMAL], index, true);
                [n[0], n[1], n[2]]
            } else {
                self.normal
            };
            // Las normales se transforman con la traspuesta de la inversa.
            let mut n = if units > 0 {
                // La de la matriz mezclada: cofactores entre determinante.
                let column = |k: usize| [blended[k * 4], blended[k * 4 + 1], blended[k * 4 + 2]];
                let x = [cross(column(1), column(2)), cross(column(2), column(0)), cross(column(0), column(1))];
                let det = dot3(column(0), x[0]);
                let k = if det != 0.0 { 1.0 / det } else { 1.0 };
                [0, 1, 2].map(|j| (x[0][j] * n[0] + x[1][j] * n[1] + x[2][j] * n[2]) * k)
            } else {
                let i = &f.inverse;
                [n[0] * i[0] + n[1] * i[1] + n[2] * i[2], n[0] * i[4] + n[1] * i[5] + n[2] * i[6], n[0] * i[8] + n[1] * i[9] + n[2] * i[10]]
            };
            if self.rescale_normal && units == 0 {
                n = [n[0] * f.rescale, n[1] * f.rescale, n[2] * f.rescale];
            }
            // Con paleta no hay una escala única: reescalar es normalizar.
            if self.normalize || (self.rescale_normal && units > 0) {
                n = normalize3(n);
            }
            let front = self.light(eye, n, color);
            let back = if self.two_side { self.light(eye, [-n[0], -n[1], -n[2]], color) } else { front };
            (front, back)
        } else {
            let c = [clamp01(color[0]), clamp01(color[1]), clamp01(color[2]), clamp01(color[3])];
            (c, c)
        };
        let mut tc = if self.arrays[ARRAY_TEXCOORD].enabled { fetch(&self.arrays[ARRAY_TEXCOORD], index, false) } else { self.texcoord };
        if !f.texture_identity {
            tc = math::transform(&f.texture, tc);
        }
        let st = if tc[3] != 1.0 && tc[3] != 0.0 { [tc[0] / tc[3], tc[1] / tc[3]] } else { [tc[0], tc[1]] };
        let fog = if self.fog {
            let distance = if eye[2] < 0.0 { -eye[2] } else { eye[2] };
            clamp01(match self.fog_mode {
                GL_LINEAR => (self.fog_end - distance) / (self.fog_end - self.fog_start),
                GL_EXP2 => math::exp(-(self.fog_density * distance) * (self.fog_density * distance)),
                _ => math::exp(-self.fog_density * distance),
            })
        } else {
            1.0
        };
        ClipVertex { clip, color, back, st, fog }
    }

    /// De coordenadas de recorte a coordenadas de ventana del rasterizador.
    fn to_window(&self, v: &ClipVertex, back: bool) -> Vertex {
        let (_, height) = self.raster.size();
        let [vx, vy, vw, vh] = self.viewport;
        let q = 1.0 / v.clip[3];
        let (nx, ny, nz) = (v.clip[0] * q, v.clip[1] * q, v.clip[2] * q);
        let [near, far] = self.depth_range;
        Vertex {
            x: (nx * 0.5 + 0.5) * vw as f32 + vx as f32,
            y: height as f32 - ((ny * 0.5 + 0.5) * vh as f32 + vy as f32),
            z: clamp01((far - near) * 0.5 * nz + (near + far) * 0.5),
            q,
            s: v.st[0],
            t: v.st[1],
            color: if back { v.back } else { v.color },
            fog: v.fog,
        }
    }

    fn draw_point(&mut self, v: &ClipVertex) {
        let c = v.clip;
        if c[3] <= 0.0 || c[0].abs() > c[3] || c[1].abs() > c[3] || c[2].abs() > c[3] {
            return;
        }
        let w = self.to_window(v, false);
        if self.point_size <= 1.0 {
            self.raster.point(&w);
        } else {
            let half = self.point_size * 0.5;
            self.raster.rect(w.x - half, w.y - half, w.x + half, w.y + half, &w);
        }
    }

    fn draw_line(&mut self, a: &ClipVertex, b: &ClipVertex) {
        // Recorte del segmento contra los seis planos (Liang-Barsky).
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        for plane in 0..6 {
            let d = |v: &ClipVertex| {
                let c = v.clip;
                if plane % 2 == 0 {
                    c[3] - c[plane / 2]
                } else {
                    c[3] + c[plane / 2]
                }
            };
            let (da, db) = (d(a), d(b));
            if da < 0.0 && db < 0.0 {
                return;
            }
            if da < 0.0 {
                t0 = t0.max(da / (da - db));
            } else if db < 0.0 {
                t1 = t1.min(da / (da - db));
            }
        }
        if t0 > t1 {
            return;
        }
        let mut pa = if t0 > 0.0 { lerp(a, b, t0) } else { *a };
        let pb = if t1 < 1.0 { lerp(a, b, t1) } else { *b };
        if !self.smooth {
            pa.color = pb.color;
        }
        let (wa, wb) = (self.to_window(&pa, false), self.to_window(&pb, false));
        if self.line_width <= 1.0 {
            self.raster.line(&wa, &wb);
            return;
        }
        // Línea ancha: un cuadrilátero perpendicular a su dirección.
        let (dx, dy) = (wb.x - wa.x, wb.y - wa.y);
        let len = math::sqrt(dx * dx + dy * dy);
        if len <= 0.0 {
            return;
        }
        let (ox, oy) = (-dy / len * self.line_width * 0.5, dx / len * self.line_width * 0.5);
        let shift = |v: &Vertex, sign: f32| Vertex { x: v.x + ox * sign, y: v.y + oy * sign, ..*v };
        let (a0, a1, b0, b1) = (shift(&wa, 1.0), shift(&wa, -1.0), shift(&wb, 1.0), shift(&wb, -1.0));
        self.raster.triangle(&a0, &a1, &b1);
        self.raster.triangle(&a0, &b1, &b0);
    }

    fn draw_triangle(&mut self, a: &ClipVertex, b: &ClipVertex, c: &ClipVertex) {
        // Un triángulo recortado por seis planos tiene como mucho 9 vértices.
        let mut poly = [*a; MAX_POLYGON];
        poly[1] = *b;
        poly[2] = *c;
        let mut count = 3;
        if !self.smooth {
            // Sombreado plano: manda el color del último vértice.
            for v in &mut poly[..3] {
                v.color = c.color;
                v.back = c.back;
            }
        }
        // Recorte contra los seis planos (Sutherland-Hodgman), sólo si hace falta.
        let distance = |v: &ClipVertex, plane: usize| {
            let k = v.clip;
            if plane % 2 == 0 {
                k[3] - k[plane / 2]
            } else {
                k[3] + k[plane / 2]
            }
        };
        for plane in 0..6 {
            if poly[..count].iter().all(|v| distance(v, plane) >= 0.0) {
                continue;
            }
            let mut out = [*a; MAX_POLYGON];
            let mut n = 0;
            for i in 0..count {
                let (cur, next) = (&poly[i], &poly[(i + 1) % count]);
                let (dc, dn) = (distance(cur, plane), distance(next, plane));
                if dc >= 0.0 && n < MAX_POLYGON {
                    out[n] = *cur;
                    n += 1;
                }
                if (dc >= 0.0) != (dn >= 0.0) && n < MAX_POLYGON {
                    out[n] = lerp(cur, next, dc / (dc - dn));
                    n += 1;
                }
            }
            poly = out;
            count = n;
            if count < 3 {
                return;
            }
        }
        let poly = &poly[..count];
        // Orientación en coordenadas de ventana de GL (y hacia arriba).
        let mut window = [Vertex::default(); MAX_POLYGON];
        for (w, v) in window.iter_mut().zip(poly) {
            *w = self.to_window(v, false);
        }
        let front = &mut window[..count];
        let mut area = 0.0;
        for i in 0..count {
            let (p, n) = (&front[i], &front[(i + 1) % count]);
            // La `y` del rasterizador va hacia abajo: se invierte el signo.
            area -= p.x * n.y - n.x * p.y;
        }
        if area == 0.0 {
            return;
        }
        let is_front = (area > 0.0) == (self.front_face == GL_CCW);
        if self.cull_face {
            let culled = match self.cull_mode {
                GL_FRONT => is_front,
                GL_BACK => !is_front,
                _ => true,
            };
            if culled {
                return;
            }
        }
        if !is_front && self.two_side && self.lighting {
            for (w, v) in front.iter_mut().zip(poly) {
                w.color = v.back;
            }
        }
        if self.polygon_offset_fill && self.polygon_offset != [0.0, 0.0] {
            // Pendiente máxima de la profundidad en el plano del triángulo.
            let (p0, p1, p2) = (front[0], front[1], front[2]);
            let (ax, ay, az) = (p1.x - p0.x, p1.y - p0.y, p1.z - p0.z);
            let (bx, by, bz) = (p2.x - p0.x, p2.y - p0.y, p2.z - p0.z);
            let det = ax * by - ay * bx;
            let slope = if det != 0.0 {
                let dzdx = ((az * by - bz * ay) / det).abs();
                let dzdy = ((bz * ax - az * bx) / det).abs();
                dzdx.max(dzdy)
            } else {
                0.0
            };
            let unit = 1.0 / ((1u32 << self.raster.depth_bits()) - 1) as f32;
            let offset = self.polygon_offset[0] * slope + self.polygon_offset[1] * unit;
            for v in front.iter_mut() {
                v.z = clamp01(v.z + offset);
            }
        }
        for i in 1..count - 1 {
            self.raster.triangle(&front[0], &front[i], &front[i + 1]);
        }
    }

    /// Camino rápido: si la llamada es de triángulos y no usa nada que el
    /// rasterizador no sepa hacer por su cuenta (en la PS2, la VU1), se le
    /// entregan los vértices sin transformar y él hace toda la geometría.
    /// Devuelve `false` si hay que seguir por el camino normal.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_fast(&mut self, mode: GLenum, count: usize, first: Option<usize>, index_of: &dyn Fn(usize) -> usize, source: Option<(GLuint, GLenum, usize)>, offsets: &[Array; ARRAYS], frame: &Frame, state: &raster::State) -> bool {
        if !matches!(mode, GL_TRIANGLES | GL_TRIANGLE_STRIP | GL_TRIANGLE_FAN) || count < 3 {
            return false;
        }
        if self.arrays[ARRAY_VERTEX].size > 3 || (self.polygon_offset_fill && self.polygon_offset != [0.0, 0.0]) {
            return false;
        }
        let units = self.skin_units();
        let is_affine = |m: &Mat4| m[3] == 0.0 && m[7] == 0.0 && m[11] == 0.0 && m[15] == 1.0;
        let affine = if units > 0 { self.palette.iter().all(|m| is_affine(&m[0])) } else { is_affine(&frame.modelview) };
        if (self.lighting || self.fog) && !affine {
            return false;
        }
        let mut palette = [math::IDENTITY; MAX_PALETTE_MATRICES];
        if units > 0 {
            for (slot, matrix) in palette.iter_mut().zip(&self.palette) {
                *slot = matrix[0];
            }
        }
        if self.fog && (self.fog_mode != GL_LINEAR || self.fog_end == self.fog_start) {
            return false;
        }
        let material = self.material;
        if self.lighting {
            // Sin brillo especular, focos ni iluminación a dos caras.
            let shiny = material.specular[..3] != [0.0; 3];
            let spots = self.lights.iter().any(|l| l.enabled && l.spot_cutoff != 180.0);
            if self.two_side || shiny || spots {
                return false;
            }
        }
        let ccw = self.front_face == GL_CCW;
        let cull = match (self.cull_face, self.cull_mode) {
            (false, _) => 0.0,
            // No queda ninguna cara que dibujar.
            (true, GL_FRONT_AND_BACK) => return true,
            (true, GL_FRONT) => {
                if ccw {
                    -1.0
                } else {
                    1.0
                }
            }
            (true, _) => {
                if ccw {
                    1.0
                } else {
                    -1.0
                }
            }
        };
        let color_material = self.color_material;
        // Color igual para todos los vértices: con luces, si el color del
        // vértice no hace de material; sin ellas, si no hay array de colores.
        let const_color = match (self.lighting, color_material, self.arrays[ARRAY_COLOR].enabled) {
            (true, false, _) => Some([1.0, 1.0, 1.0, material.diffuse[3]]),
            (_, _, false) => Some(self.color),
            _ => None,
        };
        let times = |a: [f32; 4], b: [f32; 4]| [a[0] * b[0], a[1] * b[1], a[2] * b[2]];
        let rgb = |a: [f32; 4]| [a[0], a[1], a[2]];
        let mut lights = [raster::FastLight::default(); 8];
        let mut light_count = 0;
        if self.lighting {
            for l in self.lights.iter().filter(|l| l.enabled) {
                let positional = l.position[3] != 0.0;
                let p = l.position;
                lights[light_count] = raster::FastLight {
                    vector: if positional { [p[0] / p[3], p[1] / p[3], p[2] / p[3]] } else { normalize3([p[0], p[1], p[2]]) },
                    positional,
                    ambient: if color_material { rgb(l.ambient) } else { times(material.ambient, l.ambient) },
                    diffuse: if color_material { rgb(l.diffuse) } else { times(material.diffuse, l.diffuse) },
                    attenuation: l.attenuation,
                };
                light_count += 1;
            }
        }
        let i = &frame.inverse;
        let k = if self.rescale_normal { frame.rescale } else { 1.0 };
        let setup = raster::FastSetup {
            mvp: math::mul(&frame.projection, &frame.modelview),
            modelview: frame.modelview,
            // n_ojo = traspuesta de la inversa · n, columna a columna.
            normal: [[i[0] * k, i[4] * k, i[8] * k], [i[1] * k, i[5] * k, i[9] * k], [i[2] * k, i[6] * k, i[10] * k]],
            viewport: self.viewport,
            depth_range: self.depth_range,
            lighting: self.lighting,
            normalize: self.normalize || (self.rescale_normal && units > 0),
            lights,
            light_count,
            base: if self.lighting { rgb(material.emission) } else { [0.0; 3] },
            scene: match (self.lighting, color_material) {
                (false, _) => [1.0; 3],
                (true, true) => rgb(self.light_model_ambient),
                (true, false) => times(material.ambient, self.light_model_ambient),
            },
            fog: self.fog.then(|| (self.fog_end, 1.0 / (self.fog_end - self.fog_start))),
            cull,
            flat: !self.smooth,
            projection: frame.projection,
            skin: units,
            palette: &palette[..if units > 0 { MAX_PALETTE_MATRICES } else { 0 }],
            const_color,
        };
        let floats = raster::fast_vertex_floats(units);
        // Lee un vértice de los arrays, tal cual. (Con copias del estado que
        // necesita, para no retener el contexto mientras se dibuja.)
        let (arrays, lighting, current_normal, current_texcoord) = (self.arrays, self.lighting, self.normal, self.texcoord);
        let read = |index: usize, out: &mut [f32]| {
            let p = fetch(&arrays[ARRAY_VERTEX], index, false);
            let n = match (lighting, arrays[ARRAY_NORMAL].enabled) {
                (false, _) => [0.0; 4],
                (true, true) => fetch(&arrays[ARRAY_NORMAL], index, true),
                (true, false) => [current_normal[0], current_normal[1], current_normal[2], 0.0],
            };
            let mut tc = if arrays[ARRAY_TEXCOORD].enabled { fetch(&arrays[ARRAY_TEXCOORD], index, false) } else { current_texcoord };
            if !frame.texture_identity {
                tc = math::transform(&frame.texture, tc);
            }
            let (s, t) = if tc[3] != 1.0 && tc[3] != 0.0 { (tc[0] / tc[3], tc[1] / tc[3]) } else { (tc[0], tc[1]) };
            let color = if const_color.is_some() { [0.0; 4] } else { fetch(&arrays[ARRAY_COLOR], index, true) };
            copy_vertex(&mut out[..12], &[p[0], p[1], p[2], s, n[0], n[1], n[2], t, color[0], color[1], color[2], color[3]]);
            if units > 0 {
                let (indices, weights) = skin_of(&arrays, index);
                for u in 0..4 {
                    // Las matrices que no se usan, con peso 0.
                    out[12 + u] = if u < units { weights[u] } else { 0.0 };
                    let matrix = if u < units { indices[u] } else { 0 };
                    out[16 + u] = f32::from_bits(raster::FAST_PALETTE_BASE + 4 * matrix as u32);
                }
            }
        };
        // Triángulos sueltos sin índices: cada vértice se usa una vez y va
        // directo. Si no, cada vértice se lee una sola vez y se guarda.
        let direct = first.filter(|_| mode == GL_TRIANGLES);
        let (mut table, mut known, mut lowest): (Vec<f32>, Vec<bool>, usize) = (Vec::new(), Vec::new(), 0);
        let total = match mode {
            GL_TRIANGLES => count / 3 * 3,
            _ => (count - 2) * 3,
        };
        // Vértice número `n` de la lista de triángulos sueltos.
        let mut next = 0;
        let mut fill = |out: &mut [f32]| {
            if direct.is_none() && known.is_empty() {
                let (mut low, mut high) = (usize::MAX, 0);
                for position in 0..count {
                    let index = index_of(position);
                    low = low.min(index);
                    high = high.max(index);
                }
                lowest = low;
                table = alloc::vec![0.0f32; (high - low + 1) * floats];
                known = alloc::vec![false; high - low + 1];
            }
            // Índice (menos `lowest`) del vértice `n` de la lista de triángulos.
            let index_at = |n: usize| {
                let (triangle, corner) = (n / 3, n % 3);
                let position = match mode {
                    GL_TRIANGLES => n,
                    // Los triángulos impares de una tira van al revés para
                    // conservar la orientación.
                    GL_TRIANGLE_STRIP => match (corner, triangle % 2) {
                        (2, _) => triangle + 2,
                        (0, 0) | (1, 1) => triangle,
                        _ => triangle + 1,
                    },
                    _ => [0, triangle + 1, triangle + 2][corner],
                };
                index_of(position) - lowest
            };
            let first = direct.map(|first| first + next);
            if floats == 12 {
                expand::<12>(out, first, &mut next, &mut table, &mut known, lowest, index_at, &read);
            } else {
                expand::<20>(out, first, &mut next, &mut table, &mut known, lowest, index_at, &read);
            }
        };
        // Geometría retenida: si todo sale de objetos de búfer que no
        // cambian (`GL_STATIC_DRAW`), los vértices se leen una sola vez.
        let retain_key = (|| {
            let mut arrays = [(0, 0, 0, 0, 0); ARRAYS];
            let used = [true, self.lighting, const_color.is_none(), true, units > 0, units > 0];
            for (i, array) in offsets.iter().enumerate() {
                if !used[i] || !array.enabled {
                    continue;
                }
                if self.buffers.get(&array.buffer)?.usage != GL_STATIC_DRAW {
                    return None;
                }
                arrays[i] = (array.buffer, array.pointer as usize, array.size, array.kind, array.stride);
            }
            let (index_buffer, index_kind, start) = match (first, source) {
                (Some(first), _) => (0, 0, first),
                (None, Some((buffer, kind, offset))) if self.buffers.get(&buffer)?.usage == GL_STATIC_DRAW => (buffer, kind, offset),
                _ => return None,
            };
            Some(RetainKey { mode, count, start, index_buffer, index_kind, arrays, lighting: self.lighting, const_color: const_color.is_some(), units: units as u8 })
        })();
        let mut inputs: RetainInputs = [0.0; 23];
        if self.lighting && !self.arrays[ARRAY_NORMAL].enabled {
            inputs[..3].copy_from_slice(&self.normal);
        }
        if !self.arrays[ARRAY_TEXCOORD].enabled {
            inputs[3..7].copy_from_slice(&self.texcoord);
        }
        inputs[7..].copy_from_slice(&frame.texture);
        // El recorte en x e y lo hace el rasterizador con su rectángulo de
        // recorte: debe ser, como mucho, el viewport.
        let (_, height) = self.raster.size();
        let [vx, vy, vw, vh] = self.viewport;
        let window = state.scissor.unwrap_or([0, 0, i32::MAX, i32::MAX]);
        let (x0, y0) = (vx.max(window[0]), (height as i32 - (vy + vh)).max(window[1]));
        let (x1, y1) = ((vx + vw).min(window[2]), (height as i32 - vy).min(window[3]));
        let clipped = raster::State { scissor: Some([x0, y0, x1.max(x0), y1.max(y0)]), ..*state };
        self.raster.set_state(&clipped);
        if let Some(key) = retain_key {
            self.retained_clock += 1;
            let clock = self.retained_clock;
            // ¿Está ya retenida, con las mismas entradas?
            let mut id = None;
            let mut rebuilds = 0;
            if let Some(entry) = self.retained.get_mut(&key) {
                if entry.id.is_some() && entry.inputs == inputs {
                    entry.last_used = clock;
                    id = entry.id;
                } else if entry.inputs != inputs {
                    rebuilds = entry.rebuilds.saturating_add(1);
                } else {
                    // No cabía o se soltó: no cuenta como cambio.
                    rebuilds = entry.rebuilds;
                }
            }
            // Si sus entradas cambian a cada momento, no compensa retenerla.
            let bytes = total * floats * 4;
            if id.is_none() && rebuilds <= 3 {
                self.forget(&key);
                if self.make_room(bytes) {
                    id = self.raster.fast_retain(units, total, &mut fill);
                }
                if id.is_some() {
                    self.retained_bytes += bytes;
                }
                if id.is_some() || rebuilds > 0 {
                    self.retained.insert(key, Retained { id, inputs, bytes, last_used: clock, rebuilds });
                }
            } else if id.is_none() {
                if let Some(entry) = self.retained.get_mut(&key) {
                    if let Some(old) = entry.id.take() {
                        self.raster.fast_release(old);
                        self.retained_bytes -= entry.bytes;
                    }
                }
            }
            if let Some(id) = id {
                if self.raster.fast_retained(&setup, id) {
                    self.retained_triangles += (total / 3) as u64;
                    return true;
                }
                self.raster.set_state(state);
                return false;
            }
        }
        if self.raster.fast_triangles(&setup, total, &mut fill) {
            return true;
        }
        self.raster.set_state(state);
        false
    }

    /// Dibuja `count` vértices cuyos índices da `index_of`. `first` es el
    /// primer índice cuando van seguidos (`glDrawArrays`).
    /// `source` dice de qué objeto de búfer salen los índices, si salen de
    /// uno: (búfer, tipo, desplazamiento).
    pub unsafe fn draw(&mut self, mode: GLenum, count: usize, first: Option<usize>, index_of: &dyn Fn(usize) -> usize, source: Option<(GLuint, GLenum, usize)>) {
        // Los arrays que viven en objetos de búfer guardan un desplazamiento:
        // mientras se dibuja, se cambia por el puntero de verdad.
        let offsets = self.arrays;
        for array in self.arrays.iter_mut().filter(|a| a.buffer != 0) {
            array.pointer = match self.buffers.get(&array.buffer) {
                Some(buffer) if (array.pointer as usize) < buffer.data.len() => buffer.data.as_ptr().add(array.pointer as usize),
                _ => core::ptr::null(),
            };
        }
        self.draw_arrays(mode, count, first, index_of, source, &offsets);
        self.arrays = offsets;
    }

    unsafe fn draw_arrays(&mut self, mode: GLenum, count: usize, first: Option<usize>, index_of: &dyn Fn(usize) -> usize, source: Option<(GLuint, GLenum, usize)>, offsets: &[Array; ARRAYS]) {
        if !self.arrays[ARRAY_VERTEX].enabled || count == 0 || self.arrays.iter().any(|a| a.enabled && a.pointer.is_null()) {
            return;
        }
        let state = self.raster_state();
        self.raster.set_state(&state);
        let frame = self.frame();
        if self.draw_fast(mode, count, first, index_of, source, offsets, &frame, &state) {
            return;
        }
        // Con índices, cada vértice se transforma e ilumina una sola vez.
        let indices: Vec<usize> = (0..count).map(index_of).collect();
        let lowest = indices.iter().copied().min().unwrap_or(0);
        let highest = indices.iter().copied().max().unwrap_or(0);
        let mut cache: Vec<Option<ClipVertex>> = alloc::vec![None; highest - lowest + 1];
        let mut v: Vec<ClipVertex> = Vec::with_capacity(count);
        for &index in &indices {
            let slot = &mut cache[index - lowest];
            if slot.is_none() {
                *slot = Some(self.process(&frame, index));
            }
            v.push(slot.unwrap());
        }
        match mode {
            GL_POINTS => {
                for p in &v {
                    self.draw_point(p);
                }
            }
            GL_LINES => {
                for pair in v.chunks_exact(2) {
                    self.draw_line(&pair[0], &pair[1]);
                }
            }
            GL_LINE_STRIP | GL_LINE_LOOP => {
                for i in 1..count {
                    self.draw_line(&v[i - 1], &v[i]);
                }
                if mode == GL_LINE_LOOP && count > 2 {
                    self.draw_line(&v[count - 1], &v[0]);
                }
            }
            GL_TRIANGLES => {
                for t in v.chunks_exact(3) {
                    self.draw_triangle(&t[0], &t[1], &t[2]);
                }
            }
            GL_TRIANGLE_STRIP => {
                for i in 2..count {
                    // Los triángulos impares van al revés para conservar la orientación.
                    if i % 2 == 0 {
                        self.draw_triangle(&v[i - 2], &v[i - 1], &v[i]);
                    } else {
                        self.draw_triangle(&v[i - 1], &v[i - 2], &v[i]);
                    }
                }
            }
            GL_TRIANGLE_FAN => {
                for i in 2..count {
                    self.draw_triangle(&v[0], &v[i - 1], &v[i]);
                }
            }
            _ => {}
        }
    }
}

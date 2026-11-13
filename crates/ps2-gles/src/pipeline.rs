//! Del array de vértices al rasterizador: lectura de atributos,
//! transformación, iluminación, niebla, ensamblado de primitivas, recorte
//! contra el volumen de visión, descarte de caras y paso a coordenadas de
//! ventana.

use crate::consts::*;
use crate::context::*;
use crate::math::{self, clamp01, Mat4};
use crate::raster::Vertex;
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

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len2 = dot3(v, v);
    if len2 <= 0.0 {
        return v;
    }
    let k = 1.0 / math::sqrt(len2);
    [v[0] * k, v[1] * k, v[2] * k]
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

    /// Transforma e ilumina el vértice `index` de los arrays.
    unsafe fn process(&self, f: &Frame, index: usize) -> ClipVertex {
        let object = fetch(&self.arrays[ARRAY_VERTEX], index, false);
        let eye = math::transform(&f.modelview, object);
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
            let i = &f.inverse;
            let mut n = [
                n[0] * i[0] + n[1] * i[1] + n[2] * i[2],
                n[0] * i[4] + n[1] * i[5] + n[2] * i[6],
                n[0] * i[8] + n[1] * i[9] + n[2] * i[10],
            ];
            if self.rescale_normal {
                n = [n[0] * f.rescale, n[1] * f.rescale, n[2] * f.rescale];
            }
            if self.normalize {
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

    /// Dibuja `count` vértices cuyos índices da `index_of`.
    pub unsafe fn draw(&mut self, mode: GLenum, count: usize, index_of: &dyn Fn(usize) -> usize) {
        if !self.arrays[ARRAY_VERTEX].enabled || self.arrays[ARRAY_VERTEX].pointer.is_null() || count == 0 {
            return;
        }
        let state = self.raster_state();
        self.raster.set_state(&state);
        let frame = self.frame();
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

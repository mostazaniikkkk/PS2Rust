//! OpenGL ES 1.0 en una PlayStation 2: un cubo texturado con iluminación y
//! brillo especular sobre un suelo de baldosas con mipmaps que se pierde en
//! la niebla, con cristales translúcidos girando alrededor y tres tallos que
//! se mecen (una malla animada con paleta de matrices).
//!
//! Todo se dibuja con las funciones estándar de GLES 1.0 y EGL del crate
//! `ps2-gles`; debajo, la unidad vectorial VU1 transforma, ilumina y recorta
//! los triángulos y el Graphics Synthesizer rasteriza. El cubo, que lleva
//! brillo especular, es lo único que calcula el Emotion Engine.
//!
//! El suelo, los cristales y los tallos están en objetos de búfer
//! (`GL_STATIC_DRAW`): sus vértices se leen una sola vez y después sólo
//! viajan a la VU1 las matrices. Los tallos se doblan en la propia VU1
//! (`GL_OES_matrix_palette`).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::ptr::{null, null_mut};
use ps2::println;
use ps2_gles::*;

ps2::entry!(main);

const FRAMES: u32 = 300;

/// Las seis caras del cubo: normal y dos ejes del plano de la cara.
const FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
    ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
    ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
    ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
    ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
];

/// Vértice intercalado: posición, normal y coordenadas de textura.
#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

/// Vértice de una malla animada: además, dos matrices de la paleta y sus pesos.
#[repr(C)]
#[derive(Clone, Copy)]
struct Skinned {
    position: [f32; 3],
    normal: [f32; 3],
    weights: [f32; 2],
    matrices: [u8; 4],
}

/// Huesos de un tallo y altura de cada uno.
const BONES: usize = 4;
const BONE_LENGTH: f32 = 0.8;

/// Un tallo: un cilindro que se estrecha, con cuatro huesos a lo largo.
fn stalk() -> (Vec<Skinned>, Vec<u16>) {
    let (rings, sides) = (16usize, 10usize);
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    for ring in 0..=rings {
        let along = ring as f32 / rings as f32;
        let y = along * BONES as f32 * BONE_LENGTH;
        let radius = 0.3 * (1.0 - 0.92 * along);
        // Cada tramo pasa poco a poco del hueso de abajo al de arriba.
        let bone = ((y / BONE_LENGTH) as usize).min(BONES - 1);
        let blend = if bone == BONES - 1 { 0.0 } else { y / BONE_LENGTH - bone as f32 };
        let upper = (bone + 1).min(BONES - 1);
        for side in 0..sides {
            let angle = side as f32 / sides as f32 * 2.0 * core::f32::consts::PI;
            let (sin, cos) = (ps2::math::sin(angle), ps2::math::cos(angle));
            vertices.push(Skinned { position: [radius * cos, y, radius * sin], normal: [cos, 0.0, sin], weights: [1.0 - blend, blend], matrices: [bone as u8, upper as u8, 0, 0] });
        }
    }
    for ring in 0..rings {
        for side in 0..sides {
            let a = (ring * sides + side) as u16;
            let b = (ring * sides + (side + 1) % sides) as u16;
            let (c, d) = (a + sides as u16, b + sides as u16);
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    (vertices, indices)
}

/// Crea un objeto de búfer estático con `data` y lo deja enlazado.
unsafe fn static_buffer<T>(target: GLenum, data: &[T]) -> GLuint {
    let mut name = 0;
    glGenBuffers(1, &mut name);
    glBindBuffer(target, name);
    glBufferData(target, core::mem::size_of_val(data) as GLsizeiptr, data.as_ptr().cast(), GL_STATIC_DRAW);
    name
}

/// Desplazamiento dentro del búfer enlazado, como puntero.
fn offset(bytes: usize) -> *const GLvoid {
    bytes as *const GLvoid
}

fn cube() -> (Vec<Vertex>, Vec<u8>) {
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    for (normal, right, up) in FACES {
        let base = vertices.len() as u8;
        for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let (a, b) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
            let position = [normal[0] + right[0] * a + up[0] * b, normal[1] + right[1] * a + up[1] * b, normal[2] + right[2] * a + up[2] * b];
            vertices.push(Vertex { position, normal, uv: [u, v] });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (vertices, indices)
}

/// Sube una textura RGB cuadrada con toda su pirámide de mipmaps (cada nivel
/// es la media de 2×2 del anterior).
unsafe fn upload_with_mipmaps(size: usize, mut pixels: Vec<u8>) -> GLuint {
    let mut name = 0;
    glGenTextures(1, &mut name);
    glBindTexture(GL_TEXTURE_2D, name);
    glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
    let (mut size, mut level) = (size, 0);
    loop {
        glTexImage2D(GL_TEXTURE_2D, level, GL_RGB as GLint, size as i32, size as i32, 0, GL_RGB, GL_UNSIGNED_BYTE, pixels.as_ptr().cast());
        if size == 1 {
            break;
        }
        let half = size / 2;
        let mut next = Vec::with_capacity(half * half * 3);
        for y in 0..half {
            for x in 0..half {
                for c in 0..3 {
                    let at = |dx: usize, dy: usize| pixels[((y * 2 + dy) * size + x * 2 + dx) * 3 + c] as u32;
                    next.push(((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8);
                }
            }
        }
        pixels = next;
        size = half;
        level += 1;
    }
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR_MIPMAP_LINEAR as f32);
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR as f32);
    name
}

/// Baldosas de dos tonos con junta oscura.
fn tiles(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size * size * 3);
    for y in 0..size {
        for x in 0..size {
            let half = size / 2;
            let joint = x % half == 0 || y % half == 0;
            let light = (x / half + y / half) % 2 == 0;
            out.extend_from_slice(&if joint {
                [40, 44, 52]
            } else if light {
                [210, 214, 222]
            } else {
                [96, 120, 168]
            });
        }
    }
    out
}

/// Caja de madera: tablones con un marco más oscuro.
fn crate_texture(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size * size * 3);
    for y in 0..size {
        for x in 0..size {
            let border = x < 6 || y < 6 || x >= size - 6 || y >= size - 6;
            let diagonal = (x as i32 - y as i32).abs() < 4;
            let plank = (y / 8) % 2 == 0;
            let grain = ((x * 7 + y * 13) % 11) as u8;
            let base: [u8; 3] = if border || diagonal {
                [120, 78, 40]
            } else if plank {
                [200, 150, 90]
            } else {
                [184, 134, 78]
            };
            out.extend_from_slice(&[base[0] + grain, base[1] + grain, base[2] + grain / 2]);
        }
    }
    out
}

fn main() {
    unsafe {
        // Arranque estándar con EGL.
        let display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
        eglInitialize(display, null_mut(), null_mut());
        let attributes = [EGL_RED_SIZE, 8, EGL_DEPTH_SIZE, 16, EGL_NONE];
        let (mut config, mut count) = (0, 0);
        eglChooseConfig(display, attributes.as_ptr(), &mut config, 1, &mut count);
        let surface = eglCreateWindowSurface(display, config, PS2_WINDOW_NTSC, null());
        let context = eglCreateContext(display, config, EGL_NO_CONTEXT, null());
        eglMakeCurrent(display, surface, surface, context);
        let (mut width, mut height) = (0, 0);
        eglQuerySurface(display, surface, EGL_WIDTH, &mut width);
        eglQuerySurface(display, surface, EGL_HEIGHT, &mut height);
        let text = |name| core::ffi::CStr::from_ptr(glGetString(name).cast()).to_str().unwrap_or("?");
        println!("gles-demo: {} en {} ({}x{})", text(GL_VERSION), text(GL_RENDERER), width, height);

        let floor_texture = upload_with_mipmaps(64, tiles(64));
        let crate_tex = upload_with_mipmaps(64, crate_texture(64));
        let (cube_vertices, cube_indices) = cube();
        // Suelo: una rejilla de 12×12 cuadros (x, z, s, t). El nivel de mipmap
        // se elige por triángulo, así que conviene que no sean enormes.
        let (cells, extent) = (12usize, 36.0f32);
        let mut floor: Vec<[f32; 5]> = Vec::new();
        for z in 0..=cells {
            for x in 0..=cells {
                let (fx, fz) = (x as f32 / cells as f32, z as f32 / cells as f32);
                floor.push([(fx - 0.5) * 2.0 * extent, -1.6, (0.5 - fz) * 2.0 * extent, fx * extent, fz * extent]);
            }
        }
        let mut floor_indices: Vec<u8> = Vec::new();
        for z in 0..cells {
            for x in 0..cells {
                let i = (z * (cells + 1) + x) as u8;
                let next = i + cells as u8 + 1;
                floor_indices.extend_from_slice(&[i, i + 1, next + 1, i, next + 1, next]);
            }
        }

        // Proyección en perspectiva. Los píxeles de 640×448 en una tele 4:3
        // no son cuadrados: la relación de aspecto es la de la pantalla.
        glMatrixMode(GL_PROJECTION);
        glLoadIdentity();
        let near = 0.5;
        glFrustumf(-near * 0.55 * 4.0 / 3.0, near * 0.55 * 4.0 / 3.0, -near * 0.55, near * 0.55, near, 60.0);
        glMatrixMode(GL_MODELVIEW);

        let sky = [0.55f32, 0.68, 0.86, 1.0];
        glClearColor(sky[0], sky[1], sky[2], sky[3]);
        glEnable(GL_DEPTH_TEST);
        glEnable(GL_CULL_FACE);
        glFogf(GL_FOG_MODE, GL_LINEAR as f32);
        glFogf(GL_FOG_START, 6.0);
        glFogf(GL_FOG_END, 32.0);
        glFogfv(GL_FOG_COLOR, sky.as_ptr());
        glLightfv(GL_LIGHT0, GL_DIFFUSE, [1.0f32, 0.96, 0.9, 1.0].as_ptr());
        glLightfv(GL_LIGHT0, GL_SPECULAR, [1.0f32, 1.0, 1.0, 1.0].as_ptr());
        glLightfv(GL_LIGHT0, GL_AMBIENT, [0.25f32, 0.27, 0.32, 1.0].as_ptr());
        glEnable(GL_LIGHT0);
        glEnable(GL_NORMALIZE);
        glEnableClientState(GL_VERTEX_ARRAY);

        // Antes de la escena, una medida de la geometría: una malla de 2048
        // triángulos iluminados, dibujada diez veces sin presentar.
        let side = 32usize;
        let mut mesh: Vec<[f32; 6]> = Vec::new();
        for y in 0..=side {
            for x in 0..=side {
                let (fx, fy) = (x as f32 / side as f32 - 0.5, y as f32 / side as f32 - 0.5);
                mesh.push([fx * 4.0, fy * 3.0, ps2::math::sin(fx * 9.0) * 0.3, fx, fy, 1.0]);
            }
        }
        let mut mesh_indices: Vec<u16> = Vec::new();
        for y in 0..side {
            for x in 0..side {
                let i = (y * (side + 1) + x) as u16;
                let below = i + side as u16 + 1;
                mesh_indices.extend_from_slice(&[i, i + 1, below + 1, i, below + 1, below]);
            }
        }
        // La misma malla con dos matrices por vértice, para medir la paleta.
        let mesh_skin: Vec<([f32; 2], [u8; 2])> = mesh.iter().map(|v| ([0.5 - v[3], 0.5 + v[3]], [0, 1])).collect();
        glLoadIdentity();
        glTranslatef(0.0, 0.0, -6.0);
        glEnable(GL_LIGHTING);
        glEnableClientState(GL_NORMAL_ARRAY);
        let rounds = 10;
        let triangles = (rounds * mesh_indices.len() / 3) as u64;
        // Dibuja la malla diez veces con los índices en `indices` y devuelve
        // los triángulos por segundo.
        let measure = |indices: *const GLvoid| -> u64 {
            glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
            let before = ps2::time::micros();
            for _ in 0..rounds {
                glDrawElements(GL_TRIANGLES, mesh_indices.len() as i32, GL_UNSIGNED_SHORT, indices);
            }
            glFinish();
            triangles * 1_000_000 / (ps2::time::micros() - before).max(1)
        };
        // 1. Con los vértices en memoria del programa: se leen en cada dibujo.
        glVertexPointer(3, GL_FLOAT, 24, mesh.as_ptr().cast());
        glNormalPointer(GL_FLOAT, 24, (mesh.as_ptr() as *const u8).add(12).cast());
        let rate = measure(mesh_indices.as_ptr().cast());
        println!("gles-demo: {} triangulos iluminados por segundo ({} por frame a 60 Hz)", rate, rate / 60);
        // 2. En objetos de búfer estáticos: se leen una vez (aquí, antes de
        //    medir) y después sólo se reenvían.
        let bench_buffers = [static_buffer(GL_ARRAY_BUFFER, &mesh), static_buffer(GL_ELEMENT_ARRAY_BUFFER, &mesh_indices), static_buffer(GL_ARRAY_BUFFER, &mesh_skin)];
        glBindBuffer(GL_ARRAY_BUFFER, bench_buffers[0]);
        glVertexPointer(3, GL_FLOAT, 24, offset(0));
        glNormalPointer(GL_FLOAT, 24, offset(12));
        glDrawElements(GL_TRIANGLES, mesh_indices.len() as i32, GL_UNSIGNED_SHORT, offset(0));
        let rate = measure(offset(0));
        println!("gles-demo: {} por segundo desde bufer estatico ({} por frame a 60 Hz)", rate, rate / 60);
        // 3. Y además con dos matrices de la paleta por vértice.
        glBindBuffer(GL_ARRAY_BUFFER, bench_buffers[2]);
        glWeightPointerOES(2, GL_FLOAT, 12, offset(0));
        glMatrixIndexPointerOES(2, GL_UNSIGNED_BYTE, 12, offset(8));
        glEnableClientState(GL_WEIGHT_ARRAY_OES);
        glEnableClientState(GL_MATRIX_INDEX_ARRAY_OES);
        glEnable(GL_MATRIX_PALETTE_OES);
        glCurrentPaletteMatrixOES(0);
        glLoadPaletteFromModelViewMatrixOES();
        glRotatef(25.0, 0.0, 1.0, 0.0);
        glCurrentPaletteMatrixOES(1);
        glLoadPaletteFromModelViewMatrixOES();
        glDrawElements(GL_TRIANGLES, mesh_indices.len() as i32, GL_UNSIGNED_SHORT, offset(0));
        let rate = measure(offset(0));
        println!("gles-demo: {} por segundo con paleta de matrices ({} por frame a 60 Hz)", rate, rate / 60);
        glDisable(GL_MATRIX_PALETTE_OES);
        glDisableClientState(GL_WEIGHT_ARRAY_OES);
        glDisableClientState(GL_MATRIX_INDEX_ARRAY_OES);
        glDisableClientState(GL_NORMAL_ARRAY);
        glDisable(GL_LIGHTING);
        glBindBuffer(GL_ARRAY_BUFFER, 0);
        glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, 0);
        glDeleteBuffers(3, bench_buffers.as_ptr());

        // La escena: suelo, cristales y tallos, en objetos de búfer.
        let floor_buffer = static_buffer(GL_ARRAY_BUFFER, &floor);
        let floor_index_buffer = static_buffer(GL_ELEMENT_ARRAY_BUFFER, &floor_indices);
        let pane = [-0.7f32, -0.9, 0.0, 0.7, -0.9, 0.0, 0.7, 0.9, 0.0, -0.7, 0.9, 0.0];
        let pane_buffer = static_buffer(GL_ARRAY_BUFFER, &pane);
        let (stalk_vertices, stalk_indices) = stalk();
        let stalk_buffer = static_buffer(GL_ARRAY_BUFFER, &stalk_vertices);
        let stalk_index_buffer = static_buffer(GL_ELEMENT_ARRAY_BUFFER, &stalk_indices);
        glBindBuffer(GL_ARRAY_BUFFER, 0);
        glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, 0);

        let start = ps2::time::micros();
        // Tras los frames medidos, la escena sigue unos segundos en pantalla.
        for frame in 0..FRAMES + 240 {
            if frame == FRAMES {
                let elapsed = (ps2::time::micros() - start) / 1000;
                println!("gles-demo: {} triangulos por la VU1", fast_triangles());
                println!("gles-demo: {} de ellos desde geometria retenida ({} KiB)", retained_triangles(), retained_bytes() / 1024);
                println!("gles-demo: {} frames en {} ms, glGetError() = {:#x}", FRAMES, elapsed, glGetError());
            }
            let time = frame as f32 / 60.0;
            glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
            glLoadIdentity();
            // Cámara: algo elevada, mirando al cubo.
            glTranslatef(0.0, -0.4, -7.5);
            glRotatef(18.0, 1.0, 0.0, 0.0);
            glRotatef(time * 12.0, 0.0, 1.0, 0.0);
            // La luz se coloca en coordenadas del mundo, tras la cámara.
            glLightfv(GL_LIGHT0, GL_POSITION, [0.5f32, 1.0, 0.6, 0.0].as_ptr());

            // Suelo: la textura de baldosas repetida, con niebla.
            glEnable(GL_FOG);
            glEnable(GL_TEXTURE_2D);
            glBindTexture(GL_TEXTURE_2D, floor_texture);
            glTexEnvf(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, GL_MODULATE as f32);
            glColor4f(1.0, 1.0, 1.0, 1.0);
            glBindBuffer(GL_ARRAY_BUFFER, floor_buffer);
            glVertexPointer(3, GL_FLOAT, 20, offset(0));
            glEnableClientState(GL_TEXTURE_COORD_ARRAY);
            glTexCoordPointer(2, GL_FLOAT, 20, offset(12));
            glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, floor_index_buffer);
            glDrawElements(GL_TRIANGLES, floor_indices.len() as i32, GL_UNSIGNED_BYTE, offset(0));
            glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, 0);
            glBindBuffer(GL_ARRAY_BUFFER, 0);

            // Cubo: texturado, iluminado y con brillo.
            glEnable(GL_LIGHTING);
            glMaterialfv(GL_FRONT_AND_BACK, GL_AMBIENT_AND_DIFFUSE, [1.0f32, 1.0, 1.0, 1.0].as_ptr());
            glMaterialfv(GL_FRONT_AND_BACK, GL_SPECULAR, [0.35f32, 0.35, 0.35, 1.0].as_ptr());
            glMaterialf(GL_FRONT_AND_BACK, GL_SHININESS, 24.0);
            glBindTexture(GL_TEXTURE_2D, crate_tex);
            glPushMatrix();
            glTranslatef(0.0, 0.25 * ps2::math::sin(time * 1.7), 0.0);
            glRotatef(time * 47.0, 0.0, 1.0, 0.0);
            glRotatef(time * 31.0, 1.0, 0.0, 0.3);
            let stride = core::mem::size_of::<Vertex>() as i32;
            let base = cube_vertices.as_ptr() as *const u8;
            glVertexPointer(3, GL_FLOAT, stride, base.cast());
            glEnableClientState(GL_NORMAL_ARRAY);
            glNormalPointer(GL_FLOAT, stride, base.add(12).cast());
            glTexCoordPointer(2, GL_FLOAT, stride, base.add(24).cast());
            glDrawElements(GL_TRIANGLES, cube_indices.len() as i32, GL_UNSIGNED_BYTE, cube_indices.as_ptr().cast());
            glDisableClientState(GL_TEXTURE_COORD_ARRAY);
            glPopMatrix();
            glDisable(GL_TEXTURE_2D);

            // Tallos: la misma malla tres veces, cada una con su pose. La
            // pose son cuatro matrices (los huesos, encadenados) que se
            // cargan en la paleta; los vértices no se tocan.
            glMaterialfv(GL_FRONT_AND_BACK, GL_AMBIENT_AND_DIFFUSE, [0.35f32, 0.75, 0.3, 1.0].as_ptr());
            glMaterialfv(GL_FRONT_AND_BACK, GL_SPECULAR, [0.0f32, 0.0, 0.0, 1.0].as_ptr());
            let stride = core::mem::size_of::<Skinned>() as i32;
            glBindBuffer(GL_ARRAY_BUFFER, stalk_buffer);
            glVertexPointer(3, GL_FLOAT, stride, offset(0));
            glNormalPointer(GL_FLOAT, stride, offset(12));
            glWeightPointerOES(2, GL_FLOAT, stride, offset(24));
            glMatrixIndexPointerOES(2, GL_UNSIGNED_BYTE, stride, offset(32));
            glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, stalk_index_buffer);
            glEnableClientState(GL_WEIGHT_ARRAY_OES);
            glEnableClientState(GL_MATRIX_INDEX_ARRAY_OES);
            glEnable(GL_MATRIX_PALETTE_OES);
            for (i, place) in [[-4.2f32, -2.0], [4.0, -2.6], [0.5, 4.6]].iter().enumerate() {
                glPushMatrix();
                glTranslatef(place[0], -1.6, place[1]);
                for bone in 0..BONES {
                    let sway = ps2::math::sin(time * 1.8 + bone as f32 * 0.7 + i as f32 * 2.1);
                    glRotatef(16.0 * sway, 0.0, 0.0, 1.0);
                    glRotatef(9.0 * ps2::math::sin(time * 1.1 + i as f32), 1.0, 0.0, 0.0);
                    // Matriz del hueso: su giro, deshaciendo antes la altura
                    // a la que está en la malla en reposo.
                    glPushMatrix();
                    glTranslatef(0.0, -(bone as f32) * BONE_LENGTH, 0.0);
                    glCurrentPaletteMatrixOES(bone as GLuint);
                    glLoadPaletteFromModelViewMatrixOES();
                    glPopMatrix();
                    glTranslatef(0.0, BONE_LENGTH, 0.0);
                }
                glPopMatrix();
                glDrawElements(GL_TRIANGLES, stalk_indices.len() as i32, GL_UNSIGNED_SHORT, offset(0));
            }
            glDisable(GL_MATRIX_PALETTE_OES);
            glDisableClientState(GL_WEIGHT_ARRAY_OES);
            glDisableClientState(GL_MATRIX_INDEX_ARRAY_OES);
            glDisableClientState(GL_NORMAL_ARRAY);
            glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, 0);
            glDisable(GL_LIGHTING);

            // Cristales translúcidos en órbita: mezcla alfa, sin escribir
            // profundidad y visibles por las dos caras.
            glEnable(GL_BLEND);
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glDepthMask(GL_FALSE);
            glDisable(GL_CULL_FACE);
            glBindBuffer(GL_ARRAY_BUFFER, pane_buffer);
            glVertexPointer(3, GL_FLOAT, 0, offset(0));
            glBindBuffer(GL_ARRAY_BUFFER, 0);
            let colors: [[f32; 4]; 3] = [[1.0, 0.3, 0.25, 0.45], [0.3, 1.0, 0.45, 0.45], [0.35, 0.5, 1.0, 0.45]];
            for (i, color) in colors.iter().enumerate() {
                glPushMatrix();
                glRotatef(time * 40.0 + i as f32 * 120.0, 0.0, 1.0, 0.0);
                glTranslatef(3.0, 0.0, 0.0);
                glRotatef(time * 90.0, 0.0, 1.0, 0.0);
                glColor4f(color[0], color[1], color[2], color[3]);
                glDrawArrays(GL_TRIANGLE_FAN, 0, 4);
                glPopMatrix();
            }
            glEnable(GL_CULL_FACE);
            glDepthMask(GL_TRUE);
            glDisable(GL_BLEND);
            glDisable(GL_FOG);

            eglSwapBuffers(display, surface);
            if frame == 0 {
                println!("gles-demo: primer frame presentado, glGetError() = {:#x}", glGetError());
            }
        }
    }
}

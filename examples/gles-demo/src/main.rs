//! OpenGL ES 1.0 en una PlayStation 2: un cubo texturado con iluminación y
//! brillo especular sobre un suelo de baldosas con mipmaps que se pierde en
//! la niebla, con cristales translúcidos girando alrededor.
//!
//! Todo se dibuja con las funciones estándar de GLES 1.0 y EGL del crate
//! `ps2-gles`; debajo, el Emotion Engine transforma e ilumina y el Graphics
//! Synthesizer rasteriza.

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
        glLoadIdentity();
        glTranslatef(0.0, 0.0, -6.0);
        glEnable(GL_LIGHTING);
        glEnableClientState(GL_NORMAL_ARRAY);
        glVertexPointer(3, GL_FLOAT, 24, mesh.as_ptr().cast());
        glNormalPointer(GL_FLOAT, 24, (mesh.as_ptr() as *const u8).add(12).cast());
        glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
        let before = ps2::time::micros();
        let rounds = 10;
        for _ in 0..rounds {
            glDrawElements(GL_TRIANGLES, mesh_indices.len() as i32, GL_UNSIGNED_SHORT, mesh_indices.as_ptr().cast());
        }
        glFinish();
        let micros = (ps2::time::micros() - before).max(1);
        let triangles = (rounds * mesh_indices.len() / 3) as u64;
        println!("gles-demo: {} triangulos iluminados por segundo ({} por frame a 60 Hz)", triangles * 1_000_000 / micros, triangles * 1_000_000 / micros / 60);
        glDisableClientState(GL_NORMAL_ARRAY);
        glDisable(GL_LIGHTING);

        let start = ps2::time::micros();
        // Tras los frames medidos, la escena sigue unos segundos en pantalla.
        for frame in 0..FRAMES + 240 {
            if frame == FRAMES {
                let elapsed = (ps2::time::micros() - start) / 1000;
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
            glVertexPointer(3, GL_FLOAT, 20, floor.as_ptr().cast());
            glEnableClientState(GL_TEXTURE_COORD_ARRAY);
            glTexCoordPointer(2, GL_FLOAT, 20, (floor.as_ptr() as *const u8).add(12).cast());
            glDrawElements(GL_TRIANGLES, floor_indices.len() as i32, GL_UNSIGNED_BYTE, floor_indices.as_ptr().cast());

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
            glDisableClientState(GL_NORMAL_ARRAY);
            glDisableClientState(GL_TEXTURE_COORD_ARRAY);
            glPopMatrix();
            glDisable(GL_LIGHTING);
            glDisable(GL_TEXTURE_2D);

            // Cristales translúcidos en órbita: mezcla alfa, sin escribir
            // profundidad y visibles por las dos caras.
            glEnable(GL_BLEND);
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glDepthMask(GL_FALSE);
            glDisable(GL_CULL_FACE);
            let pane = [-0.7f32, -0.9, 0.0, 0.7, -0.9, 0.0, 0.7, 0.9, 0.0, -0.7, 0.9, 0.0];
            glVertexPointer(3, GL_FLOAT, 0, pane.as_ptr().cast());
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

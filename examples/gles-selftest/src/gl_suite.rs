//! Batería de pruebas de OpenGL ES 1.0.
//!
//! Dibuja con las funciones de GL, lee el resultado con `glReadPixels` y lo
//! compara con lo que manda la especificación. Este mismo archivo se compila
//! para la PlayStation 2 (rasteriza el Graphics Synthesizer) y para el PC
//! (rasteriza la referencia por software): las dos ejecuciones deben imprimir
//! exactamente lo mismo.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;
use core::ptr::{null, null_mut};
use ps2_gles::*;

const W: i32 = 640;
const H: i32 = 448;

struct T<'a> {
    out: &'a mut dyn Write,
    total: u32,
    failed: u32,
}

/// Lee un píxel (coordenadas de GL: origen abajo a la izquierda).
fn pixel(x: i32, y: i32) -> [u8; 4] {
    let mut p = [0u8; 4];
    unsafe { glReadPixels(x, y, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, p.as_mut_ptr().cast()) };
    p
}

impl T<'_> {
    fn section(&mut self, name: &str) {
        let _ = writeln!(self.out, "== {name}");
    }

    fn check(&mut self, name: &str, ok: bool) {
        self.total += 1;
        if !ok {
            self.failed += 1;
        }
        let _ = writeln!(self.out, "[{}] {}", if ok { "ok" } else { "FALLO" }, name);
    }

    /// El píxel (x, y) debe valer `expected` (R, G, B) con un margen por canal.
    fn rgb(&mut self, name: &str, x: i32, y: i32, expected: [u8; 3], tolerance: i32) {
        let p = pixel(x, y);
        let ok = (0..3).all(|i| (p[i] as i32 - expected[i] as i32).abs() <= tolerance);
        self.check(name, ok);
        if !ok {
            let _ = writeln!(self.out, "     en ({x}, {y}) hay {:?} y se esperaba {:?} (+-{tolerance})", &p[..3], expected);
        }
    }

    fn error(&mut self, name: &str, expected: GLenum) {
        let got = glGetError();
        self.check(name, got == expected);
        if got != expected {
            let _ = writeln!(self.out, "     glGetError() = {got:#06x}, se esperaba {expected:#06x}");
        }
    }
}

const BLACK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];
const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];

/// Proyección en la que las coordenadas son píxeles y z va de -1 a 1.
fn pixel_projection() {
    glMatrixMode(GL_PROJECTION);
    glLoadIdentity();
    glOrthof(0.0, W as f32, 0.0, H as f32, -1.0, 1.0);
    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();
}

fn clear_black() {
    glClearColor(0.0, 0.0, 0.0, 0.0);
    glClearDepthf(1.0);
    glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
}

/// Dibuja vértices (x, y, z) con el color actual.
fn draw(mode: GLenum, vertices: &[f32]) {
    unsafe {
        glEnableClientState(GL_VERTEX_ARRAY);
        glVertexPointer(3, GL_FLOAT, 0, vertices.as_ptr().cast());
        glDrawArrays(mode, 0, (vertices.len() / 3) as i32);
        if mode <= GL_LINE_STRIP {
            // Tras puntos y líneas, un punto más en la esquina inferior
            // derecha. PCSX2 (por hardware) sólo devuelve a `glReadPixels` la
            // zona que cree dibujada, y en puntos y líneas deja fuera su borde
            // derecho e inferior; este punto la amplía. En la referencia y en
            // lo que se ve en pantalla no cambia nada.
            let corner = [W as f32 - 0.5, 0.5, 0.0];
            glVertexPointer(3, GL_FLOAT, 0, corner.as_ptr().cast());
            glDrawArrays(GL_POINTS, 0, 1);
        }
    }
}

/// Rectángulo [x0, x1) × [y0, y1) a profundidad z, en sentido antihorario.
fn quad(x0: f32, y0: f32, x1: f32, y1: f32, z: f32) {
    draw(GL_TRIANGLE_FAN, &[x0, y0, z, x1, y0, z, x1, y1, z, x0, y1, z]);
}

/// El mismo rectángulo con coordenadas de textura de (0, 0) a (s, t).
fn textured_quad(x0: f32, y0: f32, x1: f32, y1: f32, s: f32, t: f32) {
    let coords = [0.0, 0.0, s, 0.0, s, t, 0.0, t];
    unsafe {
        glEnableClientState(GL_TEXTURE_COORD_ARRAY);
        glTexCoordPointer(2, GL_FLOAT, 0, coords.as_ptr().cast());
        quad(x0, y0, x1, y1, 0.0);
        glDisableClientState(GL_TEXTURE_COORD_ARRAY);
    }
}

fn texture(w: i32, h: i32, format: GLenum, kind: GLenum, data: &[u8], filter: GLenum) -> GLuint {
    let mut name = 0;
    unsafe {
        glGenTextures(1, &mut name);
        glBindTexture(GL_TEXTURE_2D, name);
        glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
        glTexImage2D(GL_TEXTURE_2D, 0, format as GLint, w, h, 0, format, kind, data.as_ptr().cast());
    }
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter as f32);
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter as f32);
    name
}

/// Ejecuta toda la batería y devuelve el número de comprobaciones fallidas.
/// Deja creado el contexto de GL, con el último frame presentado.
pub fn run(out: &mut dyn Write) -> u32 {
    let mut t = T { out, total: 0, failed: 0 };
    let (display, surface) = egl(&mut t);
    queries(&mut t);
    errors(&mut t);
    clearing(&mut t);
    coverage(&mut t);
    primitives(&mut t);
    vertex_formats(&mut t);
    matrices(&mut t);
    clipping(&mut t);
    depth(&mut t);
    culling(&mut t);
    blending(&mut t);
    fragment_tests(&mut t);
    texturing(&mut t);
    texture_environment(&mut t);
    mipmaps(&mut t);
    lighting(&mut t);
    fog(&mut t);
    reading(&mut t);
    fixed_point(&mut t);
    presenting(&mut t, display, surface);
    let (total, failed) = (t.total, t.failed);
    let _ = writeln!(t.out, "== resultado: {total} comprobaciones, {failed} fallos");
    failed
}

fn egl(t: &mut T) -> (EGLDisplay, EGLSurface) {
    t.section("EGL");
    unsafe {
        let display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
        t.check("eglGetDisplay", display != EGL_NO_DISPLAY);
        let (mut major, mut minor) = (0, 0);
        t.check("eglInitialize 1.0", eglInitialize(display, &mut major, &mut minor) == EGL_TRUE && (major, minor) == (1, 0));
        let mut count = 0;
        eglGetConfigs(display, null_mut(), 0, &mut count);
        t.check("eglGetConfigs: dos configuraciones", count == 2);
        let wanted = [EGL_RED_SIZE, 8, EGL_ALPHA_SIZE, 8, EGL_DEPTH_SIZE, 16, EGL_SURFACE_TYPE, EGL_WINDOW_BIT, EGL_NONE];
        let mut configs = [0; 4];
        eglChooseConfig(display, wanted.as_ptr(), configs.as_mut_ptr(), 4, &mut count);
        let mut depth_bits = 0;
        eglGetConfigAttrib(display, configs[0], EGL_DEPTH_SIZE, &mut depth_bits);
        t.check("eglChooseConfig: primero la de 16 bits de profundidad", count == 2 && depth_bits == 16);
        let too_much = [EGL_STENCIL_SIZE, 8, EGL_NONE];
        eglChooseConfig(display, too_much.as_ptr(), configs.as_mut_ptr(), 4, &mut count);
        t.check("eglChooseConfig: no hay stencil", count == 0);
        eglChooseConfig(display, wanted.as_ptr(), configs.as_mut_ptr(), 1, &mut count);
        t.check("ventana desconocida: EGL_BAD_NATIVE_WINDOW", eglCreateWindowSurface(display, configs[0], 99, null()) == EGL_NO_SURFACE && eglGetError() == EGL_BAD_NATIVE_WINDOW);
        let surface = eglCreateWindowSurface(display, configs[0], PS2_WINDOW_NTSC, null());
        let (mut w, mut h) = (0, 0);
        eglQuerySurface(display, surface, EGL_WIDTH, &mut w);
        eglQuerySurface(display, surface, EGL_HEIGHT, &mut h);
        t.check("superficie de 640x448", surface != EGL_NO_SURFACE && (w, h) == (W, H));
        let context = eglCreateContext(display, configs[0], EGL_NO_CONTEXT, null());
        t.check("eglCreateContext", context != EGL_NO_CONTEXT);
        t.check("sin contexto actual, GL no hace nada", glGetError() == GL_NO_ERROR && glGetString(GL_VERSION).is_null());
        t.check("eglMakeCurrent", eglMakeCurrent(display, surface, surface, context) == EGL_TRUE && eglGetCurrentContext() == context);
        t.check("eglGetError limpio", eglGetError() == EGL_SUCCESS);
        (display, surface)
    }
}

fn queries(t: &mut T) {
    t.section("consultas");
    let version = unsafe { core::ffi::CStr::from_ptr(glGetString(GL_VERSION).cast()) };
    t.check("GL_VERSION = OpenGL ES-CM 1.0", version.to_bytes() == b"OpenGL ES-CM 1.0");
    let int = |name: GLenum| {
        let mut v = [0; 16];
        unsafe { glGetIntegerv(name, v.as_mut_ptr()) };
        v[0]
    };
    t.check("GL_MAX_LIGHTS = 8", int(GL_MAX_LIGHTS) == 8);
    t.check("GL_MAX_TEXTURE_SIZE >= 64", int(GL_MAX_TEXTURE_SIZE) >= 64);
    t.check("GL_MAX_MODELVIEW_STACK_DEPTH >= 16", int(GL_MAX_MODELVIEW_STACK_DEPTH) >= 16);
    t.check("GL_DEPTH_BITS = 16, GL_STENCIL_BITS = 0", int(GL_DEPTH_BITS) == 16 && int(GL_STENCIL_BITS) == 0);
    t.check("10 formatos de textura con paleta", int(GL_NUM_COMPRESSED_TEXTURE_FORMATS) == 10 && int(GL_COMPRESSED_TEXTURE_FORMATS) == GL_PALETTE4_RGB8_OES as i32);
    t.error("consultas sin errores", GL_NO_ERROR);
}

fn errors(t: &mut T) {
    t.section("errores");
    glEnable(0xFFFF);
    t.error("glEnable desconocido: GL_INVALID_ENUM", GL_INVALID_ENUM);
    t.error("glGetError limpia el error", GL_NO_ERROR);
    glMatrixMode(GL_MODELVIEW);
    glPopMatrix();
    t.error("glPopMatrix en el fondo: GL_STACK_UNDERFLOW", GL_STACK_UNDERFLOW);
    glMatrixMode(GL_PROJECTION);
    glPushMatrix();
    glPushMatrix();
    t.error("pila de proyeccion llena: GL_STACK_OVERFLOW", GL_STACK_OVERFLOW);
    glPopMatrix();
    glMatrixMode(GL_MODELVIEW);
    glViewport(0, 0, -1, 10);
    glBlendFunc(GL_SRC_COLOR, GL_ZERO);
    t.error("se conserva el primer error (GL_INVALID_VALUE)", GL_INVALID_VALUE);
    glFrustumf(-1.0, 1.0, -1.0, 1.0, 0.0, 10.0);
    t.error("glFrustumf con near = 0: GL_INVALID_VALUE", GL_INVALID_VALUE);
    unsafe { glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA as GLint, 3, 4, 0, GL_RGBA, GL_UNSIGNED_BYTE, null()) };
    t.error("textura de 3x4: GL_INVALID_VALUE", GL_INVALID_VALUE);
    unsafe { glTexImage2D(GL_TEXTURE_2D, 0, GL_RGB as GLint, 4, 4, 0, GL_RGBA, GL_UNSIGNED_BYTE, null()) };
    t.error("formato interno distinto: GL_INVALID_OPERATION", GL_INVALID_OPERATION);
    unsafe { glMaterialfv(GL_FRONT, GL_DIFFUSE, [1.0f32; 4].as_ptr()) };
    t.error("glMaterial solo con GL_FRONT_AND_BACK", GL_INVALID_ENUM);
    glActiveTexture(GL_TEXTURE1);
    t.error("una sola unidad de textura", GL_INVALID_ENUM);
    unsafe { glDrawArrays(GL_TRIANGLES, 0, -3) };
    t.error("glDrawArrays con cuenta negativa", GL_INVALID_VALUE);
}

fn clearing(t: &mut T) {
    t.section("glClear");
    glClearColor(0.2, 0.4, 0.6, 1.0);
    glClear(GL_COLOR_BUFFER_BIT);
    t.rgb("color de fondo", 320, 224, [51, 102, 153], 1);
    t.rgb("esquinas", 0, 0, [51, 102, 153], 1);
    t.rgb("esquina opuesta", W - 1, H - 1, [51, 102, 153], 1);
    glEnable(GL_SCISSOR_TEST);
    glScissor(100, 50, 20, 10);
    glClearColor(1.0, 0.0, 0.0, 1.0);
    glClear(GL_COLOR_BUFFER_BIT);
    glDisable(GL_SCISSOR_TEST);
    t.rgb("recorte: dentro", 100, 50, RED, 1);
    t.rgb("recorte: ultimo pixel", 119, 59, RED, 1);
    t.rgb("recorte: fuera a la derecha", 120, 55, [51, 102, 153], 1);
    t.rgb("recorte: fuera arriba", 110, 60, [51, 102, 153], 1);
    glColorMask(GL_FALSE, GL_TRUE, GL_FALSE, GL_TRUE);
    glClearColor(1.0, 1.0, 1.0, 1.0);
    glClear(GL_COLOR_BUFFER_BIT);
    glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
    t.rgb("mascara de color en glClear", 320, 224, [51, 255, 153], 1);
    glClear(0x8000);
    t.error("bit desconocido: GL_INVALID_VALUE", GL_INVALID_VALUE);
}

fn coverage(t: &mut T) {
    t.section("relleno de triangulos");
    pixel_projection();
    clear_black();
    glColor4f(1.0, 0.5, 0.25, 1.0);
    quad(100.0, 100.0, 200.0, 200.0, 0.0);
    t.rgb("interior", 150, 150, [255, 128, 64], 1);
    t.rgb("primer pixel", 100, 100, [255, 128, 64], 1);
    t.rgb("ultimo pixel", 199, 199, [255, 128, 64], 1);
    t.rgb("fuera por la izquierda", 99, 150, BLACK, 0);
    t.rgb("fuera por la derecha", 200, 150, BLACK, 0);
    t.rgb("fuera por arriba", 150, 200, BLACK, 0);
    t.rgb("fuera por abajo", 150, 99, BLACK, 0);
    // Dos triangulos que comparten arista, sumando: ningun pixel se pinta dos veces.
    glEnable(GL_BLEND);
    glBlendFunc(GL_ONE, GL_ONE);
    glColor4f(0.25, 0.25, 0.25, 1.0);
    quad(300.0, 100.0, 364.0, 164.0, 0.0);
    glDisable(GL_BLEND);
    let diagonal_ok = (0..64).all(|i| pixel(300 + i, 100 + i)[0] == 64);
    t.check("la arista compartida se pinta una sola vez", diagonal_ok);
    // Colores interpolados y sombreado plano.
    let vertices = [400.0f32, 100.0, 0.0, 528.0, 100.0, 0.0, 528.0, 164.0, 0.0, 400.0, 164.0, 0.0];
    let colors: [u8; 16] = [255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255, 255, 0, 0, 255];
    unsafe {
        glEnableClientState(GL_COLOR_ARRAY);
        glColorPointer(4, GL_UNSIGNED_BYTE, 0, colors.as_ptr().cast());
        draw(GL_TRIANGLE_FAN, &vertices);
        t.rgb("Gouraud: a la izquierda", 404, 130, [248, 0, 7], 6);
        t.rgb("Gouraud: en el centro", 464, 130, [127, 0, 128], 6);
        t.rgb("Gouraud: a la derecha", 523, 130, [8, 0, 247], 6);
        glShadeModel(GL_FLAT);
        let flat: [u8; 12] = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255];
        glColorPointer(4, GL_UNSIGNED_BYTE, 0, flat.as_ptr().cast());
        draw(GL_TRIANGLES, &[400.0, 200.0, 0.0, 464.0, 200.0, 0.0, 400.0, 264.0, 0.0]);
        glShadeModel(GL_SMOOTH);
        glDisableClientState(GL_COLOR_ARRAY);
    }
    t.rgb("GL_FLAT: manda el ultimo vertice", 404, 204, BLUE, 1);
    t.rgb("GL_FLAT: en todo el triangulo", 450, 204, BLUE, 1);
}

fn primitives(t: &mut T) {
    t.section("primitivas");
    pixel_projection();
    clear_black();
    glColor4f(0.0, 1.0, 0.0, 1.0);
    draw(GL_TRIANGLE_STRIP, &[20.0, 20.0, 0.0, 60.0, 20.0, 0.0, 20.0, 60.0, 0.0, 60.0, 60.0, 0.0, 20.0, 100.0, 0.0, 60.0, 100.0, 0.0]);
    t.rgb("GL_TRIANGLE_STRIP: primer cuadro", 40, 40, GREEN, 1);
    t.rgb("GL_TRIANGLE_STRIP: segundo cuadro", 40, 80, GREEN, 1);
    draw(GL_TRIANGLE_FAN, &[120.0, 60.0, 0.0, 160.0, 20.0, 0.0, 160.0, 100.0, 0.0, 80.0, 100.0, 0.0, 80.0, 20.0, 0.0]);
    t.rgb("GL_TRIANGLE_FAN: derecha", 150, 60, GREEN, 1);
    t.rgb("GL_TRIANGLE_FAN: arriba", 120, 90, GREEN, 1);
    t.rgb("GL_TRIANGLE_FAN: el hueco de abajo", 120, 30, BLACK, 0);
    let corners = [200.0f32, 20.0, 0.0, 260.0, 20.0, 0.0, 260.0, 80.0, 0.0, 200.0, 80.0, 0.0];
    unsafe {
        glEnableClientState(GL_VERTEX_ARRAY);
        glVertexPointer(3, GL_FLOAT, 0, corners.as_ptr().cast());
        let bytes: [u8; 3] = [0, 1, 2];
        glDrawElements(GL_TRIANGLES, 3, GL_UNSIGNED_BYTE, bytes.as_ptr().cast());
        t.rgb("glDrawElements con indices de 8 bits", 250, 30, GREEN, 1);
        t.rgb("la otra mitad sigue vacia", 210, 70, BLACK, 0);
        let shorts: [u16; 3] = [0, 2, 3];
        glDrawElements(GL_TRIANGLES, 3, GL_UNSIGNED_SHORT, shorts.as_ptr().cast());
        t.rgb("glDrawElements con indices de 16 bits", 210, 70, GREEN, 1);
        glDrawArrays(GL_TRIANGLES, 1, 3);
    }
    t.error("primitivas sin errores", GL_NO_ERROR);
    glColor4f(1.0, 1.0, 0.0, 1.0);
    draw(GL_LINES, &[300.5, 50.5, 0.0, 340.5, 50.5, 0.0, 320.5, 60.5, 0.0, 320.5, 100.5, 0.0]);
    t.rgb("GL_LINES: horizontal", 320, 50, [255, 255, 0], 1);
    t.rgb("GL_LINES: nada encima", 320, 51, BLACK, 0);
    t.rgb("GL_LINES: vertical", 320, 80, [255, 255, 0], 1);
    draw(GL_LINE_STRIP, &[360.5, 20.5, 0.0, 400.5, 20.5, 0.0, 400.5, 60.5, 0.0]);
    t.rgb("GL_LINE_STRIP: primer tramo", 380, 20, [255, 255, 0], 1);
    t.rgb("GL_LINE_STRIP: segundo tramo", 400, 40, [255, 255, 0], 1);
    draw(GL_LINE_LOOP, &[420.5, 20.5, 0.0, 460.5, 20.5, 0.0, 460.5, 60.5, 0.0, 420.5, 60.5, 0.0]);
    t.rgb("GL_LINE_LOOP: cierra el contorno", 420, 40, [255, 255, 0], 1);
    t.rgb("GL_LINE_LOOP: hueco por dentro", 440, 40, BLACK, 0);
    glLineWidth(5.0);
    draw(GL_LINES, &[300.0, 120.0, 0.0, 340.0, 120.0, 0.0]);
    glLineWidth(1.0);
    t.rgb("linea ancha: borde inferior", 320, 118, [255, 255, 0], 1);
    t.rgb("linea ancha: borde superior", 320, 121, [255, 255, 0], 1);
    t.rgb("linea ancha: fuera", 320, 124, BLACK, 0);
    draw(GL_POINTS, &[500.5, 40.5, 0.0, 510.5, 40.5, 0.0]);
    t.rgb("GL_POINTS: un pixel", 500, 40, [255, 255, 0], 1);
    t.rgb("GL_POINTS: el segundo", 510, 40, [255, 255, 0], 1);
    t.rgb("GL_POINTS: nada al lado", 501, 40, BLACK, 0);
    glPointSize(8.0);
    draw(GL_POINTS, &[540.0, 40.0, 0.0]);
    glPointSize(1.0);
    t.rgb("punto de 8 pixeles: esquina", 536, 36, [255, 255, 0], 1);
    t.rgb("punto de 8 pixeles: esquina opuesta", 543, 43, [255, 255, 0], 1);
    t.rgb("punto de 8 pixeles: fuera", 544, 40, BLACK, 0);
}

fn vertex_formats(t: &mut T) {
    t.section("formatos de vertices");
    pixel_projection();
    clear_black();
    glColor4f(1.0, 1.0, 1.0, 1.0);
    unsafe {
        glEnableClientState(GL_VERTEX_ARRAY);
        let bytes: [i8; 8] = [10, 10, 60, 10, 60, 60, 10, 60];
        glVertexPointer(2, GL_BYTE, 0, bytes.as_ptr().cast());
        glDrawArrays(GL_TRIANGLE_FAN, 0, 4);
        t.rgb("GL_BYTE", 30, 30, WHITE, 1);
        let shorts: [i16; 8] = [100, 10, 300, 10, 300, 60, 100, 60];
        glVertexPointer(2, GL_SHORT, 0, shorts.as_ptr().cast());
        glDrawArrays(GL_TRIANGLE_FAN, 0, 4);
        t.rgb("GL_SHORT", 290, 50, WHITE, 1);
        let fixed: [i32; 8] = [320 << 16, 10 << 16, 400 << 16, 10 << 16, 400 << 16, 60 << 16, 320 << 16, 60 << 16];
        glVertexPointer(2, GL_FIXED, 0, fixed.as_ptr().cast());
        glDrawArrays(GL_TRIANGLE_FAN, 0, 4);
        t.rgb("GL_FIXED", 390, 50, WHITE, 1);
        // Array intercalado y sin alinear: posicion (2 float) + color (4 bytes), paso de 12.
        let mut packed = vec![0u8; 1 + 4 * 12];
        let corners = [(420.0f32, 10.0f32), (500.0, 10.0), (500.0, 60.0), (420.0, 60.0)];
        for (i, (x, y)) in corners.iter().enumerate() {
            let base = 1 + i * 12;
            packed[base..base + 4].copy_from_slice(&x.to_le_bytes());
            packed[base + 4..base + 8].copy_from_slice(&y.to_le_bytes());
            packed[base + 8..base + 12].copy_from_slice(&[0, 128, 255, 255]);
        }
        glVertexPointer(2, GL_FLOAT, 12, packed.as_ptr().add(1).cast());
        glEnableClientState(GL_COLOR_ARRAY);
        glColorPointer(4, GL_UNSIGNED_BYTE, 12, packed.as_ptr().add(9).cast());
        glDrawArrays(GL_TRIANGLE_FAN, 0, 4);
        t.rgb("array intercalado con paso de 12 bytes", 460, 30, [0, 128, 255], 1);
        let float_colors = [0.5f32; 16];
        glColorPointer(4, GL_FLOAT, 0, float_colors.as_ptr().cast());
        quad(520.0, 10.0, 600.0, 60.0, 0.0);
        t.rgb("colores GL_FLOAT", 560, 30, [128, 128, 128], 1);
        let fixed_colors = [0x4000i32, 0x8000, 0x10000, 0x10000].repeat(4);
        glColorPointer(4, GL_FIXED, 0, fixed_colors.as_ptr().cast());
        quad(520.0, 80.0, 600.0, 130.0, 0.0);
        t.rgb("colores GL_FIXED", 560, 100, [64, 128, 255], 1);
        glDisableClientState(GL_COLOR_ARRAY);
        glVertexPointer(5, GL_FLOAT, 0, packed.as_ptr().cast());
    }
    t.error("glVertexPointer con 5 componentes", GL_INVALID_VALUE);
}

fn matrices(t: &mut T) {
    t.section("matrices");
    pixel_projection();
    clear_black();
    glColor4f(1.0, 1.0, 1.0, 1.0);
    glPushMatrix();
    glTranslatef(100.0, 50.0, 0.0);
    quad(0.0, 0.0, 20.0, 20.0, 0.0);
    glPopMatrix();
    t.rgb("glTranslatef", 110, 60, WHITE, 1);
    t.rgb("glTranslatef: el origen queda vacio", 10, 10, BLACK, 0);
    glPushMatrix();
    glTranslatef(200.0, 50.0, 0.0);
    glScalef(3.0, 2.0, 1.0);
    quad(0.0, 0.0, 20.0, 20.0, 0.0);
    glPopMatrix();
    t.rgb("glScalef: ancho triple", 258, 60, WHITE, 1);
    t.rgb("glScalef: alto doble", 210, 88, WHITE, 1);
    t.rgb("glScalef: nada mas alla", 262, 60, BLACK, 0);
    glPushMatrix();
    glTranslatef(400.0, 100.0, 0.0);
    glRotatef(90.0, 0.0, 0.0, 1.0);
    quad(0.0, 0.0, 60.0, 20.0, 0.0);
    glPopMatrix();
    t.rgb("glRotatef 90 grados: hacia arriba", 390, 150, WHITE, 1);
    t.rgb("glRotatef 90 grados: no a la derecha", 440, 110, BLACK, 0);
    quad(10.0, 10.0, 20.0, 20.0, 0.0);
    t.rgb("glPopMatrix restaura la matriz", 15, 15, WHITE, 1);
    let shift: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 500.0, 300.0, 0.0, 1.0];
    unsafe {
        glPushMatrix();
        glLoadMatrixf(shift.as_ptr());
        quad(0.0, 0.0, 20.0, 20.0, 0.0);
        t.rgb("glLoadMatrixf", 510, 310, WHITE, 1);
        glMultMatrixf(shift.as_ptr());
        glPopMatrix();
    }
    // Perspectiva: un cuadrado de lado 2 a distancia 2 con un campo de 90 grados
    // ocupa la mitad de la ventana.
    glClear(GL_COLOR_BUFFER_BIT);
    glMatrixMode(GL_PROJECTION);
    glLoadIdentity();
    glFrustumf(-1.0, 1.0, -1.0, 1.0, 1.0, 10.0);
    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();
    glTranslatef(0.0, 0.0, -2.0);
    quad(-1.0, -1.0, 1.0, 1.0, 0.0);
    t.rgb("glFrustumf: centro", 320, 224, WHITE, 1);
    t.rgb("glFrustumf: dentro de la mitad", 165, 115, WHITE, 1);
    t.rgb("glFrustumf: fuera de la mitad", 155, 224, BLACK, 0);
    t.rgb("glFrustumf: fuera por arriba", 320, 340, BLACK, 0);
    glViewport(320, 0, 320, 448);
    glClear(GL_COLOR_BUFFER_BIT);
    glLoadIdentity();
    glMatrixMode(GL_PROJECTION);
    glLoadIdentity();
    glMatrixMode(GL_MODELVIEW);
    quad(-1.0, -1.0, 1.0, 1.0, 0.0);
    glViewport(0, 0, W, H);
    t.rgb("glViewport: la mitad derecha", 330, 10, WHITE, 1);
    t.rgb("glViewport: no la izquierda", 310, 224, BLACK, 0);
    pixel_projection();
}

fn clipping(t: &mut T) {
    t.section("recorte");
    clear_black();
    glMatrixMode(GL_PROJECTION);
    glLoadIdentity();
    glFrustumf(-1.0, 1.0, -1.0, 1.0, 1.0, 10.0);
    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();
    glColor4f(0.0, 1.0, 1.0, 1.0);
    // Un suelo que pasa por detras de la camara: hay que recortarlo con el plano cercano.
    draw(GL_TRIANGLE_FAN, &[-5.0, -1.0, 5.0, 5.0, -1.0, 5.0, 5.0, -1.0, -50.0, -5.0, -1.0, -50.0]);
    t.rgb("suelo que cruza el plano cercano: abajo", 320, 20, [0, 255, 255], 1);
    t.rgb("el horizonte queda por debajo del centro", 320, 230, BLACK, 0);
    t.rgb("recortado en el plano lejano", 320, 215, BLACK, 0);
    // Un triangulo enorme, mucho mayor que la ventana.
    pixel_projection();
    glColor4f(1.0, 0.0, 1.0, 1.0);
    draw(GL_TRIANGLES, &[-30000.0, -30000.0, 0.0, 30000.0, -30000.0, 0.0, 320.0, 400.0, 0.0]);
    t.rgb("triangulo gigante: dentro", 320, 300, [255, 0, 255], 1);
    t.rgb("triangulo gigante: por encima del vertice", 320, 410, BLACK, 0);
    // Todo fuera.
    quad(700.0, 100.0, 800.0, 200.0, 0.0);
    quad(100.0, 100.0, 200.0, 200.0, 5.0);
    t.rgb("lo que queda fuera del volumen no se dibuja", 150, 430, BLACK, 0);
    clear_black();
    glColor4f(1.0, 1.0, 1.0, 1.0);
    draw(GL_LINES, &[-500.0, 100.5, 0.0, 2000.0, 100.5, 0.0]);
    t.rgb("linea recortada por los dos lados", 5, 100, WHITE, 1);
    t.rgb("linea recortada: extremo derecho", 630, 100, WHITE, 1);
    draw(GL_POINTS, &[-10.0, 10.0, 0.0, 10.5, 10.5, 2.0]);
    t.error("recorte sin errores", GL_NO_ERROR);
}

fn depth(t: &mut T) {
    t.section("profundidad");
    pixel_projection();
    clear_black();
    glEnable(GL_DEPTH_TEST);
    glDepthFunc(GL_LESS);
    // En esta proyeccion, z mayor = mas cerca.
    glColor4f(1.0, 0.0, 0.0, 1.0);
    quad(20.0, 20.0, 80.0, 80.0, 0.5);
    glColor4f(0.0, 0.0, 1.0, 1.0);
    quad(50.0, 50.0, 110.0, 110.0, -0.5);
    t.rgb("GL_LESS: lo cercano tapa a lo lejano dibujado despues", 60, 60, RED, 1);
    t.rgb("GL_LESS: lo lejano se ve donde no hay nada", 100, 100, BLUE, 1);
    glColor4f(0.0, 1.0, 0.0, 1.0);
    quad(20.0, 20.0, 40.0, 40.0, 0.5);
    t.rgb("GL_LESS: a igual profundidad no pasa", 30, 30, RED, 1);
    glDepthFunc(GL_LEQUAL);
    quad(20.0, 20.0, 40.0, 40.0, 0.5);
    t.rgb("GL_LEQUAL: a igual profundidad pasa", 30, 30, GREEN, 1);
    glDepthFunc(GL_ALWAYS);
    glColor4f(1.0, 1.0, 0.0, 1.0);
    quad(60.0, 60.0, 70.0, 70.0, -0.9);
    t.rgb("GL_ALWAYS", 65, 65, [255, 255, 0], 1);
    glDepthFunc(GL_NEVER);
    quad(20.0, 20.0, 110.0, 110.0, 0.9);
    t.rgb("GL_NEVER", 100, 100, BLUE, 1);
    glDepthFunc(GL_LESS);
    // Sin escritura de profundidad, lo siguiente no queda tapado.
    glDepthMask(GL_FALSE);
    glColor4f(1.0, 1.0, 1.0, 1.0);
    quad(200.0, 20.0, 260.0, 80.0, 0.8);
    glDepthMask(GL_TRUE);
    glColor4f(1.0, 0.0, 0.0, 1.0);
    quad(200.0, 20.0, 230.0, 80.0, 0.0);
    t.rgb("glDepthMask(GL_FALSE) no escribe profundidad", 210, 50, RED, 1);
    // Con la prueba desactivada no se compara ni se escribe.
    glDisable(GL_DEPTH_TEST);
    glColor4f(0.0, 1.0, 0.0, 1.0);
    quad(300.0, 20.0, 360.0, 80.0, 0.9);
    glEnable(GL_DEPTH_TEST);
    glColor4f(0.0, 0.0, 1.0, 1.0);
    quad(300.0, 20.0, 330.0, 80.0, -0.9);
    t.rgb("sin GL_DEPTH_TEST no se escribe profundidad", 310, 50, BLUE, 1);
    // glClearDepthf.
    glClearDepthf(0.5);
    glClear(GL_DEPTH_BUFFER_BIT);
    glColor4f(1.0, 1.0, 1.0, 1.0);
    quad(400.0, 20.0, 430.0, 80.0, -0.5);
    quad(430.0, 20.0, 460.0, 80.0, 0.5);
    t.rgb("glClearDepthf(0.5): detras no se dibuja", 415, 50, BLACK, 0);
    t.rgb("glClearDepthf(0.5): delante si", 445, 50, WHITE, 1);
    glClearDepthf(1.0);
    glClear(GL_DEPTH_BUFFER_BIT);
    // glDepthRangef comprime la profundidad.
    glDepthRangef(0.0, 0.1);
    glColor4f(1.0, 0.0, 0.0, 1.0);
    quad(500.0, 20.0, 560.0, 80.0, -0.9);
    glDepthRangef(0.0, 1.0);
    glColor4f(0.0, 1.0, 0.0, 1.0);
    quad(500.0, 20.0, 530.0, 80.0, 0.5);
    t.rgb("glDepthRangef", 510, 50, RED, 1);
    // Desplazamiento de poligonos coplanares.
    glColor4f(1.0, 0.0, 0.0, 1.0);
    quad(20.0, 200.0, 80.0, 260.0, 0.0);
    glEnable(GL_POLYGON_OFFSET_FILL);
    glPolygonOffset(-1.0, -4.0);
    glColor4f(0.0, 1.0, 0.0, 1.0);
    quad(20.0, 200.0, 50.0, 260.0, 0.0);
    glDisable(GL_POLYGON_OFFSET_FILL);
    t.rgb("glPolygonOffset acerca el poligono coplanar", 30, 230, GREEN, 1);
    // Sentido contrario: con GL_GREATER gana lo mas lejano.
    glClearDepthf(0.0);
    glDepthFunc(GL_GREATER);
    glClear(GL_DEPTH_BUFFER_BIT);
    glColor4f(1.0, 0.0, 0.0, 1.0);
    quad(200.0, 200.0, 260.0, 260.0, -0.5);
    glColor4f(0.0, 0.0, 1.0, 1.0);
    quad(200.0, 200.0, 230.0, 260.0, 0.5);
    t.rgb("GL_GREATER: lo lejano gana", 210, 230, RED, 1);
    glDepthFunc(GL_LESS);
    glClearDepthf(1.0);
    glClear(GL_DEPTH_BUFFER_BIT);
    glDisable(GL_DEPTH_TEST);
}

fn culling(t: &mut T) {
    t.section("caras");
    pixel_projection();
    clear_black();
    glColor4f(1.0, 1.0, 1.0, 1.0);
    let clockwise = |x: f32| draw(GL_TRIANGLE_FAN, &[x, 20.0, 0.0, x, 80.0, 0.0, x + 60.0, 80.0, 0.0, x + 60.0, 20.0, 0.0]);
    glEnable(GL_CULL_FACE);
    quad(20.0, 20.0, 80.0, 80.0, 0.0);
    t.rgb("antihorario es cara delantera: se dibuja", 50, 50, WHITE, 1);
    clockwise(100.0);
    t.rgb("horario es cara trasera: se descarta", 130, 50, BLACK, 0);
    glFrontFace(GL_CW);
    clockwise(200.0);
    quad(300.0, 20.0, 360.0, 80.0, 0.0);
    t.rgb("glFrontFace(GL_CW): el horario se dibuja", 230, 50, WHITE, 1);
    t.rgb("glFrontFace(GL_CW): el antihorario no", 330, 50, BLACK, 0);
    glFrontFace(GL_CCW);
    glCullFace(GL_FRONT);
    quad(400.0, 20.0, 460.0, 80.0, 0.0);
    clockwise(500.0);
    t.rgb("glCullFace(GL_FRONT): descarta la delantera", 430, 50, BLACK, 0);
    t.rgb("glCullFace(GL_FRONT): dibuja la trasera", 530, 50, WHITE, 1);
    glCullFace(GL_FRONT_AND_BACK);
    quad(20.0, 100.0, 80.0, 160.0, 0.0);
    draw(GL_LINES, &[100.0, 130.5, 0.0, 160.0, 130.5, 0.0]);
    t.rgb("GL_FRONT_AND_BACK: ningun triangulo", 50, 130, BLACK, 0);
    t.rgb("GL_FRONT_AND_BACK: las lineas si", 130, 130, WHITE, 1);
    glCullFace(GL_BACK);
    glDisable(GL_CULL_FACE);
}

fn blending(t: &mut T) {
    t.section("mezcla");
    pixel_projection();
    glClearColor(0.0, 0.0, 1.0, 0.5);
    glClear(GL_COLOR_BUFFER_BIT);
    glEnable(GL_BLEND);
    let mut x = 20.0;
    let mut blend = |t: &mut T, name: &str, src: GLenum, dst: GLenum, color: [f32; 4], expected: [u8; 3]| {
        glBlendFunc(src, dst);
        glColor4f(color[0], color[1], color[2], color[3]);
        quad(x, 20.0, x + 30.0, 50.0, 0.0);
        t.rgb(name, x as i32 + 15, 35, expected, 4);
        x += 40.0;
    };
    // Fondo: azul (0, 0, 255) con alfa 0,5.
    blend(t, "GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA al 50%", GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA, [1.0, 0.0, 0.0, 0.5], [128, 0, 128]);
    blend(t, "GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA al 25%", GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA, [1.0, 1.0, 1.0, 0.25], [64, 64, 255]);
    blend(t, "GL_ONE, GL_ONE (aditiva)", GL_ONE, GL_ONE, [0.5, 0.25, 0.5, 1.0], [128, 64, 255]);
    blend(t, "GL_SRC_ALPHA, GL_ONE", GL_SRC_ALPHA, GL_ONE, [1.0, 1.0, 0.0, 0.5], [128, 128, 255]);
    blend(t, "GL_ZERO, GL_SRC_ALPHA (atenua el fondo)", GL_ZERO, GL_SRC_ALPHA, [1.0, 1.0, 1.0, 0.5], [0, 0, 128]);
    blend(t, "GL_ZERO, GL_ONE_MINUS_SRC_ALPHA", GL_ZERO, GL_ONE_MINUS_SRC_ALPHA, [1.0, 1.0, 1.0, 0.75], [0, 0, 64]);
    blend(t, "GL_ONE_MINUS_SRC_ALPHA, GL_SRC_ALPHA", GL_ONE_MINUS_SRC_ALPHA, GL_SRC_ALPHA, [1.0, 0.0, 0.0, 0.75], [64, 0, 191]);
    blend(t, "GL_DST_ALPHA, GL_ONE_MINUS_DST_ALPHA", GL_DST_ALPHA, GL_ONE_MINUS_DST_ALPHA, [1.0, 1.0, 0.0, 1.0], [128, 128, 128]);
    blend(t, "GL_DST_ALPHA, GL_ZERO", GL_DST_ALPHA, GL_ZERO, [1.0, 1.0, 1.0, 1.0], [128, 128, 128]);
    blend(t, "GL_ONE_MINUS_DST_ALPHA, GL_DST_ALPHA", GL_ONE_MINUS_DST_ALPHA, GL_DST_ALPHA, [1.0, 0.5, 0.0, 1.0], [128, 64, 128]);
    blend(t, "GL_DST_ALPHA, GL_ONE", GL_DST_ALPHA, GL_ONE, [1.0, 0.5, 0.0, 1.0], [128, 64, 255]);
    blend(t, "GL_ZERO, GL_ONE (no cambia nada)", GL_ZERO, GL_ONE, [1.0, 1.0, 1.0, 1.0], [0, 0, 255]);
    blend(t, "GL_ZERO, GL_ZERO", GL_ZERO, GL_ZERO, [1.0, 1.0, 1.0, 1.0], [0, 0, 0]);
    blend(t, "GL_ONE, GL_ZERO", GL_ONE, GL_ZERO, [0.25, 0.5, 0.75, 0.1], [64, 128, 191]);
    glBlendFunc(GL_ONE, GL_ZERO);
    glDisable(GL_BLEND);
    glColor4f(1.0, 1.0, 1.0, 0.1);
    quad(20.0, 100.0, 50.0, 130.0, 0.0);
    t.rgb("sin GL_BLEND el alfa no mezcla", 35, 115, WHITE, 1);
}

fn fragment_tests(t: &mut T) {
    t.section("alfa, mascara y recorte");
    pixel_projection();
    clear_black();
    glEnable(GL_ALPHA_TEST);
    let mut x = 20.0;
    let mut alpha = |t: &mut T, name: &str, func: GLenum, value: f32, passes: bool| {
        glAlphaFunc(func, 0.5);
        glColor4f(1.0, 1.0, 1.0, value);
        quad(x, 20.0, x + 20.0, 40.0, 0.0);
        t.rgb(name, x as i32 + 10, 30, if passes { WHITE } else { BLACK }, 1);
        x += 30.0;
    };
    alpha(t, "GL_GREATER: 0,75 pasa", GL_GREATER, 0.75, true);
    alpha(t, "GL_GREATER: 0,25 no pasa", GL_GREATER, 0.25, false);
    alpha(t, "GL_LESS: 0,25 pasa", GL_LESS, 0.25, true);
    alpha(t, "GL_LESS: 0,75 no pasa", GL_LESS, 0.75, false);
    alpha(t, "GL_GEQUAL: 1 pasa", GL_GEQUAL, 1.0, true);
    alpha(t, "GL_LEQUAL: 1 no pasa", GL_LEQUAL, 1.0, false);
    alpha(t, "GL_NOTEQUAL: 0 pasa", GL_NOTEQUAL, 0.0, true);
    alpha(t, "GL_EQUAL: 0 no pasa", GL_EQUAL, 0.0, false);
    alpha(t, "GL_NEVER", GL_NEVER, 1.0, false);
    alpha(t, "GL_ALWAYS", GL_ALWAYS, 0.0, true);
    glDisable(GL_ALPHA_TEST);
    glColor4f(1.0, 1.0, 1.0, 1.0);
    glColorMask(GL_TRUE, GL_FALSE, GL_FALSE, GL_TRUE);
    quad(20.0, 100.0, 60.0, 140.0, 0.0);
    glColorMask(GL_FALSE, GL_FALSE, GL_TRUE, GL_TRUE);
    quad(40.0, 100.0, 80.0, 140.0, 0.0);
    glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
    t.rgb("glColorMask: solo rojo", 30, 120, RED, 1);
    t.rgb("glColorMask: rojo y luego azul", 50, 120, [255, 0, 255], 1);
    t.rgb("glColorMask: solo azul", 70, 120, BLUE, 1);
    glEnable(GL_SCISSOR_TEST);
    glScissor(210, 110, 20, 10);
    quad(200.0, 100.0, 260.0, 160.0, 0.0);
    draw(GL_LINES, &[150.0, 115.5, 0.0, 300.0, 115.5, 0.0]);
    glScissor(0, 0, 0, 0);
    quad(300.0, 100.0, 360.0, 160.0, 0.0);
    glDisable(GL_SCISSOR_TEST);
    t.rgb("glScissor: dentro", 210, 110, WHITE, 1);
    t.rgb("glScissor: ultimo pixel", 229, 119, WHITE, 1);
    t.rgb("glScissor: fuera", 230, 115, BLACK, 0);
    t.rgb("glScissor: fuera por abajo", 215, 109, BLACK, 0);
    t.rgb("glScissor vacio: no se dibuja nada", 330, 130, BLACK, 0);
    glEnable(GL_STENCIL_TEST);
    glStencilFunc(GL_NEVER, 1, 0xff);
    glEnable(GL_DITHER);
    quad(400.0, 100.0, 460.0, 160.0, 0.0);
    glDisable(GL_STENCIL_TEST);
    t.rgb("sin bits de stencil, su prueba siempre pasa", 430, 130, WHITE, 1);
}

/// Textura de 2x2: rojo y verde abajo, azul y blanco arriba.
const FOUR_TEXELS: [u8; 16] = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];

fn texturing(t: &mut T) {
    t.section("texturas");
    pixel_projection();
    clear_black();
    glColor4f(1.0, 1.0, 1.0, 1.0);
    glEnable(GL_TEXTURE_2D);
    glTexEnvf(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, GL_REPLACE as f32);
    let four = texture(2, 2, GL_RGBA, GL_UNSIGNED_BYTE, &FOUR_TEXELS, GL_NEAREST);
    textured_quad(20.0, 20.0, 84.0, 84.0, 1.0, 1.0);
    t.rgb("GL_NEAREST: texel (0, 0) abajo a la izquierda", 36, 36, RED, 1);
    t.rgb("GL_NEAREST: texel (1, 0)", 68, 36, GREEN, 1);
    t.rgb("GL_NEAREST: texel (0, 1)", 36, 68, BLUE, 1);
    t.rgb("GL_NEAREST: texel (1, 1)", 68, 68, WHITE, 1);
    textured_quad(100.0, 20.0, 164.0, 84.0, 2.0, 2.0);
    t.rgb("GL_REPEAT: la textura se repite", 140, 28, RED, 1);
    t.rgb("GL_REPEAT: segunda copia", 156, 60, GREEN, 1);
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE as f32);
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE as f32);
    textured_quad(180.0, 20.0, 244.0, 84.0, 2.0, 2.0);
    t.rgb("GL_CLAMP_TO_EDGE: se estira el borde", 220, 28, GREEN, 1);
    t.rgb("GL_CLAMP_TO_EDGE: esquina", 236, 76, WHITE, 1);
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR as f32);
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR as f32);
    textured_quad(260.0, 20.0, 324.0, 84.0, 1.0, 1.0);
    t.rgb("GL_LINEAR: mezcla de los cuatro en el centro", 292, 52, [128, 128, 128], 8);
    t.rgb("GL_LINEAR: la esquina conserva su texel", 261, 21, RED, 8);
    // Actualizacion parcial.
    unsafe { glTexSubImage2D(GL_TEXTURE_2D, 0, 1, 0, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, [255u8, 255, 0, 255].as_ptr().cast()) };
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST as f32);
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as f32);
    textured_quad(340.0, 20.0, 404.0, 84.0, 1.0, 1.0);
    t.rgb("glTexSubImage2D cambia un texel", 388, 36, [255, 255, 0], 1);
    t.rgb("glTexSubImage2D deja los demas", 356, 36, RED, 1);
    // Formatos de pixel.
    let mut x = 20.0;
    let mut format = |t: &mut T, name: &str, format: GLenum, kind: GLenum, data: &[u8], expected: [u8; 3]| {
        let id = texture(1, 1, format, kind, data, GL_NEAREST);
        textured_quad(x, 100.0, x + 20.0, 120.0, 1.0, 1.0);
        t.rgb(name, x as i32 + 10, 110, expected, 1);
        unsafe { glDeleteTextures(1, &id) };
        x += 30.0;
    };
    format(t, "GL_RGB", GL_RGB, GL_UNSIGNED_BYTE, &[10, 200, 30], [10, 200, 30]);
    format(t, "GL_RGB 5_6_5", GL_RGB, GL_UNSIGNED_SHORT_5_6_5, &0xf81fu16.to_le_bytes(), [255, 0, 255]);
    format(t, "GL_RGBA 4_4_4_4", GL_RGBA, GL_UNSIGNED_SHORT_4_4_4_4, &0x0f0fu16.to_le_bytes(), [0, 255, 0]);
    format(t, "GL_RGBA 5_5_5_1", GL_RGBA, GL_UNSIGNED_SHORT_5_5_5_1, &0x003fu16.to_le_bytes(), [0, 0, 255]);
    format(t, "GL_LUMINANCE", GL_LUMINANCE, GL_UNSIGNED_BYTE, &[90], [90, 90, 90]);
    format(t, "GL_LUMINANCE_ALPHA", GL_LUMINANCE_ALPHA, GL_UNSIGNED_BYTE, &[200, 255], [200, 200, 200]);
    // Con paleta de 4 bits: indices 1, 2 / 2, 1.
    let mut paletted = vec![0u8; 16 * 3];
    paletted[3..6].copy_from_slice(&[255, 128, 0]);
    paletted[6..9].copy_from_slice(&[0, 128, 255]);
    paletted.extend_from_slice(&[0x12, 0x21]);
    unsafe {
        let mut id = 0;
        glGenTextures(1, &mut id);
        glBindTexture(GL_TEXTURE_2D, id);
        glCompressedTexImage2D(GL_TEXTURE_2D, 0, GL_PALETTE4_RGB8_OES, 2, 2, 0, paletted.len() as i32, paletted.as_ptr().cast());
        glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST as f32);
        glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as f32);
        textured_quad(300.0, 100.0, 340.0, 140.0, 1.0, 1.0);
        t.rgb("textura con paleta: indice 1", 310, 110, [255, 128, 0], 1);
        t.rgb("textura con paleta: indice 2", 330, 110, [0, 128, 255], 1);
        t.rgb("textura con paleta: segunda fila", 310, 130, [0, 128, 255], 1);
        glCompressedTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, 1, 1, GL_PALETTE4_RGB8_OES, 0, null());
        t.error("glCompressedTexSubImage2D: GL_INVALID_OPERATION", GL_INVALID_OPERATION);
        glDeleteTextures(1, &id);
    }
    // Copia del framebuffer a una textura.
    unsafe {
        let mut id = 0;
        glGenTextures(1, &mut id);
        glBindTexture(GL_TEXTURE_2D, id);
        glCopyTexImage2D(GL_TEXTURE_2D, 0, GL_RGB, 20, 20, 64, 64, 0);
        glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST as f32);
        glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as f32);
        textured_quad(400.0, 100.0, 464.0, 164.0, 1.0, 1.0);
        t.rgb("glCopyTexImage2D: abajo a la izquierda", 416, 116, RED, 1);
        t.rgb("glCopyTexImage2D: arriba a la derecha", 448, 148, WHITE, 1);
        glCopyTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, 52, 52, 32, 32);
        textured_quad(480.0, 100.0, 544.0, 164.0, 1.0, 1.0);
        t.rgb("glCopyTexSubImage2D", 496, 116, WHITE, 1);
        glDeleteTextures(1, &id);
    }
    // Matriz de textura y coordenada de textura actual.
    glBindTexture(GL_TEXTURE_2D, four);
    glMatrixMode(GL_TEXTURE);
    glLoadIdentity();
    glTranslatef(0.5, 0.0, 0.0);
    glMatrixMode(GL_MODELVIEW);
    textured_quad(20.0, 200.0, 84.0, 264.0, 0.5, 0.5);
    glMatrixMode(GL_TEXTURE);
    glLoadIdentity();
    glMatrixMode(GL_MODELVIEW);
    t.rgb("matriz de textura: desplaza medio texel de textura", 52, 232, [255, 255, 0], 1);
    glMultiTexCoord4f(GL_TEXTURE0, 0.25, 0.75, 0.0, 1.0);
    quad(100.0, 200.0, 164.0, 264.0, 0.0);
    t.rgb("glMultiTexCoord4f: una coordenada para todo", 132, 232, BLUE, 1);
    // Sin textura completa o con la textura desactivada, se dibuja sin ella.
    glBindTexture(GL_TEXTURE_2D, 0);
    glColor4f(0.0, 1.0, 1.0, 1.0);
    textured_quad(180.0, 200.0, 244.0, 264.0, 1.0, 1.0);
    t.rgb("textura sin imagen: como si no hubiera", 212, 232, [0, 255, 255], 1);
    glBindTexture(GL_TEXTURE_2D, four);
    glDisable(GL_TEXTURE_2D);
    textured_quad(260.0, 200.0, 324.0, 264.0, 1.0, 1.0);
    t.rgb("GL_TEXTURE_2D desactivado", 292, 232, [0, 255, 255], 1);
    unsafe { glDeleteTextures(1, &four) };
    glTexEnvf(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, GL_MODULATE as f32);
    t.error("texturas sin errores", GL_NO_ERROR);
}

fn texture_environment(t: &mut T) {
    t.section("entorno de textura");
    pixel_projection();
    clear_black();
    glEnable(GL_TEXTURE_2D);
    let rgba = texture(1, 1, GL_RGBA, GL_UNSIGNED_BYTE, &[255, 128, 64, 128], GL_NEAREST);
    let rgb = texture(1, 1, GL_RGB, GL_UNSIGNED_BYTE, &[255, 128, 64], GL_NEAREST);
    let alpha = texture(1, 1, GL_ALPHA, GL_UNSIGNED_BYTE, &[64], GL_NEAREST);
    let mut x = 20.0;
    // Se dibuja con mezcla alfa sobre negro: el alfa resultante se ve en el brillo.
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ZERO);
    let mut env = |t: &mut T, name: &str, id: GLuint, mode: GLenum, color: [f32; 4], expected: [u8; 3]| {
        glBindTexture(GL_TEXTURE_2D, id);
        glTexEnvf(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, mode as f32);
        glColor4f(color[0], color[1], color[2], color[3]);
        textured_quad(x, 20.0, x + 20.0, 40.0, 1.0, 1.0);
        t.rgb(name, x as i32 + 10, 30, expected, 5);
        x += 30.0;
    };
    env(t, "GL_MODULATE con RGBA: color y alfa multiplicados", rgba, GL_MODULATE, [0.5, 1.0, 1.0, 1.0], [64, 64, 32]);
    env(t, "GL_MODULATE con RGB: el alfa es el del vertice", rgb, GL_MODULATE, [1.0, 0.5, 1.0, 0.5], [128, 32, 32]);
    env(t, "GL_MODULATE con ALPHA: color del vertice, alfa multiplicado", alpha, GL_MODULATE, [1.0, 1.0, 0.0, 1.0], [64, 64, 0]);
    env(t, "GL_REPLACE con RGBA: todo de la textura", rgba, GL_REPLACE, [0.0, 0.0, 0.0, 0.0], [128, 64, 32]);
    env(t, "GL_REPLACE con RGB: alfa del vertice", rgb, GL_REPLACE, [0.0, 0.0, 0.0, 0.5], [128, 64, 32]);
    env(t, "GL_REPLACE con ALPHA: color del vertice, alfa de la textura", alpha, GL_REPLACE, [0.0, 1.0, 0.0, 0.0], [0, 64, 0]);
    env(t, "GL_DECAL con RGB", rgb, GL_DECAL, [0.0, 0.0, 0.0, 1.0], [255, 128, 64]);
    glDisable(GL_BLEND);
    glBlendFunc(GL_ONE, GL_ZERO);
    glTexEnvf(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, GL_MODULATE as f32);
    glTexEnvf(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, 12345.0);
    t.error("modo de entorno desconocido", GL_INVALID_ENUM);
    unsafe { glDeleteTextures(3, [rgba, rgb, alpha].as_ptr()) };
    glDisable(GL_TEXTURE_2D);
}

fn mipmaps(t: &mut T) {
    t.section("mipmaps");
    pixel_projection();
    clear_black();
    glEnable(GL_TEXTURE_2D);
    glColor4f(1.0, 1.0, 1.0, 1.0);
    glTexEnvf(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, GL_REPLACE as f32);
    let solid = |size: usize, color: [u8; 3]| -> Vec<u8> { color.iter().copied().cycle().take(size * size * 3).collect() };
    let mut id = 0;
    unsafe {
        glGenTextures(1, &mut id);
        glBindTexture(GL_TEXTURE_2D, id);
        glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGB as GLint, 8, 8, 0, GL_RGB, GL_UNSIGNED_BYTE, solid(8, RED).as_ptr().cast());
        glTexImage2D(GL_TEXTURE_2D, 1, GL_RGB as GLint, 4, 4, 0, GL_RGB, GL_UNSIGNED_BYTE, solid(4, GREEN).as_ptr().cast());
        glTexImage2D(GL_TEXTURE_2D, 2, GL_RGB as GLint, 2, 2, 0, GL_RGB, GL_UNSIGNED_BYTE, solid(2, BLUE).as_ptr().cast());
    }
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as f32);
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST_MIPMAP_NEAREST as f32);
    glColor4f(1.0, 1.0, 0.0, 1.0);
    textured_quad(20.0, 20.0, 28.0, 28.0, 1.0, 1.0);
    t.rgb("piramide incompleta: se dibuja sin textura", 24, 24, [255, 255, 0], 1);
    unsafe { glTexImage2D(GL_TEXTURE_2D, 3, GL_RGB as GLint, 1, 1, 0, GL_RGB, GL_UNSIGNED_BYTE, WHITE.as_ptr().cast()) };
    textured_quad(40.0, 20.0, 72.0, 52.0, 1.0, 1.0);
    t.rgb("ampliada: nivel 0", 56, 36, RED, 1);
    textured_quad(80.0, 20.0, 88.0, 28.0, 1.0, 1.0);
    t.rgb("a escala 1:1: nivel 0", 84, 24, RED, 1);
    textured_quad(100.0, 20.0, 104.0, 24.0, 1.0, 1.0);
    t.rgb("reducida a la mitad: nivel 1", 102, 22, GREEN, 1);
    textured_quad(120.0, 20.0, 122.0, 22.0, 1.0, 1.0);
    t.rgb("reducida a la cuarta parte: nivel 2", 121, 21, BLUE, 1);
    textured_quad(140.0, 20.0, 180.0, 60.0, 10.0, 10.0);
    t.rgb("repetida diez veces: tambien reduce", 160, 40, GREEN, 1);
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST as f32);
    textured_quad(200.0, 20.0, 204.0, 24.0, 1.0, 1.0);
    t.rgb("GL_NEAREST no usa la piramide", 202, 22, RED, 1);
    unsafe { glDeleteTextures(1, &id) };
    glTexEnvf(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, GL_MODULATE as f32);
    glDisable(GL_TEXTURE_2D);
}

fn lighting(t: &mut T) {
    t.section("iluminacion");
    pixel_projection();
    clear_black();
    glEnable(GL_LIGHTING);
    glEnable(GL_LIGHT0);
    glNormal3f(0.0, 0.0, 1.0);
    // Posicion del siguiente cuadro de prueba.
    let column = core::cell::Cell::new(20.0f32);
    let lit = |t: &mut T, name: &str, expected: [u8; 3]| {
        let x = column.get();
        quad(x, 20.0, x + 20.0, 40.0, 0.0);
        t.rgb(name, x as i32 + 10, 30, expected, 4);
        column.set(x + 30.0);
    };
    unsafe {
        // Luz 0 por defecto: direccional desde +z, difusa blanca. Material por
        // defecto: ambiente 0,2 y difuso 0,8; luz ambiente de la escena 0,2.
        lit(t, "luz de frente: 0,2*0,2 + 0,8", [214, 214, 214]);
        glNormal3f(0.0, 0.866_025_4, 0.5);
        lit(t, "normal a 60 grados: 0,04 + 0,8*0,5", [112, 112, 112]);
        glNormal3f(0.0, 0.0, -1.0);
        lit(t, "normal de espaldas: solo ambiente", [10, 10, 10]);
        glNormal3f(0.0, 0.0, 5.0);
        lit(t, "normal sin normalizar: satura", WHITE);
        glEnable(GL_NORMALIZE);
        lit(t, "GL_NORMALIZE", [214, 214, 214]);
        glDisable(GL_NORMALIZE);
        glNormal3f(0.0, 0.0, 1.0);
        glPushMatrix();
        glScalef(2.0, 2.0, 2.0);
        let x = column.get();
        glTranslatef(-x / 2.0, 0.0, 0.0);
        quad(x, 10.0, x + 10.0, 20.0, 0.0);
        t.rgb("la escala encoge la normal: 0,04 + 0,8*0,5", x as i32 + 10, 30, [112, 112, 112], 4);
        glEnable(GL_RESCALE_NORMAL);
        quad(x + 15.0, 10.0, x + 25.0, 20.0, 0.0);
        t.rgb("GL_RESCALE_NORMAL lo corrige", x as i32 + 40, 30, [214, 214, 214], 4);
        glDisable(GL_RESCALE_NORMAL);
        glPopMatrix();
        column.set(x + 60.0);
        glMaterialfv(GL_FRONT_AND_BACK, GL_AMBIENT_AND_DIFFUSE, [1.0f32, 0.5, 0.0, 1.0].as_ptr());
        lit(t, "material naranja: (0,2 + 1; 0,1 + 0,5; 0)", [255, 153, 0]);
        glLightfv(GL_LIGHT0, GL_DIFFUSE, [0.0f32, 0.0, 1.0, 1.0].as_ptr());
        glLightfv(GL_LIGHT0, GL_AMBIENT, [0.0f32, 0.5, 0.0, 1.0].as_ptr());
        lit(t, "luz azul con ambiente verde", [51, 89, 0]);
        glLightfv(GL_LIGHT0, GL_DIFFUSE, [1.0f32; 4].as_ptr());
        glLightfv(GL_LIGHT0, GL_AMBIENT, [0.0f32, 0.0, 0.0, 1.0].as_ptr());
        glMaterialfv(GL_FRONT_AND_BACK, GL_AMBIENT_AND_DIFFUSE, [0.0f32, 0.0, 0.0, 1.0].as_ptr());
        glMaterialfv(GL_FRONT_AND_BACK, GL_EMISSION, [0.0f32, 0.5, 0.25, 1.0].as_ptr());
        lit(t, "emision", [0, 128, 64]);
        glMaterialfv(GL_FRONT_AND_BACK, GL_EMISSION, [0.0f32, 0.0, 0.0, 1.0].as_ptr());
        glMaterialfv(GL_FRONT_AND_BACK, GL_SPECULAR, [0.5f32, 0.25, 1.0, 1.0].as_ptr());
        glMaterialf(GL_FRONT_AND_BACK, GL_SHININESS, 4.0);
        lit(t, "brillo especular de frente", [128, 64, 255]);
        glNormal3f(0.0, 0.6, 0.8);
        // Luz y observador en +z: el vector medio es +z y n.h = 0,8; 0,8^4 = 0,41.
        lit(t, "brillo especular con exponente 4", [52, 26, 104]);
        glNormal3f(0.0, 0.0, 1.0);
        glMaterialfv(GL_FRONT_AND_BACK, GL_SPECULAR, [0.0f32, 0.0, 0.0, 1.0].as_ptr());
        glMaterialfv(GL_FRONT_AND_BACK, GL_AMBIENT, [0.2f32, 0.2, 0.2, 1.0].as_ptr());
        glMaterialfv(GL_FRONT_AND_BACK, GL_DIFFUSE, [0.8f32, 0.8, 0.8, 1.0].as_ptr());
        glEnable(GL_COLOR_MATERIAL);
        glColor4f(0.0, 0.5, 1.0, 1.0);
        lit(t, "GL_COLOR_MATERIAL: el color hace de material", [0, 153, 255]);
        glDisable(GL_COLOR_MATERIAL);
        glLightModelfv(GL_LIGHT_MODEL_AMBIENT, [1.0f32, 0.0, 0.0, 1.0].as_ptr());
        lit(t, "luz ambiente de la escena roja", [255, 204, 204]);
        glLightModelfv(GL_LIGHT_MODEL_AMBIENT, [0.2f32, 0.2, 0.2, 1.0].as_ptr());
        glEnable(GL_LIGHT1);
        glLightfv(GL_LIGHT1, GL_DIFFUSE, [0.0f32, 0.25, 0.0, 1.0].as_ptr());
        glLightfv(GL_LIGHT1, GL_POSITION, [0.0f32, 0.0, 1.0, 0.0].as_ptr());
        glLightfv(GL_LIGHT0, GL_DIFFUSE, [0.5f32, 0.0, 0.0, 1.0].as_ptr());
        lit(t, "dos luces se suman", [112, 61, 10]);
        glDisable(GL_LIGHT1);
        glLightfv(GL_LIGHT0, GL_DIFFUSE, [1.0f32; 4].as_ptr());
        // Luz puntual a 100 de distancia, con atenuacion lineal.
        let center = column.get() + 10.0;
        glLightfv(GL_LIGHT0, GL_POSITION, [center, 30.0, 100.0, 1.0].as_ptr());
        glLightf(GL_LIGHT0, GL_LINEAR_ATTENUATION, 0.01);
        lit(t, "luz puntual atenuada: 0,04 + 0,8/2", [112, 112, 112]);
        glLightf(GL_LIGHT0, GL_LINEAR_ATTENUATION, 0.0);
        let center = column.get() + 10.0;
        glLightfv(GL_LIGHT0, GL_POSITION, [center, 30.0, 100.0, 1.0].as_ptr());
        glLightfv(GL_LIGHT0, GL_SPOT_DIRECTION, [0.0f32, 0.0, -1.0].as_ptr());
        glLightf(GL_LIGHT0, GL_SPOT_CUTOFF, 30.0);
        lit(t, "foco: dentro del cono", [213, 213, 213]);
        let center = column.get() + 10.0;
        glLightfv(GL_LIGHT0, GL_POSITION, [center + 200.0, 30.0, 100.0, 1.0].as_ptr());
        lit(t, "foco: fuera del cono", [10, 10, 10]);
        glLightf(GL_LIGHT0, GL_SPOT_CUTOFF, 180.0);
        glLightfv(GL_LIGHT0, GL_POSITION, [0.0f32, 0.0, 1.0, 0.0].as_ptr());
        // Dos caras: la trasera se ilumina con la normal invertida.
        let clockwise = |x: f32| draw(GL_TRIANGLE_FAN, &[x, 60.0, 0.0, x, 80.0, 0.0, x + 20.0, 80.0, 0.0, x + 20.0, 60.0, 0.0]);
        clockwise(20.0);
        t.rgb("cara trasera sin GL_LIGHT_MODEL_TWO_SIDE", 30, 70, [214, 214, 214], 4);
        glLightModelf(GL_LIGHT_MODEL_TWO_SIDE, 1.0);
        clockwise(50.0);
        t.rgb("cara trasera con dos caras: normal invertida", 60, 70, [10, 10, 10], 4);
        glLightModelf(GL_LIGHT_MODEL_TWO_SIDE, 0.0);
        glDisable(GL_LIGHT0);
        quad(80.0, 60.0, 100.0, 80.0, 0.0);
        t.rgb("sin luces encendidas: solo ambiente", 90, 70, [10, 10, 10], 4);
        glLightf(GL_LIGHT0, GL_SPOT_CUTOFF, 100.0);
        t.error("angulo de foco invalido", GL_INVALID_VALUE);
    }
    glDisable(GL_LIGHTING);
    glColor4f(1.0, 1.0, 1.0, 1.0);
}

fn fog(t: &mut T) {
    t.section("niebla");
    pixel_projection();
    clear_black();
    glEnable(GL_FOG);
    glColor4f(1.0, 1.0, 1.0, 1.0);
    unsafe { glFogfv(GL_FOG_COLOR, [0.0f32, 0.0, 1.0, 1.0].as_ptr()) };
    // En esta proyeccion la distancia al ojo es |z|.
    glFogf(GL_FOG_MODE, GL_LINEAR as f32);
    glFogf(GL_FOG_START, 0.0);
    glFogf(GL_FOG_END, 1.0);
    quad(20.0, 20.0, 40.0, 40.0, -0.5);
    t.rgb("GL_LINEAR a media distancia", 30, 30, [128, 128, 255], 4);
    quad(50.0, 20.0, 70.0, 40.0, 0.0);
    t.rgb("GL_LINEAR en el inicio: sin niebla", 60, 30, WHITE, 2);
    quad(80.0, 20.0, 100.0, 40.0, -0.999);
    t.rgb("GL_LINEAR en el final: todo niebla", 90, 30, BLUE, 2);
    glFogf(GL_FOG_MODE, GL_EXP as f32);
    glFogf(GL_FOG_DENSITY, 1.386_294_4);
    quad(110.0, 20.0, 130.0, 40.0, -0.5);
    t.rgb("GL_EXP: e^(-d*z) = 0,5", 120, 30, [128, 128, 255], 4);
    glFogf(GL_FOG_MODE, GL_EXP2 as f32);
    glFogf(GL_FOG_DENSITY, 1.665_109_2);
    quad(140.0, 20.0, 160.0, 40.0, 0.5);
    t.rgb("GL_EXP2: e^(-(d*z)^2) = 0,5", 150, 30, [128, 128, 255], 4);
    // La niebla se interpola a lo largo del triangulo.
    glFogf(GL_FOG_MODE, GL_LINEAR as f32);
    draw(GL_TRIANGLE_FAN, &[200.0, 20.0, 0.0, 328.0, 20.0, -1.0, 328.0, 40.0, -1.0, 200.0, 40.0, 0.0]);
    t.rgb("niebla interpolada: cerca", 204, 30, [247, 247, 255], 6);
    t.rgb("niebla interpolada: en medio", 264, 30, [127, 127, 255], 6);
    t.rgb("niebla interpolada: lejos", 323, 30, [9, 9, 255], 6);
    glFogf(GL_FOG_DENSITY, -1.0);
    t.error("densidad negativa", GL_INVALID_VALUE);
    glDisable(GL_FOG);
}

fn reading(t: &mut T) {
    t.section("glReadPixels");
    pixel_projection();
    clear_black();
    glColor4f(1.0, 0.0, 0.0, 1.0);
    quad(100.0, 100.0, 104.0, 102.0, 0.0);
    glColor4f(0.0, 0.0, 1.0, 0.5);
    quad(100.0, 102.0, 104.0, 104.0, 0.0);
    let mut block = [0u8; 4 * 4 * 4];
    unsafe { glReadPixels(100, 100, 4, 4, GL_RGBA, GL_UNSIGNED_BYTE, block.as_mut_ptr().cast()) };
    t.check("la primera fila es la de abajo", block[..4] == [255, 0, 0, 255] && block[12..16] == [255, 0, 0, 255]);
    t.check("la ultima fila es la de arriba", block[48..51] == [0, 0, 255] && (block[51] as i32 - 128).abs() <= 1);
    // Alineacion de filas: 3 pixeles = 12 bytes, que con alineacion 8 son 16.
    let mut padded = [7u8; 16 * 2];
    glPixelStorei(GL_PACK_ALIGNMENT, 8);
    unsafe { glReadPixels(100, 101, 3, 2, GL_RGBA, GL_UNSIGNED_BYTE, padded.as_mut_ptr().cast()) };
    glPixelStorei(GL_PACK_ALIGNMENT, 4);
    t.check("GL_PACK_ALIGNMENT deja relleno entre filas", padded[..3] == [255, 0, 0] && padded[12..16] == [7; 4] && padded[16..19] == [0, 0, 255]);
    // Fuera de la ventana no se toca el destino... ni se rompe nada.
    let mut outside = [9u8; 4 * 4];
    unsafe { glReadPixels(-2, 100, 4, 1, GL_RGBA, GL_UNSIGNED_BYTE, outside.as_mut_ptr().cast()) };
    t.check("lectura que se sale por la izquierda", outside[8..11] == [0, 0, 0]);
    unsafe { glReadPixels(0, 0, 1, 1, GL_RGB, GL_UNSIGNED_BYTE, outside.as_mut_ptr().cast()) };
    t.error("solo GL_RGBA con GL_UNSIGNED_BYTE", GL_INVALID_OPERATION);
}

fn fixed_point(t: &mut T) {
    t.section("coma fija");
    const ONE: GLfixed = 0x10000;
    glClearColorx(ONE / 2, ONE / 4, ONE, ONE);
    glClear(GL_COLOR_BUFFER_BIT);
    t.rgb("glClearColorx", 320, 224, [128, 64, 255], 1);
    glMatrixMode(GL_PROJECTION);
    glLoadIdentity();
    glOrthox(0, W * ONE, 0, H * ONE, -ONE, ONE);
    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();
    glTranslatex(100 * ONE, 50 * ONE, 0);
    glScalex(2 * ONE, ONE / 2, ONE);
    glRotatex(90 * ONE, 0, 0, ONE);
    glColor4x(ONE, ONE / 2, 0, ONE);
    let fixed: [i32; 8] = [0, 0, 20 * ONE, 0, 20 * ONE, 20 * ONE, 0, 20 * ONE];
    unsafe {
        glEnableClientState(GL_VERTEX_ARRAY);
        glVertexPointer(2, GL_FIXED, 0, fixed.as_ptr().cast());
        glDrawArrays(GL_TRIANGLE_FAN, 0, 4);
    }
    // Girado 90 grados (hacia -x), estirado 2 en x y encogido a la mitad en y.
    t.rgb("glOrthox, glTranslatex, glScalex, glRotatex, glColor4x", 70, 55, [255, 128, 0], 1);
    t.rgb("fuera del rectangulo transformado", 70, 62, [128, 64, 255], 1);
    glLoadIdentity();
    glEnable(GL_FOG);
    glFogx(GL_FOG_MODE, GL_LINEAR as GLfixed);
    glFogx(GL_FOG_START, 0);
    glFogx(GL_FOG_END, ONE);
    unsafe { glFogxv(GL_FOG_COLOR, [0, ONE, 0, ONE].as_ptr()) };
    glColor4x(ONE, 0, 0, ONE);
    quad(200.0, 50.0, 220.0, 70.0, -0.5);
    glDisable(GL_FOG);
    t.rgb("glFogx y glFogxv", 210, 60, [128, 128, 0], 4);
    glAlphaFuncx(GL_GREATER, ONE / 2);
    glLineWidthx(ONE);
    glPointSizex(ONE);
    glDepthRangex(0, ONE);
    glClearDepthx(ONE);
    glPolygonOffsetx(0, 0);
    t.error("variantes de coma fija sin errores", GL_NO_ERROR);
}

fn presenting(t: &mut T, display: EGLDisplay, surface: EGLSurface) {
    t.section("presentacion");
    pixel_projection();
    t.check("eglSwapBuffers", eglSwapBuffers(display, surface) == EGL_TRUE);
    // Tras presentar se sigue dibujando con el mismo estado, en el otro bufer.
    glClearColor(0.0, 0.25, 0.0, 1.0);
    glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
    glEnable(GL_DEPTH_TEST);
    glColor4f(1.0, 1.0, 1.0, 1.0);
    quad(200.0, 150.0, 440.0, 300.0, 0.0);
    glColor4f(1.0, 0.0, 0.0, 1.0);
    quad(260.0, 190.0, 380.0, 260.0, -0.5);
    glDisable(GL_DEPTH_TEST);
    t.rgb("tras eglSwapBuffers: fondo", 50, 50, [0, 64, 0], 1);
    t.rgb("tras eglSwapBuffers: la profundidad sigue funcionando", 320, 224, WHITE, 1);
    t.check("eglSwapBuffers con otra superficie: EGL_BAD_SURFACE", eglSwapBuffers(display, 77) == EGL_FALSE && eglGetError() == EGL_BAD_SURFACE);
    eglSwapBuffers(display, surface);
    t.error("fin sin errores de GL", GL_NO_ERROR);
}

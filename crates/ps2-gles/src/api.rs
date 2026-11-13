//! Las funciones de OpenGL ES 1.0 (perfil Common), con los nombres y firmas
//! de la especificación. Las variantes de coma fija (`...x`) están en
//! `fixed.rs`.
//!
//! Como en C, actúan sobre el contexto actual (`eglMakeCurrent`) y sin
//! contexto no hacen nada. Las que reciben punteros son `unsafe`.

#![allow(non_snake_case, clippy::missing_safety_doc, clippy::too_many_arguments)]

use crate::consts::*;
use crate::context::*;
use crate::math::{self, clamp01, Mat4, IDENTITY};
use crate::raster::{Compare, Factor, Filter, TexEnv, TexFormat, Wrap};
use crate::texture;
use alloc::vec;
use alloc::vec::Vec;

macro_rules! ctx {
    () => {
        match current() {
            Some(c) => c,
            None => return,
        }
    };
    ($default:expr) => {
        match current() {
            Some(c) => c,
            None => return $default,
        }
    };
}

/// Sale de la función anotando un error de GL.
macro_rules! fail {
    ($c:expr, $error:expr) => {{
        $c.set_error($error);
        return;
    }};
}

fn compare_of(func: GLenum) -> Option<Compare> {
    Some(match func {
        GL_NEVER => Compare::Never,
        GL_LESS => Compare::Less,
        GL_EQUAL => Compare::Equal,
        GL_LEQUAL => Compare::Lequal,
        GL_GREATER => Compare::Greater,
        GL_NOTEQUAL => Compare::Notequal,
        GL_GEQUAL => Compare::Gequal,
        GL_ALWAYS => Compare::Always,
        _ => return None,
    })
}

fn factor_of(factor: GLenum) -> Option<Factor> {
    Some(match factor {
        GL_ZERO => Factor::Zero,
        GL_ONE => Factor::One,
        GL_SRC_COLOR => Factor::SrcColor,
        GL_ONE_MINUS_SRC_COLOR => Factor::OneMinusSrcColor,
        GL_DST_COLOR => Factor::DstColor,
        GL_ONE_MINUS_DST_COLOR => Factor::OneMinusDstColor,
        GL_SRC_ALPHA => Factor::SrcAlpha,
        GL_ONE_MINUS_SRC_ALPHA => Factor::OneMinusSrcAlpha,
        GL_DST_ALPHA => Factor::DstAlpha,
        GL_ONE_MINUS_DST_ALPHA => Factor::OneMinusDstAlpha,
        GL_SRC_ALPHA_SATURATE => Factor::SrcAlphaSaturate,
        _ => return None,
    })
}

unsafe fn read4(p: *const GLfloat) -> [f32; 4] {
    [p.read_unaligned(), p.add(1).read_unaligned(), p.add(2).read_unaligned(), p.add(3).read_unaligned()]
}

// ---------------------------------------------------------------- errores y consultas

pub fn glGetError() -> GLenum {
    let c = ctx!(GL_NO_ERROR);
    core::mem::replace(&mut c.error, GL_NO_ERROR)
}

/// Cadenas de la implementación, terminadas en 0 como en C.
pub fn glGetString(name: GLenum) -> *const GLubyte {
    let c = ctx!(core::ptr::null());
    let text: &'static [u8] = match name {
        GL_VENDOR => b"PS2Rust\0",
        GL_RENDERER => b"PlayStation 2 Graphics Synthesizer\0",
        GL_VERSION => b"OpenGL ES-CM 1.0\0",
        GL_EXTENSIONS => b"GL_OES_byte_coordinates GL_OES_fixed_point GL_OES_single_precision GL_OES_read_format GL_OES_compressed_paletted_texture\0",
        _ => {
            c.set_error(GL_INVALID_ENUM);
            return core::ptr::null();
        }
    };
    text.as_ptr()
}

pub unsafe fn glGetIntegerv(pname: GLenum, params: *mut GLint) {
    let c = ctx!();
    let (w, h) = c.raster.size();
    let one = |v: i32| params.write_unaligned(v);
    let two = |a: i32, b: i32| {
        params.write_unaligned(a);
        params.add(1).write_unaligned(b);
    };
    match pname {
        GL_MAX_LIGHTS => one(MAX_LIGHTS as i32),
        GL_MAX_MODELVIEW_STACK_DEPTH => one(MODELVIEW_DEPTH as i32),
        GL_MAX_PROJECTION_STACK_DEPTH => one(PROJECTION_DEPTH as i32),
        GL_MAX_TEXTURE_STACK_DEPTH => one(TEXTURE_DEPTH as i32),
        GL_MAX_TEXTURE_SIZE => one(MAX_TEXTURE_SIZE as i32),
        GL_MAX_TEXTURE_UNITS => one(1),
        GL_MAX_VIEWPORT_DIMS => two(w as i32, h as i32),
        GL_MAX_ELEMENTS_VERTICES | GL_MAX_ELEMENTS_INDICES => one(65536),
        GL_SUBPIXEL_BITS => one(4),
        GL_RED_BITS | GL_GREEN_BITS | GL_BLUE_BITS | GL_ALPHA_BITS => one(8),
        GL_DEPTH_BITS => one(c.raster.depth_bits() as i32),
        GL_STENCIL_BITS => one(0),
        GL_ALIASED_POINT_SIZE_RANGE | GL_SMOOTH_POINT_SIZE_RANGE => two(1, 64),
        GL_ALIASED_LINE_WIDTH_RANGE | GL_SMOOTH_LINE_WIDTH_RANGE => two(1, 64),
        GL_NUM_COMPRESSED_TEXTURE_FORMATS => one(texture::PALETTE_FORMATS.len() as i32),
        GL_COMPRESSED_TEXTURE_FORMATS => {
            for (i, f) in texture::PALETTE_FORMATS.iter().enumerate() {
                params.add(i).write_unaligned(*f as i32);
            }
        }
        GL_IMPLEMENTATION_COLOR_READ_TYPE_OES => one(GL_UNSIGNED_BYTE as i32),
        GL_IMPLEMENTATION_COLOR_READ_FORMAT_OES => one(GL_RGBA as i32),
        _ => c.set_error(GL_INVALID_ENUM),
    }
}

pub fn glHint(target: GLenum, mode: GLenum) {
    let c = ctx!();
    let target_ok = matches!(target, GL_PERSPECTIVE_CORRECTION_HINT | GL_POINT_SMOOTH_HINT | GL_LINE_SMOOTH_HINT | GL_POLYGON_SMOOTH_HINT | GL_FOG_HINT);
    if !target_ok || !matches!(mode, GL_DONT_CARE | GL_FASTEST | GL_NICEST) {
        c.set_error(GL_INVALID_ENUM);
    }
}

pub fn glFlush() {
    ctx!().raster.finish();
}

pub fn glFinish() {
    ctx!().raster.finish();
}

// ---------------------------------------------------------------- capacidades

fn set_cap(cap: GLenum, on: bool) {
    let c = ctx!();
    match c.cap(cap) {
        Some(flag) => *flag = on,
        None => c.set_error(GL_INVALID_ENUM),
    }
}

pub fn glEnable(cap: GLenum) {
    set_cap(cap, true);
}

pub fn glDisable(cap: GLenum) {
    set_cap(cap, false);
}

fn set_client_state(array: GLenum, on: bool) {
    let c = ctx!();
    let index = match array {
        GL_VERTEX_ARRAY => ARRAY_VERTEX,
        GL_NORMAL_ARRAY => ARRAY_NORMAL,
        GL_COLOR_ARRAY => ARRAY_COLOR,
        GL_TEXTURE_COORD_ARRAY => ARRAY_TEXCOORD,
        _ => fail!(c, GL_INVALID_ENUM),
    };
    c.arrays[index].enabled = on;
}

pub fn glEnableClientState(array: GLenum) {
    set_client_state(array, true);
}

pub fn glDisableClientState(array: GLenum) {
    set_client_state(array, false);
}

pub fn glActiveTexture(texture: GLenum) {
    let c = ctx!();
    if texture != GL_TEXTURE0 {
        c.set_error(GL_INVALID_ENUM);
    }
}

pub fn glClientActiveTexture(texture: GLenum) {
    glActiveTexture(texture);
}

// ---------------------------------------------------------------- matrices

pub fn glMatrixMode(mode: GLenum) {
    let c = ctx!();
    if !matches!(mode, GL_MODELVIEW | GL_PROJECTION | GL_TEXTURE) {
        fail!(c, GL_INVALID_ENUM);
    }
    c.matrix_mode = mode;
}

pub fn glLoadIdentity() {
    *ctx!().stack().0.last_mut().unwrap() = IDENTITY;
}

unsafe fn read_matrix(m: *const GLfloat) -> Mat4 {
    let mut out = [0.0; 16];
    for (i, x) in out.iter_mut().enumerate() {
        *x = m.add(i).read_unaligned();
    }
    out
}

pub unsafe fn glLoadMatrixf(m: *const GLfloat) {
    *ctx!().stack().0.last_mut().unwrap() = read_matrix(m);
}

pub unsafe fn glMultMatrixf(m: *const GLfloat) {
    ctx!().mult(&read_matrix(m));
}

pub fn glPushMatrix() {
    let c = ctx!();
    let (stack, depth) = c.stack();
    if stack.len() >= depth {
        fail!(c, GL_STACK_OVERFLOW);
    }
    let top = *stack.last().unwrap();
    stack.push(top);
}

pub fn glPopMatrix() {
    let c = ctx!();
    let (stack, _) = c.stack();
    if stack.len() <= 1 {
        fail!(c, GL_STACK_UNDERFLOW);
    }
    stack.pop();
}

pub fn glTranslatef(x: GLfloat, y: GLfloat, z: GLfloat) {
    let mut m = IDENTITY;
    m[12] = x;
    m[13] = y;
    m[14] = z;
    ctx!().mult(&m);
}

pub fn glScalef(x: GLfloat, y: GLfloat, z: GLfloat) {
    let mut m = IDENTITY;
    m[0] = x;
    m[5] = y;
    m[10] = z;
    ctx!().mult(&m);
}

pub fn glRotatef(angle: GLfloat, x: GLfloat, y: GLfloat, z: GLfloat) {
    let c = ctx!();
    let len2 = x * x + y * y + z * z;
    if len2 <= 0.0 {
        return;
    }
    let k = 1.0 / math::sqrt(len2);
    let (x, y, z) = (x * k, y * k, z * k);
    let radians = angle * (core::f32::consts::PI / 180.0);
    let (s, co) = (math::sin(radians), math::cos(radians));
    let t = 1.0 - co;
    let m: Mat4 = [
        x * x * t + co,
        y * x * t + z * s,
        x * z * t - y * s,
        0.0,
        x * y * t - z * s,
        y * y * t + co,
        y * z * t + x * s,
        0.0,
        x * z * t + y * s,
        y * z * t - x * s,
        z * z * t + co,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ];
    c.mult(&m);
}

pub fn glFrustumf(left: GLfloat, right: GLfloat, bottom: GLfloat, top: GLfloat, near: GLfloat, far: GLfloat) {
    let c = ctx!();
    if near <= 0.0 || far <= 0.0 || left == right || bottom == top || near == far {
        fail!(c, GL_INVALID_VALUE);
    }
    let mut m = [0.0; 16];
    m[0] = 2.0 * near / (right - left);
    m[5] = 2.0 * near / (top - bottom);
    m[8] = (right + left) / (right - left);
    m[9] = (top + bottom) / (top - bottom);
    m[10] = -(far + near) / (far - near);
    m[11] = -1.0;
    m[14] = -2.0 * far * near / (far - near);
    c.mult(&m);
}

pub fn glOrthof(left: GLfloat, right: GLfloat, bottom: GLfloat, top: GLfloat, near: GLfloat, far: GLfloat) {
    let c = ctx!();
    if left == right || bottom == top || near == far {
        fail!(c, GL_INVALID_VALUE);
    }
    let mut m = IDENTITY;
    m[0] = 2.0 / (right - left);
    m[5] = 2.0 / (top - bottom);
    m[10] = -2.0 / (far - near);
    m[12] = -(right + left) / (right - left);
    m[13] = -(top + bottom) / (top - bottom);
    m[14] = -(far + near) / (far - near);
    c.mult(&m);
}

pub fn glViewport(x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    let c = ctx!();
    if width < 0 || height < 0 {
        fail!(c, GL_INVALID_VALUE);
    }
    let (w, h) = c.raster.size();
    c.viewport = [x, y, width.min(w as i32), height.min(h as i32)];
}

pub fn glDepthRangef(near: GLclampf, far: GLclampf) {
    ctx!().depth_range = [clamp01(near), clamp01(far)];
}

pub fn glScissor(x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    let c = ctx!();
    if width < 0 || height < 0 {
        fail!(c, GL_INVALID_VALUE);
    }
    c.scissor = [x, y, width, height];
}

// ---------------------------------------------------------------- vértices

pub fn glColor4f(red: GLfloat, green: GLfloat, blue: GLfloat, alpha: GLfloat) {
    ctx!().color = [red, green, blue, alpha];
}

pub fn glNormal3f(nx: GLfloat, ny: GLfloat, nz: GLfloat) {
    ctx!().normal = [nx, ny, nz];
}

pub fn glMultiTexCoord4f(target: GLenum, s: GLfloat, t: GLfloat, r: GLfloat, q: GLfloat) {
    let c = ctx!();
    if target != GL_TEXTURE0 {
        fail!(c, GL_INVALID_ENUM);
    }
    c.texcoord = [s, t, r, q];
}

unsafe fn set_pointer(index: usize, sizes: core::ops::RangeInclusive<i32>, types: &[GLenum], size: GLint, kind: GLenum, stride: GLsizei, pointer: *const GLvoid) {
    let c = ctx!();
    if !sizes.contains(&size) || stride < 0 {
        fail!(c, GL_INVALID_VALUE);
    }
    if !types.contains(&kind) {
        fail!(c, GL_INVALID_ENUM);
    }
    let enabled = c.arrays[index].enabled;
    c.arrays[index] = Array { enabled, size, kind, stride, pointer: pointer as *const u8 };
}

pub unsafe fn glVertexPointer(size: GLint, kind: GLenum, stride: GLsizei, pointer: *const GLvoid) {
    set_pointer(ARRAY_VERTEX, 2..=4, &[GL_BYTE, GL_SHORT, GL_FIXED, GL_FLOAT], size, kind, stride, pointer);
}

pub unsafe fn glNormalPointer(kind: GLenum, stride: GLsizei, pointer: *const GLvoid) {
    set_pointer(ARRAY_NORMAL, 3..=3, &[GL_BYTE, GL_SHORT, GL_FIXED, GL_FLOAT], 3, kind, stride, pointer);
}

pub unsafe fn glColorPointer(size: GLint, kind: GLenum, stride: GLsizei, pointer: *const GLvoid) {
    set_pointer(ARRAY_COLOR, 4..=4, &[GL_UNSIGNED_BYTE, GL_FIXED, GL_FLOAT], size, kind, stride, pointer);
}

pub unsafe fn glTexCoordPointer(size: GLint, kind: GLenum, stride: GLsizei, pointer: *const GLvoid) {
    set_pointer(ARRAY_TEXCOORD, 2..=4, &[GL_BYTE, GL_SHORT, GL_FIXED, GL_FLOAT], size, kind, stride, pointer);
}

pub unsafe fn glDrawArrays(mode: GLenum, first: GLint, count: GLsizei) {
    let c = ctx!();
    if mode > GL_TRIANGLE_FAN {
        fail!(c, GL_INVALID_ENUM);
    }
    if count < 0 || first < 0 {
        fail!(c, GL_INVALID_VALUE);
    }
    c.draw(mode, count as usize, &|i| first as usize + i);
}

pub unsafe fn glDrawElements(mode: GLenum, count: GLsizei, kind: GLenum, indices: *const GLvoid) {
    let c = ctx!();
    if mode > GL_TRIANGLE_FAN || !matches!(kind, GL_UNSIGNED_BYTE | GL_UNSIGNED_SHORT) {
        fail!(c, GL_INVALID_ENUM);
    }
    if count < 0 {
        fail!(c, GL_INVALID_VALUE);
    }
    if kind == GL_UNSIGNED_BYTE {
        let p = indices as *const u8;
        c.draw(mode, count as usize, &|i| p.add(i).read() as usize);
    } else {
        let p = indices as *const u16;
        c.draw(mode, count as usize, &|i| p.add(i).read_unaligned() as usize);
    }
}

// ---------------------------------------------------------------- iluminación y niebla

pub fn glShadeModel(mode: GLenum) {
    let c = ctx!();
    if !matches!(mode, GL_FLAT | GL_SMOOTH) {
        fail!(c, GL_INVALID_ENUM);
    }
    c.smooth = mode == GL_SMOOTH;
}

pub unsafe fn glLightfv(light: GLenum, pname: GLenum, params: *const GLfloat) {
    let c = ctx!();
    if !(GL_LIGHT0..=GL_LIGHT7).contains(&light) {
        fail!(c, GL_INVALID_ENUM);
    }
    let modelview = *c.modelview.last().unwrap();
    let l = &mut c.lights[(light - GL_LIGHT0) as usize];
    let value = params.read_unaligned();
    let mut error = GL_NO_ERROR;
    match pname {
        GL_AMBIENT => l.ambient = read4(params),
        GL_DIFFUSE => l.diffuse = read4(params),
        GL_SPECULAR => l.specular = read4(params),
        // La posición y la dirección se guardan en coordenadas de ojo.
        GL_POSITION => l.position = math::transform(&modelview, read4(params)),
        GL_SPOT_DIRECTION => {
            let d = [value, params.add(1).read_unaligned(), params.add(2).read_unaligned()];
            let t = math::transform(&modelview, [d[0], d[1], d[2], 0.0]);
            l.spot_direction = [t[0], t[1], t[2]];
        }
        GL_SPOT_EXPONENT if (0.0..=128.0).contains(&value) => l.spot_exponent = value,
        GL_SPOT_CUTOFF if (0.0..=90.0).contains(&value) || value == 180.0 => l.spot_cutoff = value,
        GL_CONSTANT_ATTENUATION if value >= 0.0 => l.attenuation[0] = value,
        GL_LINEAR_ATTENUATION if value >= 0.0 => l.attenuation[1] = value,
        GL_QUADRATIC_ATTENUATION if value >= 0.0 => l.attenuation[2] = value,
        GL_SPOT_EXPONENT | GL_SPOT_CUTOFF | GL_CONSTANT_ATTENUATION | GL_LINEAR_ATTENUATION | GL_QUADRATIC_ATTENUATION => error = GL_INVALID_VALUE,
        _ => error = GL_INVALID_ENUM,
    }
    if error != GL_NO_ERROR {
        c.set_error(error);
    }
}

pub fn glLightf(light: GLenum, pname: GLenum, param: GLfloat) {
    let c = ctx!();
    if !matches!(pname, GL_SPOT_EXPONENT | GL_SPOT_CUTOFF | GL_CONSTANT_ATTENUATION | GL_LINEAR_ATTENUATION | GL_QUADRATIC_ATTENUATION) {
        fail!(c, GL_INVALID_ENUM);
    }
    unsafe { glLightfv(light, pname, &param) };
}

pub unsafe fn glMaterialfv(face: GLenum, pname: GLenum, params: *const GLfloat) {
    let c = ctx!();
    if face != GL_FRONT_AND_BACK {
        fail!(c, GL_INVALID_ENUM);
    }
    let m = &mut c.material;
    match pname {
        GL_AMBIENT => m.ambient = read4(params),
        GL_DIFFUSE => m.diffuse = read4(params),
        GL_AMBIENT_AND_DIFFUSE => {
            m.ambient = read4(params);
            m.diffuse = m.ambient;
        }
        GL_SPECULAR => m.specular = read4(params),
        GL_EMISSION => m.emission = read4(params),
        GL_SHININESS => {
            let value = params.read_unaligned();
            if !(0.0..=128.0).contains(&value) {
                fail!(c, GL_INVALID_VALUE);
            }
            m.shininess = value;
        }
        _ => c.set_error(GL_INVALID_ENUM),
    }
}

pub fn glMaterialf(face: GLenum, pname: GLenum, param: GLfloat) {
    let c = ctx!();
    if pname != GL_SHININESS {
        fail!(c, GL_INVALID_ENUM);
    }
    unsafe { glMaterialfv(face, pname, &param) };
}

pub unsafe fn glLightModelfv(pname: GLenum, params: *const GLfloat) {
    let c = ctx!();
    match pname {
        GL_LIGHT_MODEL_AMBIENT => c.light_model_ambient = read4(params),
        GL_LIGHT_MODEL_TWO_SIDE => c.two_side = params.read_unaligned() != 0.0,
        _ => c.set_error(GL_INVALID_ENUM),
    }
}

pub fn glLightModelf(pname: GLenum, param: GLfloat) {
    let c = ctx!();
    if pname != GL_LIGHT_MODEL_TWO_SIDE {
        fail!(c, GL_INVALID_ENUM);
    }
    c.two_side = param != 0.0;
}

pub unsafe fn glFogfv(pname: GLenum, params: *const GLfloat) {
    let c = ctx!();
    let value = params.read_unaligned();
    match pname {
        GL_FOG_MODE => {
            let mode = value as GLenum;
            if !matches!(mode, GL_LINEAR | GL_EXP | GL_EXP2) {
                fail!(c, GL_INVALID_ENUM);
            }
            c.fog_mode = mode;
        }
        GL_FOG_DENSITY => {
            if value < 0.0 {
                fail!(c, GL_INVALID_VALUE);
            }
            c.fog_density = value;
        }
        GL_FOG_START => c.fog_start = value,
        GL_FOG_END => c.fog_end = value,
        GL_FOG_COLOR => {
            let v = read4(params);
            c.fog_color = [clamp01(v[0]), clamp01(v[1]), clamp01(v[2]), clamp01(v[3])];
        }
        _ => c.set_error(GL_INVALID_ENUM),
    }
}

pub fn glFogf(pname: GLenum, param: GLfloat) {
    let c = ctx!();
    if !matches!(pname, GL_FOG_MODE | GL_FOG_DENSITY | GL_FOG_START | GL_FOG_END) {
        fail!(c, GL_INVALID_ENUM);
    }
    unsafe { glFogfv(pname, &param) };
}

// ---------------------------------------------------------------- rasterizado y pruebas

pub fn glCullFace(mode: GLenum) {
    let c = ctx!();
    if !matches!(mode, GL_FRONT | GL_BACK | GL_FRONT_AND_BACK) {
        fail!(c, GL_INVALID_ENUM);
    }
    c.cull_mode = mode;
}

pub fn glFrontFace(mode: GLenum) {
    let c = ctx!();
    if !matches!(mode, GL_CW | GL_CCW) {
        fail!(c, GL_INVALID_ENUM);
    }
    c.front_face = mode;
}

pub fn glLineWidth(width: GLfloat) {
    let c = ctx!();
    if width <= 0.0 {
        fail!(c, GL_INVALID_VALUE);
    }
    c.line_width = width.min(64.0);
}

pub fn glPointSize(size: GLfloat) {
    let c = ctx!();
    if size <= 0.0 {
        fail!(c, GL_INVALID_VALUE);
    }
    c.point_size = size.min(64.0);
}

pub fn glPolygonOffset(factor: GLfloat, units: GLfloat) {
    ctx!().polygon_offset = [factor, units];
}

pub fn glAlphaFunc(func: GLenum, reference: GLclampf) {
    let c = ctx!();
    let Some(f) = compare_of(func) else { fail!(c, GL_INVALID_ENUM) };
    c.alpha_func = f;
    c.alpha_ref = clamp01(reference);
}

pub fn glBlendFunc(sfactor: GLenum, dfactor: GLenum) {
    let c = ctx!();
    let (Some(s), Some(d)) = (factor_of(sfactor), factor_of(dfactor)) else { fail!(c, GL_INVALID_ENUM) };
    // El origen no admite factores del propio origen ni el destino los suyos.
    if matches!(s, Factor::SrcColor | Factor::OneMinusSrcColor) || matches!(d, Factor::DstColor | Factor::OneMinusDstColor | Factor::SrcAlphaSaturate) {
        fail!(c, GL_INVALID_ENUM);
    }
    c.blend_src = s;
    c.blend_dst = d;
}

pub fn glDepthFunc(func: GLenum) {
    let c = ctx!();
    let Some(f) = compare_of(func) else { fail!(c, GL_INVALID_ENUM) };
    c.depth_func = f;
}

pub fn glDepthMask(flag: GLboolean) {
    ctx!().depth_mask = flag != 0;
}

pub fn glColorMask(red: GLboolean, green: GLboolean, blue: GLboolean, alpha: GLboolean) {
    ctx!().color_mask = [red != 0, green != 0, blue != 0, alpha != 0];
}

pub fn glClearColor(red: GLclampf, green: GLclampf, blue: GLclampf, alpha: GLclampf) {
    ctx!().clear_color = [clamp01(red), clamp01(green), clamp01(blue), clamp01(alpha)];
}

pub fn glClearDepthf(depth: GLclampf) {
    ctx!().clear_depth = clamp01(depth);
}

pub fn glClear(mask: GLbitfield) {
    let c = ctx!();
    if mask & !(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT | GL_STENCIL_BUFFER_BIT) != 0 {
        fail!(c, GL_INVALID_VALUE);
    }
    let color = (mask & GL_COLOR_BUFFER_BIT != 0).then_some(c.clear_color);
    let depth = (mask & GL_DEPTH_BUFFER_BIT != 0 && c.depth_mask).then_some(c.clear_depth);
    if color.is_some() || depth.is_some() {
        let scissor = c.raster_scissor();
        c.raster.depth_func_hint(c.depth_func);
        c.raster.clear(color, depth, c.color_mask, scissor);
    }
}

// El búfer de stencil tiene 0 bits: su prueba siempre pasa. Las funciones
// validan sus argumentos y nada más.

pub fn glClearStencil(_s: GLint) {}

pub fn glStencilMask(_mask: GLuint) {}

pub fn glStencilFunc(func: GLenum, _reference: GLint, _mask: GLuint) {
    let c = ctx!();
    if compare_of(func).is_none() {
        c.set_error(GL_INVALID_ENUM);
    }
}

pub fn glStencilOp(fail: GLenum, zfail: GLenum, zpass: GLenum) {
    let c = ctx!();
    let ok = |op: GLenum| matches!(op, GL_KEEP | GL_ZERO | GL_REPLACE | GL_INCR | GL_DECR | GL_INVERT);
    if !ok(fail) || !ok(zfail) || !ok(zpass) {
        c.set_error(GL_INVALID_ENUM);
    }
}

/// El GS no tiene operaciones lógicas: se valida y se recuerda, sin efecto.
pub fn glLogicOp(opcode: GLenum) {
    let c = ctx!();
    if !(GL_CLEAR..=GL_SET).contains(&opcode) {
        c.set_error(GL_INVALID_ENUM);
    }
}

/// Sin multimuestreo: no tiene efecto.
pub fn glSampleCoverage(_value: GLclampf, _invert: GLboolean) {}

pub fn glPixelStorei(pname: GLenum, param: GLint) {
    let c = ctx!();
    if !matches!(param, 1 | 2 | 4 | 8) {
        fail!(c, GL_INVALID_VALUE);
    }
    match pname {
        GL_PACK_ALIGNMENT => c.pack_alignment = param,
        GL_UNPACK_ALIGNMENT => c.unpack_alignment = param,
        _ => c.set_error(GL_INVALID_ENUM),
    }
}

/// Lee del framebuffer un rectángulo en coordenadas de GL (origen abajo a la
/// izquierda), con la fila inferior primero. Lo que cae fuera queda a cero.
fn read_framebuffer(c: &mut Context, x: i32, y: i32, w: i32, h: i32) -> Vec<u8> {
    let (fw, fh) = c.raster.size();
    let (fw, fh) = (fw as i32, fh as i32);
    let mut out = vec![0u8; (w * h * 4) as usize];
    let (x0, x1) = (x.max(0), (x + w).min(fw));
    let (y0, y1) = (y.max(0), (y + h).min(fh));
    if x0 >= x1 || y0 >= y1 {
        return out;
    }
    let (cw, ch) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut part = vec![0u8; cw * ch * 4];
    c.raster.finish();
    // El rasterizador cuenta las filas desde arriba.
    c.raster.read_pixels(x0 as u32, (fh - y1) as u32, cw as u32, ch as u32, &mut part);
    for row in 0..ch {
        let src = (ch - 1 - row) * cw * 4;
        let dst = (((y0 - y) as usize + row) * w as usize + (x0 - x) as usize) * 4;
        out[dst..dst + cw * 4].copy_from_slice(&part[src..src + cw * 4]);
    }
    out
}

pub unsafe fn glReadPixels(x: GLint, y: GLint, width: GLsizei, height: GLsizei, format: GLenum, kind: GLenum, pixels: *mut GLvoid) {
    let c = ctx!();
    if width < 0 || height < 0 {
        fail!(c, GL_INVALID_VALUE);
    }
    if format != GL_RGBA || kind != GL_UNSIGNED_BYTE {
        fail!(c, if texture::format_of(format).is_none() { GL_INVALID_ENUM } else { GL_INVALID_OPERATION });
    }
    if width == 0 || height == 0 {
        return;
    }
    let data = read_framebuffer(c, x, y, width, height);
    let row = width as usize * 4;
    let align = c.pack_alignment as usize;
    let stride = (row + align - 1) / align * align;
    for r in 0..height as usize {
        core::ptr::copy_nonoverlapping(data.as_ptr().add(r * row), (pixels as *mut u8).add(r * stride), row);
    }
}

// ---------------------------------------------------------------- texturas

pub unsafe fn glGenTextures(n: GLsizei, textures: *mut GLuint) {
    let c = ctx!();
    if n < 0 {
        fail!(c, GL_INVALID_VALUE);
    }
    for i in 0..n as usize {
        textures.add(i).write_unaligned(c.next_texture);
        c.next_texture += 1;
    }
}

pub unsafe fn glDeleteTextures(n: GLsizei, textures: *const GLuint) {
    let c = ctx!();
    if n < 0 {
        fail!(c, GL_INVALID_VALUE);
    }
    for i in 0..n as usize {
        let name = textures.add(i).read_unaligned();
        if name != 0 && c.textures.remove(&name).is_some() {
            c.raster.tex_delete(name);
            if c.bound_texture == name {
                c.bound_texture = 0;
            }
        }
    }
}

pub fn glBindTexture(target: GLenum, texture: GLuint) {
    let c = ctx!();
    if target != GL_TEXTURE_2D {
        fail!(c, GL_INVALID_ENUM);
    }
    c.textures.entry(texture).or_insert_with(Texture::new);
    c.next_texture = c.next_texture.max(texture.saturating_add(1));
    c.bound_texture = texture;
}

pub fn glTexParameterf(target: GLenum, pname: GLenum, param: GLfloat) {
    let c = ctx!();
    if target != GL_TEXTURE_2D {
        fail!(c, GL_INVALID_ENUM);
    }
    let value = param as GLenum;
    let filter = match value {
        GL_NEAREST => Some(Filter::Nearest),
        GL_LINEAR => Some(Filter::Linear),
        GL_NEAREST_MIPMAP_NEAREST => Some(Filter::NearestMipNearest),
        GL_LINEAR_MIPMAP_NEAREST => Some(Filter::LinearMipNearest),
        GL_NEAREST_MIPMAP_LINEAR => Some(Filter::NearestMipLinear),
        GL_LINEAR_MIPMAP_LINEAR => Some(Filter::LinearMipLinear),
        _ => None,
    };
    let wrap = match value {
        GL_REPEAT => Some(Wrap::Repeat),
        GL_CLAMP_TO_EDGE => Some(Wrap::ClampToEdge),
        _ => None,
    };
    let bound = c.bound_texture;
    let t = c.textures.get_mut(&bound).unwrap();
    let ok = match (pname, filter, wrap) {
        (GL_TEXTURE_MIN_FILTER, Some(f), _) => {
            t.min = f;
            true
        }
        (GL_TEXTURE_MAG_FILTER, Some(f @ (Filter::Nearest | Filter::Linear)), _) => {
            t.mag = f;
            true
        }
        (GL_TEXTURE_WRAP_S, _, Some(w)) => {
            t.wrap_s = w;
            true
        }
        (GL_TEXTURE_WRAP_T, _, Some(w)) => {
            t.wrap_t = w;
            true
        }
        _ => false,
    };
    if !ok {
        c.set_error(GL_INVALID_ENUM);
    }
}

pub unsafe fn glTexEnvfv(target: GLenum, pname: GLenum, params: *const GLfloat) {
    let c = ctx!();
    if target != GL_TEXTURE_ENV {
        fail!(c, GL_INVALID_ENUM);
    }
    match pname {
        GL_TEXTURE_ENV_MODE => {
            c.tex_env = match params.read_unaligned() as GLenum {
                GL_MODULATE => TexEnv::Modulate,
                GL_DECAL => TexEnv::Decal,
                GL_BLEND => TexEnv::Blend,
                GL_REPLACE => TexEnv::Replace,
                GL_ADD => TexEnv::Add,
                _ => fail!(c, GL_INVALID_ENUM),
            }
        }
        GL_TEXTURE_ENV_COLOR => {
            let v = read4(params);
            c.tex_env_color = [clamp01(v[0]), clamp01(v[1]), clamp01(v[2]), clamp01(v[3])];
        }
        _ => c.set_error(GL_INVALID_ENUM),
    }
}

pub fn glTexEnvf(target: GLenum, pname: GLenum, param: GLfloat) {
    let c = ctx!();
    if pname != GL_TEXTURE_ENV_MODE {
        fail!(c, GL_INVALID_ENUM);
    }
    unsafe { glTexEnvfv(target, pname, &param) };
}

/// Comprueba el nivel y el tamaño de una imagen de textura.
fn valid_image(level: GLint, width: GLsizei, height: GLsizei, border: GLint) -> bool {
    let pot = |v: i32| v >= 0 && (v == 0 || (v as u32).is_power_of_two()) && v as u32 <= MAX_TEXTURE_SIZE;
    (0..MAX_LEVELS as i32).contains(&level) && border == 0 && pot(width) && pot(height)
}

/// Guarda un nivel ya convertido a RGBA en la textura ligada.
fn store_level(c: &mut Context, level: usize, w: usize, h: usize, format: TexFormat, rgba: &[u8]) {
    let name = c.bound_texture;
    let stored = if w == 0 || h == 0 {
        None
    } else if c.raster.tex_image(name, level as u32, w as u32, h as u32, rgba) {
        Some(Level { width: w as u32, height: h as u32, format })
    } else {
        c.set_error(GL_OUT_OF_MEMORY);
        None
    };
    c.textures.get_mut(&name).unwrap().levels[level] = stored;
}

/// Deja los texels en la forma canónica de su formato base (ver `TexFormat`).
fn canonical(format: TexFormat, rgba: &mut [u8]) {
    for p in rgba.chunks_exact_mut(4) {
        match format {
            TexFormat::Alpha => p[..3].copy_from_slice(&[255; 3]),
            TexFormat::Luminance => p.copy_from_slice(&[p[0], p[0], p[0], 255]),
            TexFormat::LuminanceAlpha => {
                p[1] = p[0];
                p[2] = p[0];
            }
            TexFormat::Rgb => p[3] = 255,
            TexFormat::Rgba => {}
        }
    }
}

pub unsafe fn glTexImage2D(target: GLenum, level: GLint, internalformat: GLint, width: GLsizei, height: GLsizei, border: GLint, format: GLenum, kind: GLenum, pixels: *const GLvoid) {
    let c = ctx!();
    if target != GL_TEXTURE_2D {
        fail!(c, GL_INVALID_ENUM);
    }
    let Some(base) = texture::format_of(format) else { fail!(c, GL_INVALID_ENUM) };
    if !matches!(kind, GL_UNSIGNED_BYTE | GL_UNSIGNED_SHORT_4_4_4_4 | GL_UNSIGNED_SHORT_5_5_5_1 | GL_UNSIGNED_SHORT_5_6_5) {
        fail!(c, GL_INVALID_ENUM);
    }
    if !valid_image(level, width, height, border) {
        fail!(c, GL_INVALID_VALUE);
    }
    if internalformat as GLenum != format || texture::bytes_per_pixel(format, kind).is_none() {
        fail!(c, GL_INVALID_OPERATION);
    }
    let (w, h) = (width as usize, height as usize);
    let rgba = if pixels.is_null() {
        vec![0u8; w * h * 4]
    } else {
        texture::to_rgba(format, kind, w, h, c.unpack_alignment as usize, pixels as *const u8)
    };
    store_level(c, level as usize, w, h, base, &rgba);
}

pub unsafe fn glTexSubImage2D(target: GLenum, level: GLint, xoffset: GLint, yoffset: GLint, width: GLsizei, height: GLsizei, format: GLenum, kind: GLenum, pixels: *const GLvoid) {
    let c = ctx!();
    if target != GL_TEXTURE_2D {
        fail!(c, GL_INVALID_ENUM);
    }
    let Some(base) = texture::format_of(format) else { fail!(c, GL_INVALID_ENUM) };
    let name = c.bound_texture;
    let stored = c.textures[&name].levels.get(level.max(0) as usize).copied().flatten();
    let Some(l) = stored.filter(|_| level >= 0) else { fail!(c, GL_INVALID_OPERATION) };
    if xoffset < 0 || yoffset < 0 || width < 0 || height < 0 || (xoffset + width) as u32 > l.width || (yoffset + height) as u32 > l.height {
        fail!(c, GL_INVALID_VALUE);
    }
    if base != l.format || texture::bytes_per_pixel(format, kind).is_none() {
        fail!(c, GL_INVALID_OPERATION);
    }
    if width == 0 || height == 0 || pixels.is_null() {
        return;
    }
    let rgba = texture::to_rgba(format, kind, width as usize, height as usize, c.unpack_alignment as usize, pixels as *const u8);
    c.raster.tex_sub_image(name, level as u32, xoffset as u32, yoffset as u32, width as u32, height as u32, &rgba);
}

pub fn glCopyTexImage2D(target: GLenum, level: GLint, internalformat: GLenum, x: GLint, y: GLint, width: GLsizei, height: GLsizei, border: GLint) {
    let c = ctx!();
    if target != GL_TEXTURE_2D {
        fail!(c, GL_INVALID_ENUM);
    }
    let Some(base) = texture::format_of(internalformat) else { fail!(c, GL_INVALID_ENUM) };
    if !valid_image(level, width, height, border) {
        fail!(c, GL_INVALID_VALUE);
    }
    let mut rgba = read_framebuffer(c, x, y, width, height);
    canonical(base, &mut rgba);
    store_level(c, level as usize, width as usize, height as usize, base, &rgba);
}

pub fn glCopyTexSubImage2D(target: GLenum, level: GLint, xoffset: GLint, yoffset: GLint, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    let c = ctx!();
    if target != GL_TEXTURE_2D {
        fail!(c, GL_INVALID_ENUM);
    }
    let name = c.bound_texture;
    let stored = c.textures[&name].levels.get(level.max(0) as usize).copied().flatten();
    let Some(l) = stored.filter(|_| level >= 0) else { fail!(c, GL_INVALID_OPERATION) };
    if xoffset < 0 || yoffset < 0 || width < 0 || height < 0 || (xoffset + width) as u32 > l.width || (yoffset + height) as u32 > l.height {
        fail!(c, GL_INVALID_VALUE);
    }
    if width == 0 || height == 0 {
        return;
    }
    let mut rgba = read_framebuffer(c, x, y, width, height);
    canonical(l.format, &mut rgba);
    c.raster.tex_sub_image(name, level as u32, xoffset as u32, yoffset as u32, width as u32, height as u32, &rgba);
}

/// Texturas con paleta (`OES_compressed_paletted_texture`). `level` es 0 o
/// negativo: `-level` es el número de niveles de mipmap adicionales que
/// vienen en `data`.
pub unsafe fn glCompressedTexImage2D(target: GLenum, level: GLint, internalformat: GLenum, width: GLsizei, height: GLsizei, border: GLint, image_size: GLsizei, data: *const GLvoid) {
    let c = ctx!();
    if target != GL_TEXTURE_2D || texture::palette_format(internalformat).is_none() {
        fail!(c, GL_INVALID_ENUM);
    }
    if level > 0 || !valid_image(-level, width, height, border) || width == 0 || height == 0 || image_size < 0 || data.is_null() {
        fail!(c, GL_INVALID_VALUE);
    }
    let bytes = core::slice::from_raw_parts(data as *const u8, image_size as usize);
    let Some((format, levels)) = texture::decode_paletted(internalformat, width as usize, height as usize, (-level) as usize + 1, bytes) else {
        fail!(c, GL_INVALID_VALUE)
    };
    let base = texture::format_of(format).unwrap();
    for (i, (w, h, rgba)) in levels.iter().enumerate() {
        store_level(c, i, *w, *h, base, rgba);
    }
}

/// La extensión de paletas no permite actualizar parte de una textura.
pub unsafe fn glCompressedTexSubImage2D(_target: GLenum, _level: GLint, _xoffset: GLint, _yoffset: GLint, _width: GLsizei, _height: GLsizei, _format: GLenum, _image_size: GLsizei, _data: *const GLvoid) {
    ctx!().set_error(GL_INVALID_OPERATION);
}

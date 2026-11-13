//! EGL 1.0 mínimo: lo justo para crear la superficie y el contexto con las
//! llamadas estándar. Hay una sola pantalla, una sola superficie (la pantalla
//! completa, en NTSC o PAL) y un solo contexto a la vez.

#![allow(non_snake_case, clippy::missing_safety_doc)]

use crate::consts::*;
use crate::context::{self, Context};
use crate::raster::Rasterizer;
use alloc::boxed::Box;

/// Configuraciones: color RGBA de 8 bits por canal, sin stencil, y 16 o 24
/// bits de profundidad. La de 16 deja más memoria de vídeo para texturas.
const DEPTH_BITS: [EGLint; 2] = [16, 24];

const DISPLAY: EGLDisplay = 1;
const SURFACE: EGLSurface = 1;
const CONTEXT: EGLContext = 1;

struct Egl {
    initialized: bool,
    error: EGLint,
    /// Superficie creada: (configuración, ventana nativa).
    surface: Option<(EGLConfig, EGLNativeWindowType)>,
    /// Configuración del contexto creado.
    context_config: Option<EGLConfig>,
    /// El contexto de GL, que se construye al hacerlo actual por primera vez.
    context: Option<Box<Context>>,
    current: bool,
}

static mut EGL: Egl = Egl { initialized: false, error: EGL_SUCCESS, surface: None, context_config: None, context: None, current: false };

fn egl() -> &'static mut Egl {
    unsafe { &mut *core::ptr::addr_of_mut!(EGL) }
}

fn fail(error: EGLint) -> EGLBoolean {
    egl().error = error;
    EGL_FALSE
}

fn ok() -> EGLBoolean {
    egl().error = EGL_SUCCESS;
    EGL_TRUE
}

/// Comprueba la pantalla y que esté inicializada.
fn check_display(dpy: EGLDisplay) -> bool {
    if dpy != DISPLAY {
        fail(EGL_BAD_DISPLAY);
        false
    } else if !egl().initialized {
        fail(EGL_NOT_INITIALIZED);
        false
    } else {
        true
    }
}

fn window_size(window: EGLNativeWindowType) -> Option<(u32, u32)> {
    match window {
        PS2_WINDOW_NTSC => Some((640, 448)),
        PS2_WINDOW_PAL => Some((640, 512)),
        _ => None,
    }
}

#[cfg(target_arch = "mips")]
fn new_rasterizer(window: EGLNativeWindowType, depth_bits: u32) -> Box<dyn Rasterizer> {
    let mode = if window == PS2_WINDOW_PAL { ps2::gs::VideoMode::Pal } else { ps2::gs::VideoMode::Ntsc };
    Box::new(crate::gs::GsRasterizer::new(mode, depth_bits))
}

#[cfg(not(target_arch = "mips"))]
fn new_rasterizer(window: EGLNativeWindowType, depth_bits: u32) -> Box<dyn Rasterizer> {
    let (w, h) = window_size(window).unwrap();
    Box::new(crate::soft::SoftRasterizer::new(w, h, depth_bits))
}

pub fn eglGetError() -> EGLint {
    core::mem::replace(&mut egl().error, EGL_SUCCESS)
}

pub fn eglGetDisplay(display: EGLNativeDisplayType) -> EGLDisplay {
    if display == EGL_DEFAULT_DISPLAY {
        DISPLAY
    } else {
        EGL_NO_DISPLAY
    }
}

pub unsafe fn eglInitialize(dpy: EGLDisplay, major: *mut EGLint, minor: *mut EGLint) -> EGLBoolean {
    if dpy != DISPLAY {
        return fail(EGL_BAD_DISPLAY);
    }
    egl().initialized = true;
    if !major.is_null() {
        major.write(1);
    }
    if !minor.is_null() {
        minor.write(0);
    }
    ok()
}

pub fn eglTerminate(dpy: EGLDisplay) -> EGLBoolean {
    if dpy != DISPLAY {
        return fail(EGL_BAD_DISPLAY);
    }
    context::set_current(core::ptr::null_mut());
    let e = egl();
    *e = Egl { initialized: false, error: EGL_SUCCESS, surface: None, context_config: None, context: None, current: false };
    EGL_TRUE
}

/// Cadenas de la implementación, terminadas en 0.
pub fn eglQueryString(dpy: EGLDisplay, name: EGLint) -> *const u8 {
    if !check_display(dpy) {
        return core::ptr::null();
    }
    let text: &'static [u8] = match name {
        EGL_VENDOR => b"PS2Rust\0",
        EGL_VERSION => b"1.0 PS2Rust\0",
        EGL_EXTENSIONS => b"\0",
        _ => {
            fail(EGL_BAD_PARAMETER);
            return core::ptr::null();
        }
    };
    ok();
    text.as_ptr()
}

fn config_attribute(config: EGLConfig, attribute: EGLint) -> Option<EGLint> {
    let depth = *DEPTH_BITS.get(config.wrapping_sub(1))?;
    Some(match attribute {
        EGL_BUFFER_SIZE => 32,
        EGL_RED_SIZE | EGL_GREEN_SIZE | EGL_BLUE_SIZE | EGL_ALPHA_SIZE => 8,
        EGL_DEPTH_SIZE => depth,
        EGL_STENCIL_SIZE | EGL_LEVEL | EGL_SAMPLES | EGL_SAMPLE_BUFFERS | EGL_NATIVE_VISUAL_ID | EGL_NATIVE_VISUAL_TYPE => 0,
        EGL_MAX_PBUFFER_WIDTH | EGL_MAX_PBUFFER_HEIGHT | EGL_MAX_PBUFFER_PIXELS => 0,
        EGL_TRANSPARENT_RED_VALUE | EGL_TRANSPARENT_GREEN_VALUE | EGL_TRANSPARENT_BLUE_VALUE => 0,
        EGL_CONFIG_CAVEAT | EGL_TRANSPARENT_TYPE => EGL_NONE,
        EGL_CONFIG_ID => config as EGLint,
        EGL_NATIVE_RENDERABLE => 0,
        EGL_SURFACE_TYPE => EGL_WINDOW_BIT,
        _ => return None,
    })
}

pub unsafe fn eglGetConfigAttrib(dpy: EGLDisplay, config: EGLConfig, attribute: EGLint, value: *mut EGLint) -> EGLBoolean {
    if !check_display(dpy) {
        return EGL_FALSE;
    }
    if config == 0 || config > DEPTH_BITS.len() {
        return fail(EGL_BAD_CONFIG);
    }
    match config_attribute(config, attribute) {
        Some(v) => {
            value.write(v);
            ok()
        }
        None => fail(EGL_BAD_ATTRIBUTE),
    }
}

/// Escribe las configuraciones que cumplen `accept`, de menor a mayor
/// profundidad (el orden que pide EGL).
unsafe fn list_configs(configs: *mut EGLConfig, size: EGLint, count: *mut EGLint, accept: &dyn Fn(EGLConfig) -> bool) -> EGLBoolean {
    if count.is_null() {
        return fail(EGL_BAD_PARAMETER);
    }
    let mut found = 0;
    for config in 1..=DEPTH_BITS.len() {
        if accept(config) {
            if !configs.is_null() {
                if found >= size {
                    break;
                }
                configs.add(found as usize).write(config);
            }
            found += 1;
        }
    }
    count.write(found);
    ok()
}

pub unsafe fn eglGetConfigs(dpy: EGLDisplay, configs: *mut EGLConfig, config_size: EGLint, num_config: *mut EGLint) -> EGLBoolean {
    if !check_display(dpy) {
        return EGL_FALSE;
    }
    list_configs(configs, config_size, num_config, &|_| true)
}

/// `attrib_list`: pares (atributo, valor) terminados en `EGL_NONE`, o nulo.
pub unsafe fn eglChooseConfig(dpy: EGLDisplay, attrib_list: *const EGLint, configs: *mut EGLConfig, config_size: EGLint, num_config: *mut EGLint) -> EGLBoolean {
    if !check_display(dpy) {
        return EGL_FALSE;
    }
    let mut wanted = alloc::vec::Vec::new();
    let mut p = attrib_list;
    while !p.is_null() && p.read() != EGL_NONE {
        let (attribute, value) = (p.read(), p.add(1).read());
        if config_attribute(1, attribute).is_none() {
            return fail(EGL_BAD_ATTRIBUTE);
        }
        wanted.push((attribute, value));
        p = p.add(2);
    }
    list_configs(configs, config_size, num_config, &|config| {
        wanted.iter().all(|&(attribute, value)| {
            let have = config_attribute(config, attribute).unwrap();
            match attribute {
                _ if value == EGL_DONT_CARE => true,
                // Tamaños: "al menos".
                EGL_BUFFER_SIZE | EGL_RED_SIZE | EGL_GREEN_SIZE | EGL_BLUE_SIZE | EGL_ALPHA_SIZE | EGL_DEPTH_SIZE | EGL_STENCIL_SIZE | EGL_SAMPLES | EGL_SAMPLE_BUFFERS => have >= value,
                // Máscara: todos los bits pedidos.
                EGL_SURFACE_TYPE => have & value == value,
                _ => have == value,
            }
        })
    })
}

pub unsafe fn eglCreateWindowSurface(dpy: EGLDisplay, config: EGLConfig, window: EGLNativeWindowType, _attrib_list: *const EGLint) -> EGLSurface {
    if !check_display(dpy) {
        return EGL_NO_SURFACE;
    }
    if config == 0 || config > DEPTH_BITS.len() {
        fail(EGL_BAD_CONFIG);
        return EGL_NO_SURFACE;
    }
    if window_size(window).is_none() {
        fail(EGL_BAD_NATIVE_WINDOW);
        return EGL_NO_SURFACE;
    }
    if egl().surface.is_some() {
        // Sólo hay una pantalla.
        fail(EGL_BAD_ALLOC);
        return EGL_NO_SURFACE;
    }
    egl().surface = Some((config, window));
    ok();
    SURFACE
}

pub fn eglDestroySurface(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean {
    if !check_display(dpy) {
        return EGL_FALSE;
    }
    if surface != SURFACE || egl().surface.is_none() {
        return fail(EGL_BAD_SURFACE);
    }
    // El contexto de GL dibuja en esta superficie: se va con ella.
    context::set_current(core::ptr::null_mut());
    let e = egl();
    e.surface = None;
    e.context = None;
    e.current = false;
    ok()
}

pub unsafe fn eglQuerySurface(dpy: EGLDisplay, surface: EGLSurface, attribute: EGLint, value: *mut EGLint) -> EGLBoolean {
    if !check_display(dpy) {
        return EGL_FALSE;
    }
    let Some((config, window)) = egl().surface.filter(|_| surface == SURFACE) else { return fail(EGL_BAD_SURFACE) };
    let (w, h) = window_size(window).unwrap();
    value.write(match attribute {
        EGL_WIDTH => w as EGLint,
        EGL_HEIGHT => h as EGLint,
        EGL_CONFIG_ID => config as EGLint,
        _ => return fail(EGL_BAD_ATTRIBUTE),
    });
    ok()
}

pub unsafe fn eglCreateContext(dpy: EGLDisplay, config: EGLConfig, share_context: EGLContext, _attrib_list: *const EGLint) -> EGLContext {
    if !check_display(dpy) {
        return EGL_NO_CONTEXT;
    }
    if config == 0 || config > DEPTH_BITS.len() {
        fail(EGL_BAD_CONFIG);
        return EGL_NO_CONTEXT;
    }
    if share_context != EGL_NO_CONTEXT {
        fail(EGL_BAD_CONTEXT);
        return EGL_NO_CONTEXT;
    }
    if egl().context_config.is_some() {
        fail(EGL_BAD_ALLOC);
        return EGL_NO_CONTEXT;
    }
    egl().context_config = Some(config);
    ok();
    CONTEXT
}

pub fn eglDestroyContext(dpy: EGLDisplay, ctx: EGLContext) -> EGLBoolean {
    if !check_display(dpy) {
        return EGL_FALSE;
    }
    if ctx != CONTEXT || egl().context_config.is_none() {
        return fail(EGL_BAD_CONTEXT);
    }
    context::set_current(core::ptr::null_mut());
    let e = egl();
    e.context_config = None;
    e.context = None;
    e.current = false;
    ok()
}

pub fn eglMakeCurrent(dpy: EGLDisplay, draw: EGLSurface, read: EGLSurface, ctx: EGLContext) -> EGLBoolean {
    if !check_display(dpy) {
        return EGL_FALSE;
    }
    if ctx == EGL_NO_CONTEXT {
        if draw != EGL_NO_SURFACE || read != EGL_NO_SURFACE {
            return fail(EGL_BAD_MATCH);
        }
        context::set_current(core::ptr::null_mut());
        egl().current = false;
        return ok();
    }
    let e = egl();
    let Some(context_config) = e.context_config.filter(|_| ctx == CONTEXT) else { return fail(EGL_BAD_CONTEXT) };
    let Some((surface_config, window)) = e.surface.filter(|_| draw == SURFACE && read == SURFACE) else { return fail(EGL_BAD_SURFACE) };
    if context_config != surface_config {
        return fail(EGL_BAD_MATCH);
    }
    if e.context.is_none() {
        let depth_bits = DEPTH_BITS[context_config - 1] as u32;
        e.context = Some(Box::new(Context::new(new_rasterizer(window, depth_bits))));
    }
    context::set_current(&mut **e.context.as_mut().unwrap());
    e.current = true;
    ok()
}

pub fn eglGetCurrentContext() -> EGLContext {
    if egl().current {
        CONTEXT
    } else {
        EGL_NO_CONTEXT
    }
}

pub fn eglGetCurrentSurface(readdraw: EGLint) -> EGLSurface {
    if egl().current && matches!(readdraw, EGL_DRAW | EGL_READ) {
        SURFACE
    } else {
        EGL_NO_SURFACE
    }
}

pub fn eglGetCurrentDisplay() -> EGLDisplay {
    if egl().current {
        DISPLAY
    } else {
        EGL_NO_DISPLAY
    }
}

/// Presenta el frame: espera al retrazo vertical y cambia de búfer. El
/// contenido del búfer nuevo queda indefinido, como permite EGL.
pub fn eglSwapBuffers(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean {
    if !check_display(dpy) {
        return EGL_FALSE;
    }
    if surface != SURFACE || egl().surface.is_none() {
        return fail(EGL_BAD_SURFACE);
    }
    if let Some(c) = egl().context.as_mut() {
        c.raster.swap();
    }
    ok()
}

pub fn eglWaitGL() -> EGLBoolean {
    if let Some(c) = context::current() {
        c.raster.finish();
    }
    EGL_TRUE
}

pub fn eglWaitNative(_engine: EGLint) -> EGLBoolean {
    EGL_TRUE
}

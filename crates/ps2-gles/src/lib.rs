//! OpenGL ES 1.0 (perfil Common) y EGL 1.0 para PlayStation 2.
//!
//! Las funciones tienen los nombres y las firmas de la especificación de
//! Khronos, así que el código de GLES 1.0 se traduce casi línea a línea:
//!
//! ```ignore
//! use ps2_gles::*;
//!
//! unsafe {
//!     let display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
//!     eglInitialize(display, core::ptr::null_mut(), core::ptr::null_mut());
//!     let attributes = [EGL_DEPTH_SIZE, 16, EGL_NONE];
//!     let (mut config, mut count) = (0, 0);
//!     eglChooseConfig(display, attributes.as_ptr(), &mut config, 1, &mut count);
//!     let surface = eglCreateWindowSurface(display, config, PS2_WINDOW_NTSC, core::ptr::null());
//!     let context = eglCreateContext(display, config, EGL_NO_CONTEXT, core::ptr::null());
//!     eglMakeCurrent(display, surface, surface, context);
//!
//!     glClearColor(0.1, 0.2, 0.3, 1.0);
//!     glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
//!     // glVertexPointer, glDrawArrays…
//!     eglSwapBuffers(display, surface);
//! }
//! ```
//!
//! El estado y las matrices los lleva el Emotion Engine; la geometría de los
//! triángulos (transformación, iluminación, niebla, recorte) la hace la
//! unidad vectorial VU1 con un microprograma propio ([`vu1prog`]), y el
//! Graphics Synthesizer rasteriza. Lo que la VU1 no cubre (brillo especular,
//! focos, niebla exponencial, puntos y líneas) lo calcula el Emotion Engine. Lo que el GS no puede
//! hacer está documentado en [`gs`](crate::gs) y en el README.
//!
//! El crate también compila en el PC, con un rasterizador por software que
//! sigue la especificación: es la referencia contra la que se comprueba la
//! PS2.

#![cfg_attr(target_arch = "mips", no_std)]
#![allow(static_mut_refs)]

extern crate alloc;

mod api;
pub mod blend;
mod consts;
pub mod context;
mod egl;
mod fixed;
#[cfg(target_arch = "mips")]
pub mod gs;
pub mod math;
mod pipeline;
pub mod raster;
#[cfg(not(target_arch = "mips"))]
pub mod soft;
pub mod texture;
pub mod vu;
#[cfg(target_arch = "mips")]
mod vu1;
pub mod vu1prog;
#[cfg(not(target_arch = "mips"))]
pub mod vuinterp;
#[cfg(not(target_arch = "mips"))]
pub mod vusoft;
#[cfg(all(test, not(target_arch = "mips")))]
mod vutests;

pub use api::*;
pub use consts::*;
pub use context::fast_triangles;
pub use egl::*;
pub use fixed::*;

//! Estado de un contexto de OpenGL ES 1.0.

use crate::consts::*;
use crate::math::{self, Mat4, IDENTITY};
use crate::raster::{self, Compare, Factor, Filter, Rasterizer, TexEnv, TexFormat, Wrap};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

pub const MAX_LIGHTS: usize = 8;
pub const MODELVIEW_DEPTH: usize = 16;
pub const PROJECTION_DEPTH: usize = 2;
pub const TEXTURE_DEPTH: usize = 2;
pub const MAX_TEXTURE_SIZE: u32 = 512;
/// Niveles de mipmap que caben en una textura del tamaño máximo.
pub const MAX_LEVELS: usize = 10;
/// Niveles que el rasterizador puede usar a la vez (límite del GS).
pub const USABLE_LEVELS: u32 = 7;

#[derive(Clone, Copy)]
pub struct Light {
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    /// En coordenadas de ojo.
    pub position: [f32; 4],
    pub spot_direction: [f32; 3],
    pub spot_exponent: f32,
    pub spot_cutoff: f32,
    pub attenuation: [f32; 3],
    pub enabled: bool,
}

#[derive(Clone, Copy)]
pub struct Material {
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub emission: [f32; 4],
    pub shininess: f32,
}

/// Un array de atributos de vértice (`glVertexPointer` y compañía).
#[derive(Clone, Copy)]
pub struct Array {
    pub enabled: bool,
    pub size: i32,
    pub kind: GLenum,
    pub stride: i32,
    pub pointer: *const u8,
}

pub const ARRAY_VERTEX: usize = 0;
pub const ARRAY_NORMAL: usize = 1;
pub const ARRAY_COLOR: usize = 2;
pub const ARRAY_TEXCOORD: usize = 3;

#[derive(Clone, Copy, PartialEq)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub format: TexFormat,
}

pub struct Texture {
    pub levels: [Option<Level>; MAX_LEVELS],
    pub min: Filter,
    pub mag: Filter,
    pub wrap_s: Wrap,
    pub wrap_t: Wrap,
}

impl Texture {
    pub fn new() -> Texture {
        Texture { levels: [None; MAX_LEVELS], min: Filter::NearestMipLinear, mag: Filter::Linear, wrap_s: Wrap::Repeat, wrap_t: Wrap::Repeat }
    }

    /// Niveles utilizables, o 0 si la textura está incompleta (entonces GL
    /// dibuja como si la textura estuviera desactivada).
    pub fn usable_levels(&self) -> u32 {
        let Some(base) = self.levels[0] else { return 0 };
        if matches!(self.min, Filter::Nearest | Filter::Linear) {
            return 1;
        }
        // Con mipmaps hace falta la pirámide entera, hasta 1×1.
        let (mut w, mut h, mut count) = (base.width, base.height, 1);
        while w > 1 || h > 1 {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            match self.levels.get(count as usize).copied().flatten() {
                Some(l) if l.width == w && l.height == h && l.format == base.format => count += 1,
                _ => return 0,
            }
        }
        count.min(USABLE_LEVELS)
    }
}

pub struct Context {
    pub raster: Box<dyn Rasterizer>,
    pub error: GLenum,

    pub matrix_mode: GLenum,
    pub modelview: Vec<Mat4>,
    pub projection: Vec<Mat4>,
    pub texture_matrix: Vec<Mat4>,
    pub viewport: [i32; 4],
    pub depth_range: [f32; 2],

    pub color: [f32; 4],
    pub normal: [f32; 3],
    pub texcoord: [f32; 4],
    pub arrays: [Array; 4],

    pub lighting: bool,
    pub lights: [Light; MAX_LIGHTS],
    pub material: Material,
    pub light_model_ambient: [f32; 4],
    pub two_side: bool,
    pub color_material: bool,
    pub normalize: bool,
    pub rescale_normal: bool,
    pub smooth: bool,

    pub fog: bool,
    pub fog_mode: GLenum,
    pub fog_density: f32,
    pub fog_start: f32,
    pub fog_end: f32,
    pub fog_color: [f32; 4],

    pub cull_face: bool,
    pub cull_mode: GLenum,
    pub front_face: GLenum,
    pub alpha_test: bool,
    pub alpha_func: Compare,
    pub alpha_ref: f32,
    pub blend: bool,
    pub blend_src: Factor,
    pub blend_dst: Factor,
    pub depth_test: bool,
    pub depth_func: Compare,
    pub depth_mask: bool,
    pub color_mask: [bool; 4],
    pub scissor_test: bool,
    pub scissor: [i32; 4],
    pub clear_color: [f32; 4],
    pub clear_depth: f32,
    pub line_width: f32,
    pub point_size: f32,
    pub polygon_offset_fill: bool,
    pub polygon_offset: [f32; 2],
    /// Capacidades que se aceptan y se recuerdan pero no cambian el dibujo en
    /// esta implementación (stencil de 0 bits, tramado, multimuestreo…).
    pub inert_caps: BTreeMap<GLenum, bool>,

    pub texture_2d: bool,
    pub textures: BTreeMap<GLuint, Texture>,
    pub bound_texture: GLuint,
    pub next_texture: GLuint,
    pub tex_env: TexEnv,
    pub tex_env_color: [f32; 4],
    pub pack_alignment: i32,
    pub unpack_alignment: i32,
}

impl Context {
    pub fn new(raster: Box<dyn Rasterizer>) -> Context {
        let (w, h) = raster.size();
        let mut lights = [Light {
            ambient: [0.0, 0.0, 0.0, 1.0],
            diffuse: [0.0, 0.0, 0.0, 1.0],
            specular: [0.0, 0.0, 0.0, 1.0],
            position: [0.0, 0.0, 1.0, 0.0],
            spot_direction: [0.0, 0.0, -1.0],
            spot_exponent: 0.0,
            spot_cutoff: 180.0,
            attenuation: [1.0, 0.0, 0.0],
            enabled: false,
        }; MAX_LIGHTS];
        lights[0].diffuse = [1.0; 4];
        lights[0].specular = [1.0; 4];
        let mut textures = BTreeMap::new();
        textures.insert(0, Texture::new());
        let mut inert_caps = BTreeMap::new();
        for cap in [GL_STENCIL_TEST, GL_COLOR_LOGIC_OP, GL_POINT_SMOOTH, GL_LINE_SMOOTH, GL_SAMPLE_ALPHA_TO_COVERAGE, GL_SAMPLE_ALPHA_TO_ONE, GL_SAMPLE_COVERAGE] {
            inert_caps.insert(cap, false);
        }
        inert_caps.insert(GL_DITHER, true);
        inert_caps.insert(GL_MULTISAMPLE, true);
        let array = |size, kind| Array { enabled: false, size, kind, stride: 0, pointer: core::ptr::null() };
        Context {
            raster,
            error: GL_NO_ERROR,
            matrix_mode: GL_MODELVIEW,
            modelview: vec![IDENTITY],
            projection: vec![IDENTITY],
            texture_matrix: vec![IDENTITY],
            viewport: [0, 0, w as i32, h as i32],
            depth_range: [0.0, 1.0],
            color: [1.0; 4],
            normal: [0.0, 0.0, 1.0],
            texcoord: [0.0, 0.0, 0.0, 1.0],
            arrays: [array(4, GL_FLOAT), array(3, GL_FLOAT), array(4, GL_FLOAT), array(4, GL_FLOAT)],
            lighting: false,
            lights,
            material: Material {
                ambient: [0.2, 0.2, 0.2, 1.0],
                diffuse: [0.8, 0.8, 0.8, 1.0],
                specular: [0.0, 0.0, 0.0, 1.0],
                emission: [0.0, 0.0, 0.0, 1.0],
                shininess: 0.0,
            },
            light_model_ambient: [0.2, 0.2, 0.2, 1.0],
            two_side: false,
            color_material: false,
            normalize: false,
            rescale_normal: false,
            smooth: true,
            fog: false,
            fog_mode: GL_EXP,
            fog_density: 1.0,
            fog_start: 0.0,
            fog_end: 1.0,
            fog_color: [0.0; 4],
            cull_face: false,
            cull_mode: GL_BACK,
            front_face: GL_CCW,
            alpha_test: false,
            alpha_func: Compare::Always,
            alpha_ref: 0.0,
            blend: false,
            blend_src: Factor::One,
            blend_dst: Factor::Zero,
            depth_test: false,
            depth_func: Compare::Less,
            depth_mask: true,
            color_mask: [true; 4],
            scissor_test: false,
            scissor: [0, 0, w as i32, h as i32],
            clear_color: [0.0; 4],
            clear_depth: 1.0,
            line_width: 1.0,
            point_size: 1.0,
            polygon_offset_fill: false,
            polygon_offset: [0.0, 0.0],
            inert_caps,
            texture_2d: false,
            textures,
            bound_texture: 0,
            next_texture: 1,
            tex_env: TexEnv::Modulate,
            tex_env_color: [0.0; 4],
            pack_alignment: 4,
            unpack_alignment: 4,
        }
    }

    /// Anota un error (GL conserva el primero hasta que se consulta).
    pub fn set_error(&mut self, error: GLenum) {
        if self.error == GL_NO_ERROR {
            self.error = error;
        }
    }

    /// Pila de matrices seleccionada con `glMatrixMode` y su profundidad máxima.
    pub fn stack(&mut self) -> (&mut Vec<Mat4>, usize) {
        match self.matrix_mode {
            GL_PROJECTION => (&mut self.projection, PROJECTION_DEPTH),
            GL_TEXTURE => (&mut self.texture_matrix, TEXTURE_DEPTH),
            _ => (&mut self.modelview, MODELVIEW_DEPTH),
        }
    }

    /// Multiplica la matriz actual por `m` (por la derecha).
    pub fn mult(&mut self, m: &Mat4) {
        let top = self.stack().0.last_mut().unwrap();
        *top = math::mul(top, m);
    }

    /// Variable de una capacidad de `glEnable`/`glDisable`.
    pub fn cap(&mut self, cap: GLenum) -> Option<&mut bool> {
        Some(match cap {
            GL_LIGHTING => &mut self.lighting,
            GL_FOG => &mut self.fog,
            GL_TEXTURE_2D => &mut self.texture_2d,
            GL_CULL_FACE => &mut self.cull_face,
            GL_ALPHA_TEST => &mut self.alpha_test,
            GL_BLEND => &mut self.blend,
            GL_DEPTH_TEST => &mut self.depth_test,
            GL_SCISSOR_TEST => &mut self.scissor_test,
            GL_COLOR_MATERIAL => &mut self.color_material,
            GL_NORMALIZE => &mut self.normalize,
            GL_RESCALE_NORMAL => &mut self.rescale_normal,
            GL_POLYGON_OFFSET_FILL => &mut self.polygon_offset_fill,
            GL_LIGHT0..=GL_LIGHT7 => &mut self.lights[(cap - GL_LIGHT0) as usize].enabled,
            _ => return self.inert_caps.get_mut(&cap),
        })
    }

    /// Rectángulo de recorte en coordenadas del rasterizador (origen arriba),
    /// limitado a la ventana; `None` si la prueba está desactivada.
    pub fn raster_scissor(&self) -> Option<[i32; 4]> {
        if !self.scissor_test {
            return None;
        }
        let (w, h) = self.raster.size();
        let (w, h) = (w as i32, h as i32);
        let [x, y, sw, sh] = self.scissor;
        let x0 = x.clamp(0, w);
        let x1 = x.saturating_add(sw).clamp(0, w);
        let y0 = (h - y.saturating_add(sh)).clamp(0, h);
        let y1 = (h - y).clamp(0, h);
        Some([x0, y0, x1.max(x0), y1.max(y0)])
    }

    /// Estado por fragmento que corresponde al estado actual de GL.
    pub fn raster_state(&self) -> raster::State {
        let texture = if self.texture_2d {
            self.textures.get(&self.bound_texture).and_then(|t| {
                let levels = t.usable_levels();
                let base = t.levels[0]?;
                (levels > 0).then(|| raster::TextureState {
                    id: self.bound_texture,
                    format: base.format,
                    env: self.tex_env,
                    env_color: self.tex_env_color,
                    min: t.min,
                    mag: t.mag,
                    wrap_s: t.wrap_s,
                    wrap_t: t.wrap_t,
                    levels,
                    width: base.width,
                    height: base.height,
                })
            })
        } else {
            None
        };
        raster::State {
            texture,
            blend: (self.blend && (self.blend_src, self.blend_dst) != (Factor::One, Factor::Zero)).then_some((self.blend_src, self.blend_dst)),
            alpha_test: (self.alpha_test && self.alpha_func != Compare::Always).then_some((self.alpha_func, self.alpha_ref)),
            depth_test: self.depth_test.then_some(self.depth_func),
            depth_write: self.depth_mask,
            color_mask: self.color_mask,
            scissor: self.raster_scissor(),
            fog_color: self.fog.then_some([self.fog_color[0], self.fog_color[1], self.fog_color[2]]),
        }
    }
}

static mut CURRENT: *mut Context = core::ptr::null_mut();

/// Contexto actual (el de `eglMakeCurrent`). Como en GL, es estado global:
/// las funciones `gl*` deben llamarse desde un solo hilo.
pub fn current() -> Option<&'static mut Context> {
    unsafe { CURRENT.as_mut() }
}

pub(crate) fn set_current(context: *mut Context) {
    unsafe { CURRENT = context };
}

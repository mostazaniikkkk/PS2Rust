//! Frontera entre el núcleo de GL (estado, transformación, iluminación,
//! recorte) y el rasterizador. Hay dos rasterizadores: el Graphics
//! Synthesizer (`gs.rs`, en la PS2) y uno por software (`soft.rs`, en el PC)
//! que sigue la especificación al pie de la letra y sirve de referencia.

/// Vértice ya transformado a coordenadas de ventana.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    /// Coordenadas de ventana con el origen **arriba a la izquierda** (la `y`
    /// de GL ya invertida). Los centros de píxel están en `.5`.
    pub x: f32,
    pub y: f32,
    /// Profundidad de 0 (cerca) a 1 (lejos).
    pub z: f32,
    /// 1/w, para la interpolación con corrección de perspectiva.
    pub q: f32,
    /// Coordenadas de textura (sin dividir por w). `t = 0` es la primera fila
    /// de la imagen.
    pub s: f32,
    pub t: f32,
    /// Color RGBA entre 0 y 1.
    pub color: [f32; 4],
    /// Factor de niebla: 1 = sin niebla, 0 = todo color de niebla.
    pub fog: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    Never,
    Less,
    Equal,
    Lequal,
    Greater,
    Notequal,
    Gequal,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Factor {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    DstColor,
    OneMinusDstColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    SrcAlphaSaturate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexEnv {
    Modulate,
    Decal,
    Blend,
    Replace,
    Add,
}

/// Formato base de la textura. Los texels llegan siempre como RGBA de 8 bits:
/// `Alpha` = (255, 255, 255, A), `Luminance` = (L, L, L, 255),
/// `LuminanceAlpha` = (L, L, L, A), `Rgb` = (R, G, B, 255).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexFormat {
    Alpha,
    Luminance,
    LuminanceAlpha,
    Rgb,
    Rgba,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Nearest,
    Linear,
    NearestMipNearest,
    LinearMipNearest,
    NearestMipLinear,
    LinearMipLinear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    Repeat,
    ClampToEdge,
}

/// Textura con la que se dibuja.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureState {
    /// Identificador con el que se subieron sus niveles (`tex_image`).
    pub id: u32,
    pub format: TexFormat,
    pub env: TexEnv,
    pub env_color: [f32; 4],
    pub min: Filter,
    pub mag: Filter,
    pub wrap_s: Wrap,
    pub wrap_t: Wrap,
    /// Niveles de mipmap utilizables (1 = sólo el nivel 0).
    pub levels: u32,
    /// Tamaño del nivel 0.
    pub width: u32,
    pub height: u32,
}

/// Estado por fragmento. Vale para todas las primitivas hasta el siguiente
/// `set_state`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub texture: Option<TextureState>,
    /// Factores de origen y destino; `None` = sin mezcla.
    pub blend: Option<(Factor, Factor)>,
    /// Función y valor de referencia (0 a 1); `None` = sin prueba alfa.
    pub alpha_test: Option<(Compare, f32)>,
    /// `None` = prueba de profundidad desactivada (pasa siempre y no escribe).
    pub depth_test: Option<Compare>,
    /// Escribir la profundidad de los fragmentos que pasan.
    pub depth_write: bool,
    /// Canales de color que se escriben (R, G, B, A).
    pub color_mask: [bool; 4],
    /// Rectángulo de recorte `[x0, y0, x1, y1)` con el origen arriba a la
    /// izquierda, ya limitado a la ventana.
    pub scissor: Option<[i32; 4]>,
    /// Color de la niebla; `None` = sin niebla.
    pub fog_color: Option<[f32; 3]>,
}

impl State {
    pub const DEFAULT: State = State {
        texture: None,
        blend: None,
        alpha_test: None,
        depth_test: None,
        depth_write: true,
        color_mask: [true; 4],
        scissor: None,
        fog_color: None,
    };
}

/// Lo que el núcleo de GL pide al rasterizador.
pub trait Rasterizer {
    /// Ancho y alto de la ventana en píxeles.
    fn size(&self) -> (u32, u32);
    /// Bits del búfer de profundidad.
    fn depth_bits(&self) -> u32;
    fn set_state(&mut self, state: &State);
    /// Función de profundidad vigente, avisada antes de un `clear`. El GS la
    /// necesita para decidir en qué sentido guarda la profundidad; el
    /// rasterizador por software no.
    fn depth_func_hint(&mut self, _func: Compare) {}
    /// Punto de un píxel.
    fn point(&mut self, v: &Vertex);
    /// Línea de un píxel de ancho.
    fn line(&mut self, a: &Vertex, b: &Vertex);
    fn triangle(&mut self, a: &Vertex, b: &Vertex, c: &Vertex);
    /// Rectángulo `[x0, x1) × [y0, y1)` (coordenadas de ventana) con el
    /// color, la profundidad, la niebla y las coordenadas de textura de `v`
    /// en toda su superficie. Para los puntos de más de un píxel.
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, v: &Vertex);
    /// Limpia el color y/o la profundidad. Sólo respeta el rectángulo de
    /// recorte y la máscara de color indicados, no el resto del estado.
    fn clear(&mut self, color: Option<[f32; 4]>, depth: Option<f32>, color_mask: [bool; 4], scissor: Option<[i32; 4]>);
    /// Lee `w`×`h` píxeles RGBA de 8 bits desde (`x`, `y`), origen arriba a
    /// la izquierda, fila superior primero. El rectángulo está dentro de la
    /// ventana.
    fn read_pixels(&mut self, x: u32, y: u32, w: u32, h: u32, out: &mut [u8]);
    /// Define un nivel de una textura (sustituye al anterior). `rgba` tiene
    /// `w`·`h`·4 bytes, con la fila de `t = 0` primero. Devuelve `false` si
    /// no queda memoria de vídeo.
    fn tex_image(&mut self, id: u32, level: u32, w: u32, h: u32, rgba: &[u8]) -> bool;
    /// Sustituye un rectángulo de un nivel ya definido.
    fn tex_sub_image(&mut self, id: u32, level: u32, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]);
    /// Libera todos los niveles de una textura.
    fn tex_delete(&mut self, id: u32);
    /// Termina de dibujar lo pendiente.
    fn finish(&mut self);
    /// Presenta el frame y pasa a dibujar en el otro búfer.
    fn swap(&mut self);
    /// Camino rápido: dibuja `count / 3` triángulos con el estado de
    /// `set_state`, haciendo el rasterizador toda la geometría. Los vértices
    /// los pide a `fill`, que rellena en orden los trozos que se le pasan
    /// (cada uno con un número de vértices múltiplo de 3). Devuelve `false`,
    /// sin llamar a `fill` ni dibujar nada, si no puede.
    fn fast_triangles(&mut self, _setup: &FastSetup, _count: usize, _fill: &mut dyn FnMut(&mut [f32])) -> bool {
        false
    }
    /// Geometría retenida: guarda `count / 3` triángulos en el formato del
    /// camino rápido (con `units` matrices por vértice, 0 = sin *skinning*)
    /// para dibujarlos después cuantas veces haga falta sin volver a leer
    /// los vértices. Devuelve su identificador, o `None` si el rasterizador
    /// no retiene geometría (entonces no llama a `fill`).
    fn fast_retain(&mut self, _units: usize, _count: usize, _fill: &mut dyn FnMut(&mut [f32])) -> Option<u32> {
        None
    }
    /// Dibuja geometría retenida con el estado de `set_state` y las
    /// constantes de `setup`. Devuelve `false`, sin dibujar, si no puede.
    fn fast_retained(&mut self, _setup: &FastSetup, _id: u32) -> bool {
        false
    }
    /// Libera geometría retenida.
    fn fast_release(&mut self, _id: u32) {}
    /// Triángulos dibujados por el camino rápido desde el arranque.
    fn fast_triangle_count(&self) -> u64 {
        0
    }
}

/// Una luz para el camino rápido: direccional o puntual, sin foco.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FastLight {
    /// Dirección hacia la luz (unitaria) o, si es puntual, su posición; en
    /// coordenadas de ojo.
    pub vector: [f32; 3],
    pub positional: bool,
    /// Términos ambiente y difuso, ya multiplicados por el material (o sin
    /// multiplicar, si el material es el color del vértice).
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    /// Atenuación constante, lineal y cuadrática (sólo puntuales).
    pub attenuation: [f32; 3],
}

/// Todo lo que es constante durante una llamada de dibujo de triángulos que
/// puede resolver entera el rasterizador (en la PS2, la unidad vectorial
/// VU1): transformación, iluminación, niebla, recorte y descarte de caras.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FastSetup<'a> {
    /// Proyección · modelview, por columnas.
    pub mvp: [f32; 16],
    /// Modelview (afín), por columnas.
    pub modelview: [f32; 16],
    /// Columnas de la matriz de normales (con el reescalado ya aplicado).
    pub normal: [[f32; 3]; 3],
    pub viewport: [i32; 4],
    pub depth_range: [f32; 2],
    pub lighting: bool,
    /// Normalizar la normal tras transformarla.
    pub normalize: bool,
    pub lights: [FastLight; 8],
    pub light_count: usize,
    /// Color = `base + color del vértice · (scene + Σ luces)`.
    pub base: [f32; 3],
    pub scene: [f32; 3],
    /// Niebla lineal: `(fin, 1 / (fin - inicio))`.
    pub fog: Option<(f32, f32)>,
    /// Descarte de caras: se dibuja si `área · cull > 0` (con el área
    /// positiva para los triángulos antihorarios). 0 = no se descarta nada.
    pub cull: f32,
    /// Sombreado plano: manda el color del tercer vértice.
    pub flat: bool,
    /// Proyección sola, por columnas (para el *skinning*).
    pub projection: [f32; 16],
    /// *Skinning*: matrices que se mezclan por vértice (de 1 a 4), o 0 si
    /// todos los vértices usan la modelview.
    pub skin: usize,
    /// Paleta de matrices del *skinning* (modelviews afines, por columnas).
    pub palette: &'a [[f32; 16]],
    /// Color común a todos los vértices; entonces se ignora el de cada uno.
    pub const_color: Option<[f32; 4]>,
}

/// Matrices de la paleta del camino rápido.
pub const FAST_PALETTE_SIZE: usize = 32;
/// Los índices de matriz de un vértice van codificados como enteros:
/// `FAST_PALETTE_BASE + 4 · índice` (su dirección en la memoria de la VU1).
pub const FAST_PALETTE_BASE: u32 = 64;

/// Números de coma flotante por vértice de entrada del camino rápido:
/// posición (x, y, z) y `s`; normal (x, y, z) y `t`; color (r, g, b, a). Con
/// *skinning* siguen cuatro pesos y cuatro índices de matriz codificados (los
/// bits de un entero, ver [`FAST_PALETTE_BASE`]).
pub const fn fast_vertex_floats(units: usize) -> usize {
    if units > 0 {
        20
    } else {
        12
    }
}

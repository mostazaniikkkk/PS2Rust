//! El microprograma de la VU1 que hace la geometría de OpenGL ES.
//!
//! Recibe triángulos sueltos (tres vértices con posición, normal, color y
//! coordenadas de textura) y, por cada uno:
//!
//! 1. transforma los vértices a coordenadas de recorte y, si hace falta, de
//!    ojo; calcula la niebla lineal y la iluminación (luces direccionales y
//!    puntuales con atenuación);
//! 2. descarta la cara si mira hacia donde no debe;
//! 3. si el triángulo se sale del volumen de visión (o de la banda de guarda
//!    del GS, en x e y), lo recorta contra los seis planos;
//! 4. pasa lo que queda a coordenadas de ventana y lo deja en un paquete para
//!    el GS, que envía con XGKICK cuando se llena.
//!
//! El programa se genera a medida para cada combinación de estado ([`Key`]):
//! sin iluminación no hay código de luces, cada luz lleva el código de su
//! tipo, etc. Así dentro no hay ni una comprobación de modo.

use crate::raster::FastSetup;
use crate::vu::*;
use alloc::vec::Vec;

// Memoria de datos de la VU1 (direcciones en palabras de 128 bits).
pub const C_MVP: usize = 0;
pub const C_MODELVIEW: usize = 4;
pub const C_NORMAL: usize = 8;
pub const C_SCALE: usize = 11;
pub const C_OFFSET: usize = 12;
pub const C_GUARD: usize = 13;
pub const C_COLOR_SCALE: usize = 14;
pub const C_MISC: usize = 15;
pub const C_BASE: usize = 16;
pub const C_SCENE: usize = 17;
pub const C_FOG: usize = 18;
pub const C_CULL: usize = 19;
pub const C_TAG: usize = 20;
pub const C_TEX1: usize = 21;
pub const C_PLANES: usize = 22;
pub const C_LIGHTS: usize = 28;
pub const C_LOD: usize = 60;
/// Fin de las constantes de una llamada de dibujo.
pub const CONSTANTS: usize = 62;
/// Enteros de cada tanda: (triángulos, 0x8000, 0, 0).
pub const C_COUNT: usize = 62;
/// Vértices de entrada: tres palabras por vértice.
pub const INPUT: usize = 64;
/// Triángulos por tanda.
pub const BATCH: usize = 40;
const WORK: usize = INPUT + BATCH * 9;
const CLIP_A: usize = WORK + 12;
const CLIP_B: usize = CLIP_A + 30;
const OUT_A: usize = 500;
const OUT_B: usize = 762;
/// Triángulos con los que se envía el paquete: caben 25 y un triángulo
/// recortado puede dar hasta 7.
const KICK_AT: u32 = 18;

/// Lo que cambia el código del microprograma.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub lighting: bool,
    pub light_count: u8,
    /// Un bit por luz: es puntual.
    pub positional: u8,
    pub normalize: bool,
    pub fog: bool,
    pub flat: bool,
    /// Calcular el nivel de detalle de la textura por triángulo.
    pub lod: bool,
    /// Sin descarte de caras (sólo se quitan los triángulos sin área).
    pub no_cull: bool,
}

/// Lo que el rasterizador sabe y el núcleo de GL no.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    /// Alto de la ventana en píxeles.
    pub height: f32,
    /// Origen de la ventana en el espacio de coordenadas del GS.
    pub origin: f32,
    /// Mayor valor de profundidad.
    pub depth_max: f32,
    /// Profundidad invertida (cerca = valor alto).
    pub inverted: bool,
    /// 255 sin textura; 128 con ella (el GS multiplica por dos).
    pub color_scale: f32,
    /// Valor de PRIM para los triángulos.
    pub prim: u32,
    /// Con nivel de detalle por triángulo: (TEX1 sin K, texels del nivel 0).
    pub lod: Option<(u32, f32)>,
}

impl Key {
    pub fn of(setup: &FastSetup, target: &Target) -> Key {
        let mut positional = 0;
        for (i, light) in setup.lights[..setup.light_count].iter().enumerate() {
            positional |= (light.positional as u8) << i;
        }
        Key {
            lighting: setup.lighting,
            light_count: if setup.lighting { setup.light_count as u8 } else { 0 },
            positional: if setup.lighting { positional } else { 0 },
            normalize: setup.lighting && setup.normalize,
            fog: setup.fog.is_some(),
            flat: setup.flat,
            lod: target.lod.is_some(),
            no_cull: setup.cull == 0.0,
        }
    }

    fn needs_eye(&self) -> bool {
        self.fog || self.positional != 0
    }
}

// Registros de coma flotante con constantes.
const MVP: u32 = 1; // ..4
const SCALE: u32 = 5;
const OFFSET: u32 = 6;
const GUARD: u32 = 7;
const ZERO: u32 = 8;
const COLOR_SCALE: u32 = 9;
/// (0.5, BIG, profundidad máxima, 255)
const MISC: u32 = 10;
/// Temporal de las comparaciones.
const T: u32 = 31;

// Registros enteros.
const FLAG: u32 = 1;
const IN: u32 = 2;
const TRIS: u32 = 3;
const OUT: u32 = 4;
const OUT_BASE: u32 = 5;
const OUT_COUNT: u32 = 6;
const PA: u32 = 8;
const PB: u32 = 9;
const AUX: u32 = 10;
const RET_EMIT: u32 = 11;
const WORK_PTR: u32 = 12;
const PC: u32 = 13;
const EOP: u32 = 14;
const RET: u32 = 15;

/// `vi = 1` si `src.x > 0` (con margen), si no 0. Usa [`T`].
fn positive(a: &mut Asm, src: u32, vi: u32) {
    a.u(mulbc(X, T, src, MISC, BY));
    a.u(maxbc(X, T, T, ZERO, BX));
    a.u(minibc(X, T, T, 0, BW));
    a.u(ftoi0(X, T, T));
    a.l(mtir(vi, T, BX));
}

/// `dst.x = a · b` con los tres primeros componentes.
fn dot3(a: &mut Asm, dst: u32, x: u32, y: u32) {
    a.u(mul(XYZ, dst, x, y));
    a.u(addbc(X, dst, dst, dst, BY));
    a.u(addbc(X, dst, dst, dst, BZ));
}

/// Copia `words` palabras de `from(src)` a `to(dst)`.
fn copy(a: &mut Asm, words: i32, from: i32, src: u32, to: i32, dst: u32) {
    for i in 0..words {
        a.l(lq(XYZW, 11, from + i, src));
        a.l(sq(XYZW, 11, to + i, dst));
    }
}

/// Genera el microprograma para `key`.
pub fn build(key: &Key) -> Vec<u64> {
    let mut a = Asm::new();
    let (transform, emit, kick, clip_plane, next_triangle, triangle, done) = (a.label(), a.label(), a.label(), a.label(), a.label(), a.label(), a.label());
    // Palabras por triángulo en el paquete de salida.
    let record = if key.lod { 10 } else { 9 };

    // ------------------------------------------------------------ arranque
    for i in 0..4 {
        a.l(lq(XYZW, MVP + i, (C_MVP + i as usize) as i32, 0));
    }
    a.l(lq(XYZW, SCALE, C_SCALE as i32, 0));
    a.l(lq(XYZW, OFFSET, C_OFFSET as i32, 0));
    a.l(lq(XYZW, GUARD, C_GUARD as i32, 0));
    a.l(lq(XYZW, COLOR_SCALE, C_COLOR_SCALE as i32, 0));
    a.l(lq(XYZW, MISC, C_MISC as i32, 0));
    a.u(sub(XYZW, ZERO, 0, 0));
    a.l(ilw(X, TRIS, C_COUNT as i32, 0));
    a.l(ilw(Y, EOP, C_COUNT as i32, 0));
    a.l(iaddiu(IN, 0, INPUT as u32));
    a.l(iaddiu(OUT_BASE, 0, OUT_A as u32));
    // Paquete de salida vacío: su etiqueta y nada más.
    a.l(lq(XYZW, 11, C_TAG as i32, 0));
    a.l(sq(XYZW, 11, 0, OUT_BASE));
    a.l(iaddiu(OUT, OUT_BASE, 1));
    a.l(iaddiu(OUT_COUNT, 0, 0));
    a.branch(Branch::Equal(TRIS, 0), done, true);

    // ------------------------------------------------------------ por triángulo
    a.place(triangle);
    a.l(iaddiu(WORK_PTR, 0, WORK as u32));
    for _ in 0..3 {
        a.call(AUX, transform);
    }
    if key.flat {
        // Manda el color del tercer vértice.
        a.l(lq(XYZW, 11, WORK as i32 + 7, 0));
        a.l(sq(XYZW, 11, WORK as i32 + 1, 0));
        a.l(sq(XYZW, 11, WORK as i32 + 4, 0));
    }
    // Orientación: signo del determinante de (x, y, w) de los tres vértices,
    // que es el del área en la ventana aunque alguno quede detrás del ojo.
    for i in 0..3u32 {
        a.l(lq(XYZW, 11, WORK as i32 + 3 * i as i32, 0));
        a.u(add(XY, 12 + i, 11, ZERO));
        a.u(addbc(Z, 12 + i, ZERO, 11, BW));
    }
    a.u(opmula(13, 14));
    a.u(opmsub(15, 14, 13));
    dot3(&mut a, 16, 12, 15);
    if key.no_cull {
        a.u(abs(X, 16, 16));
    } else {
        a.l(lq(XYZW, 17, C_CULL as i32, 0));
        a.u(mul(X, 16, 16, 17));
    }
    positive(&mut a, 16, FLAG);
    a.branch(Branch::Equal(FLAG, 0), next_triangle, true);
    // ¿Algún vértice fuera de algún plano? (Los CLIP se hicieron al
    // transformar; ya han pasado de sobra las 4 instrucciones de espera.)
    let clipped = a.label();
    let emitted = a.label();
    a.l(fcand(0x3_FFFF));
    a.branch(Branch::NotEqual(FLAG, 0), clipped, true);
    a.l(iaddiu(PA, 0, WORK as u32));
    a.l(iaddiu(PB, 0, WORK as u32 + 3));
    a.l(iaddiu(PC, 0, WORK as u32 + 6));
    a.call(RET_EMIT, emit);
    a.branch(Branch::Always, emitted, false);

    // Recorte contra los seis planos, alternando entre dos zonas de trabajo.
    a.place(clipped);
    copy(&mut a, 9, WORK as i32, 0, CLIP_A as i32, 0);
    a.l(iaddiu(PC, 0, 3));
    for plane in 0..6 {
        let (src, dst) = if plane % 2 == 0 { (CLIP_A, CLIP_B) } else { (CLIP_B, CLIP_A) };
        a.l(lq(XYZW, 20, (C_PLANES + plane) as i32, 0));
        a.l(iaddiu(PA, 0, src as u32));
        a.l(iaddiu(PB, 0, dst as u32));
        a.call(RET, clip_plane);
        // Con menos de tres vértices no queda nada.
        a.l(isubiu(FLAG, PC, 3));
        a.branch(Branch::Negative(FLAG), next_triangle, true);
    }
    // El polígono resultante (en CLIP_A) sale como abanico de triángulos.
    let fan = a.label();
    a.l(isubiu(WORK_PTR, PC, 2));
    a.l(iaddiu(PA, 0, CLIP_A as u32));
    a.l(iaddiu(PB, 0, CLIP_A as u32 + 3));
    a.l(iaddiu(PC, 0, CLIP_A as u32 + 6));
    a.place(fan);
    a.call(RET_EMIT, emit);
    a.l(iaddiu(PB, PB, 3));
    a.l(iaddiu(PC, PC, 3));
    a.l(isubiu(WORK_PTR, WORK_PTR, 1));
    a.branch(Branch::NotEqual(WORK_PTR, 0), fan, true);

    a.place(emitted);
    // Si el paquete va lleno, al GS.
    a.l(isubiu(FLAG, OUT_COUNT, KICK_AT));
    a.branch(Branch::Negative(FLAG), next_triangle, true);
    a.call(RET, kick);

    a.place(next_triangle);
    a.l(isubiu(TRIS, TRIS, 1));
    a.branch(Branch::NotEqual(TRIS, 0), triangle, true);

    a.place(done);
    let finished = a.label();
    a.branch(Branch::Equal(OUT_COUNT, 0), finished, false);
    a.call(RET, kick);
    a.place(finished);
    a.end();

    // ------------------------------------------------------------ enviar el paquete
    a.place(kick);
    a.l(ior(FLAG, OUT_COUNT, EOP));
    a.l(isw(X, FLAG, 0, OUT_BASE));
    a.l(xgkick(OUT_BASE));
    // Se sigue en la otra zona de salida mientras el GS lee ésta.
    a.l(iaddiu(FLAG, 0, (OUT_A + OUT_B) as u32));
    a.l(isub(OUT_BASE, FLAG, OUT_BASE));
    a.l(lq(XYZW, 11, C_TAG as i32, 0));
    a.l(sq(XYZW, 11, 0, OUT_BASE));
    a.l(iaddiu(OUT, OUT_BASE, 1));
    a.l(iaddiu(OUT_COUNT, 0, 0));
    a.ret(RET);

    // ------------------------------------------------------------ transformar un vértice
    // Entrada en IN (3 palabras); salida en WORK_PTR: recorte, color y
    // (s, t, niebla). Avanza los dos punteros.
    a.place(transform);
    let (pos, normal, color, clip, stf, eye) = (11, 12, 13, 14, 15, 20);
    a.l(lq(XYZW, pos, 0, IN));
    a.l(lq(XYZW, normal, 1, IN));
    a.l(lq(XYZW, color, 2, IN));
    a.u(mulabc(XYZW, MVP, pos, BX));
    a.u(maddabc(XYZW, MVP + 1, pos, BY));
    a.u(maddabc(XYZW, MVP + 2, pos, BZ));
    a.u(maddbc(XYZW, clip, MVP + 3, 0, BW));
    // s y t viajan en el cuarto componente de la posición y de la normal.
    a.u(addbc(X, stf, ZERO, pos, BW));
    a.u(addbc(Y, stf, ZERO, normal, BW));
    a.u(addbc(Z, stf, ZERO, MISC, BW));
    if key.needs_eye() {
        for i in 0..4 {
            a.l(lq(XYZW, 16 + i, (C_MODELVIEW + i as usize) as i32, 0));
        }
        a.u(mulabc(XYZW, 16, pos, BX));
        a.u(maddabc(XYZW, 17, pos, BY));
        a.u(maddabc(XYZW, 18, pos, BZ));
        a.u(maddbc(XYZW, eye, 19, 0, BW));
    }
    if key.fog {
        // niebla = (fin - |z de ojo|) / (fin - inicio), entre 0 y 1.
        a.l(lq(XYZW, 21, C_FOG as i32, 0));
        a.u(abs(Z, 22, eye));
        a.u(subbc(X, 22, 21, 22, BZ));
        a.u(mulbc(X, 22, 22, 21, BY));
        a.u(maxbc(X, 22, 22, ZERO, BX));
        a.u(minibc(X, 22, 22, 0, BW));
        a.u(mulbc(X, 22, 22, MISC, BW));
        a.u(addbc(Z, stf, ZERO, 22, BX));
    }
    if key.lighting {
        let (n, acc, t) = (23, 24, 28);
        for i in 0..3 {
            a.l(lq(XYZW, 16 + i, (C_NORMAL + i as usize) as i32, 0));
        }
        a.u(mulabc(XYZ, 16, normal, BX));
        a.u(maddabc(XYZ, 17, normal, BY));
        a.u(maddbc(XYZ, n, 18, normal, BZ));
        if key.normalize {
            dot3(&mut a, t, n, n);
            a.l(rsqrt(0, BW, t, BX));
            a.l(WAITQ);
            a.u(mulq(XYZ, n, n));
        }
        a.l(lq(XYZW, acc, C_SCENE as i32, 0));
        for light in 0..key.light_count as usize {
            let base = (C_LIGHTS + light * 4) as i32;
            let (vector, ambient, diffuse) = (25, 26, 27);
            a.l(lq(XYZW, vector, base, 0));
            a.l(lq(XYZW, ambient, base + 1, 0));
            a.l(lq(XYZW, diffuse, base + 2, 0));
            if key.positional & (1 << light) == 0 {
                // Direccional: suma ambiente + max(n·l, 0) · difusa.
                dot3(&mut a, t, n, vector);
                a.u(maxbc(X, t, t, ZERO, BX));
                a.u(add(XYZ, acc, acc, ambient));
                a.u(mulabc(XYZ, diffuse, t, BX));
                a.u(maddbc(XYZ, acc, acc, 0, BW));
            } else {
                // Puntual: dirección y distancia a la luz, y atenuación.
                let (d, dist, k, den) = (29, 30, 21, 22);
                a.u(sub(XYZ, d, vector, eye));
                dot3(&mut a, t, d, d);
                a.l(rsqrt(0, BW, t, BX));
                a.l(WAITQ);
                a.u(mulq(XYZ, d, d));
                a.u(mulq(X, dist, t));
                a.l(lq(XYZW, k, base + 3, 0));
                // k0 + k1·distancia + k2·distancia²
                a.u(mulabc(X, k, 0, BW));
                a.u(maddabc(X, dist, k, BY));
                a.u(maddbc(X, den, t, k, BZ));
                a.l(div(0, BW, den, BX));
                dot3(&mut a, t, n, d);
                a.u(maxbc(X, t, t, ZERO, BX));
                a.l(WAITQ);
                a.u(mulabc(XYZ, ambient, 0, BW));
                a.u(maddbc(XYZ, diffuse, diffuse, t, BX));
                a.u(mulq(XYZ, diffuse, diffuse));
                a.u(add(XYZ, acc, acc, diffuse));
            }
        }
        // color = base + color del vértice · acumulado; el alfa no se toca.
        a.l(lq(XYZW, 25, C_BASE as i32, 0));
        a.u(mulabc(XYZ, 25, 0, BW));
        a.u(madd(XYZ, color, color, acc));
    }
    a.u(maxbc(XYZW, color, color, ZERO, BX));
    a.u(minibc(XYZW, color, color, 0, BW));
    a.l(sq(XYZW, clip, 0, WORK_PTR));
    a.l(sq(XYZW, color, 1, WORK_PTR));
    a.l(sq(XYZW, stf, 2, WORK_PTR));
    // Prueba de recorte, con x e y a escala de la banda de guarda.
    a.u(mul(XYZW, 16, clip, GUARD));
    a.u(clipw(16, 16));
    a.l(iaddiu(IN, IN, 3));
    a.l(iaddiu(WORK_PTR, WORK_PTR, 3));
    a.ret(AUX);

    // ------------------------------------------------------------ emitir un triángulo
    // Vértices en PA, PB y PC (recorte, color, (s, t, niebla)); salida en OUT.
    a.place(emit);
    let lod_offset = key.lod as i32;
    for (k, pointer) in [PA, PB, PC].into_iter().enumerate() {
        let (clip, color, stf, ndc, win, fixed, stq, rgba) = (11, 12, 22 + k as u32, 14, 19 + k as u32, 16, 17, 18);
        let at = lod_offset + 3 * k as i32;
        a.l(lq(XYZW, clip, 0, pointer));
        a.l(lq(XYZW, color, 1, pointer));
        a.l(lq(XYZW, stf, 2, pointer));
        a.l(div(0, BW, clip, BW));
        a.l(WAITQ);
        a.u(mulq(XYZ, ndc, clip));
        // ventana = desplazamiento + ndc · escala, con la profundidad acotada.
        a.u(mulabc(XYZW, OFFSET, 0, BW));
        a.u(madd(XYZW, win, ndc, SCALE));
        a.u(maxbc(Z, win, win, ZERO, BX));
        a.u(minibc(Z, win, win, MISC, BZ));
        a.u(addbc(W, win, ZERO, stf, BZ));
        // x, y, profundidad y niebla van en coma fija con 4 bits de fracción.
        a.u(ftoi4(XYZW, fixed, win));
        a.l(sq(XYZW, fixed, at + 2, OUT));
        // (s/w, t/w, 1/w) para la corrección de perspectiva.
        a.u(mulq(XY, stq, stf));
        a.u(addq(Z, stq, ZERO));
        a.l(sq(XYZW, stq, at, OUT));
        a.u(addabc(XYZW, ZERO, MISC, BX));
        a.u(madd(XYZW, rgba, color, COLOR_SCALE));
        a.u(ftoi0(XYZW, rgba, rgba));
        a.l(sq(XYZW, rgba, at + 1, OUT));
    }
    if key.lod {
        // Nivel de detalle: la mitad del log2 de (texels / píxeles) que
        // cubre el triángulo, en coma fija para el campo K de TEX1.
        let (ratio, consts_a, consts_b, level, tmp) = (25, 26, 27, 29, 30);
        a.u(sub(XYZW, 25, 20, 19));
        a.u(sub(XYZW, 26, 21, 19));
        a.u(opmula(25, 26));
        a.u(opmsub(27, 26, 25));
        a.u(sub(XYZW, 28, 23, 22));
        a.u(sub(XYZW, 29, 24, 22));
        a.u(opmula(28, 29));
        a.u(opmsub(30, 29, 28));
        a.u(abs(Z, 27, 27));
        a.u(abs(Z, 30, 30));
        a.l(lq(XYZW, 28, C_CULL as i32, 0));
        a.u(mulbc(Z, 30, 30, 28, BY));
        a.l(div(30, BZ, 27, BZ));
        a.l(WAITQ);
        a.u(addq(X, ratio, ZERO));
        a.l(lq(XYZW, consts_a, C_LOD as i32, 0));
        a.l(lq(XYZW, consts_b, C_LOD as i32 + 1, 0));
        // ¿Se reduce la textura (más de un texel por píxel)?
        a.u(subbc(X, tmp, ratio, 0, BW));
        positive(&mut a, tmp, AUX);
        a.u(sub(X, level, level, level));
        // `T.x = 1` si `ratio >= umbral`, si no 0.
        let step = |a: &mut Asm, threshold: u32, factor: u32, gain: Option<u32>| {
            a.u(subbc(X, T, ratio, consts_a, threshold));
            a.u(mulbc(X, T, T, MISC, BY));
            a.u(maxbc(X, T, T, ZERO, BX));
            a.u(minibc(X, T, T, 0, BW));
            match gain {
                None => a.u(add(X, level, level, T)),
                Some(g) => {
                    a.u(mulbc(X, tmp, T, consts_a, g));
                    a.u(add(X, level, level, tmp));
                }
            }
            // ratio -= ratio · factor · T
            a.u(mulbc(X, tmp, T, consts_a, factor));
            a.u(mul(X, tmp, tmp, ratio));
            a.u(sub(X, ratio, ratio, tmp));
        };
        // Cada división por 4 es un nivel; una por 2, medio.
        for _ in 0..7 {
            step(&mut a, BX, BY, None);
        }
        step(&mut a, BZ, BW, Some(BW));
        // Entre 1 y 2, log2(x) ≈ (x - 1) · (1,3465 - 0,3465 · (x - 1)).
        a.u(subbc(X, ratio, ratio, 0, BW));
        a.u(mulbc(X, tmp, ratio, consts_b, BY));
        a.u(sub(X, tmp, consts_b, tmp));
        a.u(mul(X, tmp, tmp, ratio));
        a.u(mulbc(X, tmp, tmp, consts_a, BW));
        a.u(add(X, level, level, tmp));
        a.u(maxbc(X, level, level, consts_b, BZ));
        a.u(minibc(X, level, level, consts_b, BW));
        a.u(ftoi4(X, level, level));
        a.l(mtir(FLAG, level, BX));
        let minified = a.label();
        a.branch(Branch::NotEqual(AUX, 0), minified, false);
        // Ampliada: K negativo, que elige el filtro de ampliación.
        a.l(iaddiu(FLAG, 0, 0xFFF));
        a.place(minified);
        a.l(lq(XYZW, 11, C_TEX1 as i32, 0));
        a.l(sq(XYZW, 11, 0, OUT));
        a.l(isw(Y, FLAG, 0, OUT));
    }
    a.l(iaddiu(OUT, OUT, record));
    a.l(iaddiu(OUT_COUNT, OUT_COUNT, 1));
    a.ret(RET_EMIT);

    // ------------------------------------------------------------ recortar contra un plano
    // Polígono de PC vértices en PA; el resultado va a PB y su número de
    // vértices queda en PC. El plano está en el registro 20.
    a.place(clip_plane);
    let plane = 20;
    // Una copia del primer vértice al final cierra el polígono.
    a.l(iadd(FLAG, PC, PC));
    a.l(iadd(FLAG, FLAG, PC));
    a.l(iadd(FLAG, FLAG, PA));
    copy(&mut a, 3, 0, PA, 0, FLAG);
    a.l(iaddiu(AUX, 0, 0));
    let (edge, current_out, same_side) = (a.label(), a.label(), a.label());
    a.place(edge);
    for i in 0..6 {
        a.l(lq(XYZW, 11 + i, i as i32, PA));
    }
    // Distancia de cada extremo al plano (negativa = fuera).
    let (dc, dn) = (17, 18);
    for (d, vertex) in [(dc, 11), (dn, 14)] {
        a.u(mul(XYZW, d, vertex, plane));
        a.u(addbc(X, d, d, d, BY));
        a.u(addbc(X, d, d, d, BZ));
        a.u(addbc(X, d, d, d, BW));
    }
    a.u(subbc(X, 19, ZERO, dc, BX));
    positive(&mut a, 19, FLAG);
    a.u(subbc(X, 19, ZERO, dn, BX));
    positive(&mut a, 19, RET_EMIT);
    a.branch(Branch::NotEqual(FLAG, 0), current_out, true);
    // El extremo actual está dentro: pasa tal cual.
    for i in 0..3 {
        a.l(sq(XYZW, 11 + i, i as i32, PB));
    }
    a.l(iaddiu(PB, PB, 3));
    a.l(iaddiu(AUX, AUX, 1));
    a.place(current_out);
    a.branch(Branch::Equal(FLAG, RET_EMIT), same_side, false);
    // La arista cruza el plano: vértice nuevo en el corte.
    a.u(subbc(X, 19, dc, dn, BX));
    a.l(div(dc, BX, 19, BX));
    a.l(WAITQ);
    for i in 0..3 {
        a.u(sub(XYZW, 21, 14 + i, 11 + i));
        a.u(mulabc(XYZW, 11 + i, 0, BW));
        a.u(maddq(XYZW, 21, 21));
        a.l(sq(XYZW, 21, i as i32, PB));
    }
    a.l(iaddiu(PB, PB, 3));
    a.l(iaddiu(AUX, AUX, 1));
    a.place(same_side);
    a.l(iaddiu(PA, PA, 3));
    a.l(isubiu(PC, PC, 1));
    a.branch(Branch::NotEqual(PC, 0), edge, true);
    a.l(iadd(PC, AUX, 0));
    a.ret(RET);

    a.finish()
}

fn floats(v: [f32; 4]) -> [u32; 4] {
    v.map(f32::to_bits)
}

/// Las constantes de una llamada de dibujo, listas para la memoria de la VU1.
pub fn constants(setup: &FastSetup, target: &Target) -> [[u32; 4]; CONSTANTS] {
    let mut c = [[0u32; 4]; CONSTANTS];
    for i in 0..4 {
        c[C_MVP + i] = floats([setup.mvp[i * 4], setup.mvp[i * 4 + 1], setup.mvp[i * 4 + 2], setup.mvp[i * 4 + 3]]);
        c[C_MODELVIEW + i] = floats([setup.modelview[i * 4], setup.modelview[i * 4 + 1], setup.modelview[i * 4 + 2], setup.modelview[i * 4 + 3]]);
    }
    for i in 0..3 {
        c[C_NORMAL + i] = floats([setup.normal[i][0], setup.normal[i][1], setup.normal[i][2], 0.0]);
    }
    let [vx, vy, vw, vh] = setup.viewport.map(|v| v as f32);
    let [near, far] = setup.depth_range;
    // Centro del viewport en coordenadas del GS.
    let (cx, cy) = (vx + vw * 0.5 + target.origin, target.height - (vy + vh * 0.5) + target.origin);
    let (depth_scale, depth_offset) = if target.inverted {
        (-(far - near) * 0.5 * target.depth_max, (1.0 - (near + far) * 0.5) * target.depth_max)
    } else {
        ((far - near) * 0.5 * target.depth_max, (near + far) * 0.5 * target.depth_max)
    };
    c[C_SCALE] = floats([vw * 0.5, -vh * 0.5, depth_scale, 0.0]);
    // Los centros de píxel del GS están en los enteros (-0,5) y la
    // conversión a coma fija trunca (+1/32 para redondear; +0,5 en z).
    c[C_OFFSET] = floats([cx - 0.5 + 1.0 / 32.0, cy - 0.5 + 1.0 / 32.0, depth_offset + 0.5, 0.0]);
    // Banda de guarda: hasta dónde puede salirse un vértice del viewport sin
    // desbordar las coordenadas del GS (de 0 a 4095).
    let guard = |center: f32, half: f32| {
        let margin = center.min(4095.0 - center) - 1.0;
        if margin > half && half > 0.0 {
            half / margin
        } else {
            1.0
        }
    };
    let (gx, gy) = (guard(cx, vw * 0.5), guard(cy, vh * 0.5));
    c[C_GUARD] = floats([gx, gy, 1.0, 1.0]);
    c[C_COLOR_SCALE] = floats([target.color_scale, target.color_scale, target.color_scale, 128.0]);
    c[C_MISC] = floats([0.5, 1.0e15, target.depth_max, 255.0]);
    c[C_BASE] = floats([setup.base[0], setup.base[1], setup.base[2], 0.0]);
    c[C_SCENE] = floats([setup.scene[0], setup.scene[1], setup.scene[2], 0.0]);
    let (fog_end, fog_scale) = setup.fog.unwrap_or((0.0, 0.0));
    c[C_FOG] = floats([fog_end, fog_scale, 0.0, 0.0]);
    let (tex1, texels) = target.lod.unwrap_or((0, 0.0));
    c[C_CULL] = floats([setup.cull, texels, 0.0, 0.0]);
    // Etiqueta GIF: un "bucle" por triángulo, con sus tres vértices como
    // (ST, RGBAQ, XYZF2) y, delante, un registro suelto (TEX1) si hay nivel
    // de detalle. El número de triángulos lo pone el microprograma.
    let (registers, list): (u32, u64) = if target.lod.is_some() { (10, 0x41_2412_412E) } else { (9, 0x4_1241_2412) };
    c[C_TAG] = [0, 1 << 14 | target.prim << 15 | registers << 28, list as u32, (list >> 32) as u32];
    c[C_TEX1] = [tex1, 0, 0x14, 0];
    // Planos de recorte: dentro si plano · vértice >= 0.
    c[C_PLANES] = floats([0.0, 0.0, 1.0, 1.0]);
    c[C_PLANES + 1] = floats([0.0, 0.0, -1.0, 1.0]);
    c[C_PLANES + 2] = floats([gx, 0.0, 0.0, 1.0]);
    c[C_PLANES + 3] = floats([-gx, 0.0, 0.0, 1.0]);
    c[C_PLANES + 4] = floats([0.0, gy, 0.0, 1.0]);
    c[C_PLANES + 5] = floats([0.0, -gy, 0.0, 1.0]);
    for (i, light) in setup.lights[..setup.light_count].iter().enumerate() {
        let at = C_LIGHTS + i * 4;
        c[at] = floats([light.vector[0], light.vector[1], light.vector[2], 0.0]);
        c[at + 1] = floats([light.ambient[0], light.ambient[1], light.ambient[2], 0.0]);
        c[at + 2] = floats([light.diffuse[0], light.diffuse[1], light.diffuse[2], 0.0]);
        c[at + 3] = floats([light.attenuation[0], light.attenuation[1], light.attenuation[2], 0.0]);
    }
    c[C_LOD] = floats([3.999, 0.75, 1.999, 0.5]);
    c[C_LOD + 1] = floats([1.3465, 0.3465, 1.0 / 16.0, 7.0]);
    c
}

/// Los enteros de una tanda de `triangles` triángulos.
pub fn batch_header(triangles: usize) -> [u32; 4] {
    [triangles as u32, 0x8000, 0, 0]
}

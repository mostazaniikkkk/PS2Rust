//! Matemáticas `f32` sobre la FPU del Emotion Engine.
//!
//! La FPU del R5900 es de precisión simple y no sigue IEEE 754 al pie de la
//! letra: no tiene NaN, infinitos ni denormales, y redondea hacia cero.

pub const PI: f32 = 3.141_592_7;
pub const TAU: f32 = 6.283_185_5;

/// Raíz cuadrada (instrucción `sqrt.s` de la FPU).
#[inline]
pub fn sqrt(x: f32) -> f32 {
    core::intrinsics::sqrtf32(x)
}

/// Mayor entero menor o igual que `x`.
#[inline]
pub fn floor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

/// Seno de `x` (radianes). Error absoluto por debajo de 1e-5.
pub fn sin(x: f32) -> f32 {
    // Reduce a [-π, π] y luego a [-π/2, π/2] por simetría.
    let mut r = x - TAU * floor(x / TAU + 0.5);
    if r > PI / 2.0 {
        r = PI - r;
    } else if r < -PI / 2.0 {
        r = -PI - r;
    }
    let r2 = r * r;
    // Serie de Taylor hasta x^11, en forma de Horner.
    r * (1.0
        + r2 * (-1.0 / 6.0
            + r2 * (1.0 / 120.0 + r2 * (-1.0 / 5040.0 + r2 * (1.0 / 362_880.0 + r2 * (-1.0 / 39_916_800.0))))))
}

/// Coseno de `x` (radianes).
#[inline]
pub fn cos(x: f32) -> f32 {
    sin(x + PI / 2.0)
}

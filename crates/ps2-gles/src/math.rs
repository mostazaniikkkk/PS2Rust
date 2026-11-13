//! Matrices 4×4 y las funciones matemáticas que usa el núcleo de GL. Las
//! exponenciales y logaritmos son propios (no hay `libm` en la PS2) y se usan
//! igual en el PC, para que las dos plataformas calculen lo mismo.

/// Matriz 4×4 por columnas, como en OpenGL: `m[columna * 4 + fila]`.
pub type Mat4 = [f32; 16];

pub const IDENTITY: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Producto `a · b`.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

/// Producto `m · v`.
#[inline]
pub fn transform(m: &Mat4, v: [f32; 4]) -> [f32; 4] {
    [
        m[0] * v[0] + m[4] * v[1] + m[8] * v[2] + m[12] * v[3],
        m[1] * v[0] + m[5] * v[1] + m[9] * v[2] + m[13] * v[3],
        m[2] * v[0] + m[6] * v[1] + m[10] * v[2] + m[14] * v[3],
        m[3] * v[0] + m[7] * v[1] + m[11] * v[2] + m[15] * v[3],
    ]
}

/// Inversa general (cofactores). `None` si la matriz es singular.
pub fn invert(m: &Mat4) -> Option<Mat4> {
    let mut inv = [0.0f32; 16];
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
    let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if det == 0.0 || det != det {
        return None;
    }
    let k = 1.0 / det;
    for x in &mut inv {
        *x *= k;
    }
    Some(inv)
}

#[cfg(target_arch = "mips")]
pub use ps2::math::{cos, sin, sqrt};

#[cfg(not(target_arch = "mips"))]
pub fn sqrt(x: f32) -> f32 {
    x.sqrt()
}

#[cfg(not(target_arch = "mips"))]
pub fn sin(x: f32) -> f32 {
    x.sin()
}

#[cfg(not(target_arch = "mips"))]
pub fn cos(x: f32) -> f32 {
    x.cos()
}

const LN2: f32 = 0.693_147_2;

/// e^x.
pub fn exp(x: f32) -> f32 {
    if x < -87.0 {
        return 0.0;
    }
    let x = if x > 88.0 { 88.0 } else { x };
    // x = k·ln2 + r, con |r| <= ln2/2.
    let k = (x / LN2 + if x < 0.0 { -0.5 } else { 0.5 }) as i32;
    let r = x - k as f32 * LN2;
    let p = 1.0 + r * (1.0 + r * (0.5 + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r * (1.0 / 120.0 + r * (1.0 / 720.0))))));
    p * f32::from_bits(((k + 127) as u32) << 23)
}

/// Logaritmo natural (para x > 0).
pub fn ln(x: f32) -> f32 {
    if x <= 0.0 {
        return -87.0;
    }
    let bits = x.to_bits();
    let mut e = ((bits >> 23) & 0xff) as i32 - 127;
    let mut m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    if m > 1.414_213_5 {
        m *= 0.5;
        e += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    e as f32 * LN2 + 2.0 * s * (1.0 + s2 * (1.0 / 3.0 + s2 * (1.0 / 5.0 + s2 * (1.0 / 7.0 + s2 * (1.0 / 9.0)))))
}

pub fn log2(x: f32) -> f32 {
    ln(x) / LN2
}

/// x^y para x >= 0.
pub fn pow(x: f32, y: f32) -> f32 {
    if y == 0.0 {
        1.0
    } else if x <= 0.0 {
        0.0
    } else {
        exp(y * ln(x))
    }
}

#[inline]
pub fn clamp01(x: f32) -> f32 {
    if x < 0.0 {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_ln_pow_are_accurate() {
        for &x in &[-20.0f32, -3.5, -1.0, -0.1, 0.0, 0.3, 1.0, 2.5, 10.0, 40.0] {
            let (mine, real) = (exp(x), x.exp());
            assert!((mine - real).abs() <= real * 2e-6, "exp({x}) = {mine}, real {real}");
        }
        for &x in &[1e-6f32, 0.01, 0.5, 1.0, 1.5, 2.0, 10.0, 12345.0] {
            assert!((ln(x) - x.ln()).abs() < 2e-5, "ln({x}) = {}", ln(x));
        }
        assert!((pow(0.9, 64.0) - 0.9f32.powf(64.0)).abs() < 1e-6);
        assert_eq!((pow(0.0, 5.0), pow(3.0, 0.0)), (0.0, 1.0));
    }

    #[test]
    fn inverse_times_matrix_is_identity() {
        let m: Mat4 = [2.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, -1.0, 3.0, 0.0, 4.0, 5.0, 6.0, 1.0];
        let p = mul(&m, &invert(&m).unwrap());
        for i in 0..16 {
            assert!((p[i] - IDENTITY[i]).abs() < 1e-5);
        }
        assert!(invert(&[0.0; 16]).is_none());
    }
}

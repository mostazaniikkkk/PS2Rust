//! Matemáticas sin FPU. El IOP hace toda la coma flotante por software (las
//! operaciones básicas las pone `compiler-builtins`); aquí está lo que falta.

/// Raíz cuadrada `f32`, redondeada correctamente (IEEE 754, al más cercano).
pub fn sqrt(x: f32) -> f32 {
    let bits = x.to_bits();
    let magnitude = bits & 0x7fff_ffff;
    // ±0, +infinito y NaN se devuelven tal cual.
    if magnitude == 0 || bits == 0x7f80_0000 || magnitude > 0x7f80_0000 {
        return x;
    }
    if bits >> 31 != 0 {
        return f32::NAN;
    }
    let mut exp = (bits >> 23) as i32;
    let mut man = bits & 0x007f_ffff;
    if exp == 0 {
        // Subnormal: se normaliza a mano.
        let shift = man.leading_zeros() as i32 - 8;
        man <<= shift;
        exp = 1 - shift;
    }
    man = (man & 0x007f_ffff) | 0x0080_0000;
    exp -= 127;
    if exp & 1 != 0 {
        man <<= 1;
    }
    exp >>= 1;
    // man está en [2^23, 2^25): la raíz entera de man·2^25 da 24 bits de
    // resultado y uno más para redondear.
    let n = (man as u64) << 25;
    let r = n.isqrt();
    let exact = r * r == n;
    let mut q = (r >> 1) as u32;
    if r & 1 == 1 && (!exact || q & 1 == 1) {
        q += 1;
    }
    if q == 1 << 24 {
        q >>= 1;
        exp += 1;
    }
    f32::from_bits((((exp + 127) as u32) << 23) | (q & 0x007f_ffff))
}

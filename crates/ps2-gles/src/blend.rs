//! La mezcla del Graphics Synthesizer frente a `glBlendFunc`.

use crate::raster::Factor;

/// Resuelve una pareja de factores de GL en el registro ALPHA del GS, o
/// `None` si el GS no puede hacer esa mezcla.
///
/// La fórmula del GS es `(A - B)·C + D`, donde A, B y D pueden ser el color
/// de origen (0), el de destino (1) o cero (2), y C el alfa de origen (0), el
/// de destino (1) o una constante (2), que aquí vale siempre 1.
pub fn solve_blend(src: Factor, dst: Factor) -> Option<u64> {
    // Un factor como combinación (constante, alfa de origen, alfa de destino).
    let parts = |f: Factor| -> Option<[i8; 3]> {
        Some(match f {
            Factor::Zero => [0, 0, 0],
            Factor::One => [1, 0, 0],
            Factor::SrcAlpha => [0, 1, 0],
            Factor::OneMinusSrcAlpha => [1, -1, 0],
            Factor::DstAlpha => [0, 0, 1],
            Factor::OneMinusDstAlpha => [1, 0, -1],
            _ => return None,
        })
    };
    let (want_src, want_dst) = (parts(src)?, parts(dst)?);
    for a in 0..3u64 {
        for b in 0..3u64 {
            for c in 0..3u64 {
                for d in 0..3u64 {
                    let weight: [i8; 3] = [[0, 1, 0], [0, 0, 1], [1, 0, 0]][c as usize];
                    // Coeficiente con el que entra el color `which` (0 = origen, 1 = destino).
                    let coefficient = |which: u64| {
                        let mut k = [0i8; 3];
                        for i in 0..3 {
                            k[i] = (a == which) as i8 * weight[i] - (b == which) as i8 * weight[i];
                        }
                        k[0] += (d == which) as i8;
                        k
                    };
                    if coefficient(0) == want_src && coefficient(1) == want_dst {
                        return Some(a | b << 2 | c << 4 | d << 6 | 0x80 << 32);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use Factor::*;

    #[test]
    fn standard_blends_are_solved() {
        // (Cs - Cd)·As + Cd
        assert_eq!(solve_blend(SrcAlpha, OneMinusSrcAlpha), Some(0 | 1 << 2 | 0 << 4 | 1 << 6 | 0x80 << 32));
        // (Cs - 0)·1 + Cd
        assert_eq!(solve_blend(One, One), Some(0 | 2 << 2 | 2 << 4 | 1 << 6 | 0x80 << 32));
        assert!(solve_blend(SrcAlpha, One).is_some());
        assert!(solve_blend(OneMinusDstAlpha, DstAlpha).is_some());
    }

    #[test]
    fn supported_pairs_are_exactly_these() {
        let all = [Zero, One, SrcAlpha, OneMinusSrcAlpha, DstAlpha, OneMinusDstAlpha];
        let mut supported = Vec::new();
        for s in all {
            for d in all {
                if solve_blend(s, d).is_some() {
                    supported.push((s, d));
                }
            }
        }
        assert_eq!(
            supported,
            [
                (Zero, Zero),
                (Zero, One),
                (Zero, SrcAlpha),
                (Zero, OneMinusSrcAlpha),
                (Zero, DstAlpha),
                (Zero, OneMinusDstAlpha),
                (One, Zero),
                (One, One),
                (One, SrcAlpha),
                (One, DstAlpha),
                (SrcAlpha, Zero),
                (SrcAlpha, One),
                (SrcAlpha, OneMinusSrcAlpha),
                (OneMinusSrcAlpha, Zero),
                (OneMinusSrcAlpha, SrcAlpha),
                (DstAlpha, Zero),
                (DstAlpha, One),
                (DstAlpha, OneMinusDstAlpha),
                (OneMinusDstAlpha, Zero),
                (OneMinusDstAlpha, DstAlpha),
            ]
        );
        // Con factores de color no hay nada que hacer.
        assert!(solve_blend(DstColor, Zero).is_none());
        assert!(solve_blend(SrcAlphaSaturate, One).is_none());
    }
}

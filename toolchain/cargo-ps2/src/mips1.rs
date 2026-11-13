//! Verificador de código máquina para el IOP (R3000A, MIPS I sin FPU).
//!
//! Comprueba el `.text` de un módulo IRX:
//!
//! * Sólo instrucciones de MIPS I. El R3000A no tiene las de MIPS II (saltos
//!   "likely", trampas, `ll`/`sc`, `sync`), ni FPU, ni nada de 64 bits.
//! * **Retardo de carga**: en MIPS I el valor de una carga (`lw`, `lb`…) no
//!   está disponible para la instrucción siguiente. Se sigue también el salto
//!   cuando la carga está en su hueco.
//! * **HI/LO**: tras `mfhi`/`mflo`, las dos instrucciones siguientes no pueden
//!   escribir HI/LO (`mult`, `div`, `mthi`, `mtlo`).
//!
//! LLVM respeta el retardo de carga para `mips1`, pero no la regla de HI/LO
//! (el ensamblador de GNU sí mete los `nop`): [`fix_hilo`] la corrige sobre
//! el binario enlazado y [`check`] comprueba el resultado final. Las tablas de
//! importación (que viven en `.text` pero son datos) se saltan.

use crate::irx::import_tables;
use std::collections::HashSet;

pub struct Report {
    pub scanned: usize,
    pub errors: Vec<(u32, u32, String)>,
}

fn is_load(w: u32) -> bool {
    matches!(w >> 26, 0x20..=0x26)
}

/// Máscara de registros enteros que lee la instrucción.
fn reads(w: u32) -> u32 {
    let op = w >> 26;
    let rs = (w >> 21) & 31;
    let rt = (w >> 16) & 31;
    let bit = |r: u32| if r == 0 { 0 } else { 1u32 << r };
    match op {
        0x00 => match w & 63 {
            0x00 | 0x02 | 0x03 => bit(rt),
            0x08 | 0x09 | 0x11 | 0x13 => bit(rs),
            0x0c | 0x0d | 0x10 | 0x12 => 0,
            _ => bit(rs) | bit(rt),
        },
        0x01 => bit(rs),
        0x02 | 0x03 | 0x0f => 0,
        0x04 | 0x05 => bit(rs) | bit(rt),
        0x06 | 0x07 => bit(rs),
        0x08..=0x0e => bit(rs),
        0x10 => {
            if rs == 0x04 {
                bit(rt)
            } else {
                0
            }
        }
        0x20 | 0x21 | 0x23 | 0x24 | 0x25 => bit(rs),
        // lwl/lwr completan el registro destino: lo leen.
        0x22 | 0x26 => bit(rs) | bit(rt),
        0x28..=0x2b | 0x2e => bit(rs) | bit(rt),
        _ => bit(rs) | bit(rt),
    }
}

/// Destino de un salto relativo o absoluto, como índice de palabra.
fn branch_target(w: u32, i: usize) -> Option<Option<usize>> {
    let op = w >> 26;
    match op {
        0x02 | 0x03 => Some(Some(((w & 0x03ff_ffff) as usize) & 0x00ff_ffff)),
        0x01 | 0x04..=0x07 => {
            let off = (w & 0xffff) as u16 as i16 as i64;
            let t = i as i64 + 1 + off;
            Some(if t >= 0 { Some(t as usize) } else { None })
        }
        0x00 if matches!(w & 63, 0x08 | 0x09) => Some(None),
        _ => None,
    }
}

fn is_mfhilo(w: u32) -> bool {
    w >> 26 == 0 && matches!(w & 63, 0x10 | 0x12)
}

fn writes_hilo(w: u32) -> bool {
    w >> 26 == 0 && matches!(w & 63, 0x11 | 0x13 | 0x18..=0x1b)
}

/// Resultado de [`fix_hilo`].
pub struct HiLoFix {
    /// Secuencias corregidas.
    pub sites: usize,
    /// Código de los trampolines, para colocar en `tramp_offset`.
    pub code: Vec<u8>,
    /// Instrucciones que cambiaron de sitio: (desplazamiento antiguo, nuevo).
    /// Sus relocaciones deben seguirlas.
    pub moved: Vec<(u32, u32)>,
    /// Desplazamientos de los `j` añadidos: necesitan una relocación `R_MIPS_26`.
    pub jumps: Vec<u32>,
}

/// Aleja cada `mult`/`div` de los `mfhi`/`mflo` que tiene justo delante.
///
/// En el R3000A, si una interrupción llega cuando un `mult`/`div` ya ha
/// empezado y el `mfhi`/`mflo` anterior aún no ha terminado, éste se repite
/// con HI/LO ya pisados. Por eso MIPS I pide dos instrucciones de margen. La
/// secuencia `mflo; [mfhi;] [K;] mult` se sustituye por un salto a un
/// trampolín que la ejecuta con el margen (el propio salto de vuelta cuenta):
///
/// ```text
///     j    trampolin            trampolin:  mflo ...
///     nop                                   nop          (o K)
///                                           j    vuelta
///     vuelta: ...                           mult ...     (en el hueco)
/// ```
///
/// `text` es el código enlazado en `base`; `tramp_offset`, el desplazamiento
/// (desde el inicio del texto) donde quedarán los trampolines; `targets`, los
/// índices de palabra a los que se puede saltar desde fuera del propio código
/// (símbolos, tablas de saltos).
pub fn fix_hilo(text: &mut [u8], base: u32, tramp_offset: u32, targets: &HashSet<usize>) -> Result<HiLoFix, String> {
    let mut words: Vec<u32> = text.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let n = words.len();
    let mut is_data = vec![false; n];
    for (_, _, range) in import_tables(text) {
        for i in range {
            is_data[i] = true;
        }
    }
    // Destinos de los saltos del propio código.
    let mut targets = targets.clone();
    for (i, &w) in words.iter().enumerate() {
        if is_data[i] {
            continue;
        }
        match w >> 26 {
            0x02 | 0x03 => {
                targets.insert((((w & 0x03ff_ffff) << 2).wrapping_sub(base) / 4) as usize);
            }
            0x01 | 0x04..=0x07 => {
                targets.insert((i as i64 + 1 + (w & 0xffff) as u16 as i16 as i64) as usize);
            }
            _ => {}
        }
    }
    let jump = |offset: u32| 0x0800_0000 | ((base.wrapping_add(offset) >> 2) & 0x03ff_ffff);
    let mut fix = HiLoFix { sites: 0, code: Vec::new(), moved: Vec::new(), jumps: Vec::new() };
    let mut code: Vec<u32> = Vec::new();
    let mut i = 0;
    while i < n {
        if is_data[i] || !is_mfhilo(words[i]) {
            i += 1;
            continue;
        }
        // Tramo de mfhi/mflo seguidos.
        let mut e = i + 1;
        while e < n && !is_data[e] && is_mfhilo(words[e]) {
            e += 1;
        }
        let at = |k: usize| if k < n && !is_data[k] { words[k] } else { 0 };
        // ¿Hay un mult/div justo detrás, o con una instrucción K por medio?
        let (last, moves_k) = if writes_hilo(at(e)) {
            (e, false)
        } else if writes_hilo(at(e + 1)) {
            (e + 1, true)
        } else {
            i = e;
            continue;
        };
        let addr = base + 4 * i as u32;
        let cannot = |why: &str| Err(format!("{addr:#010x}: mult/div demasiado cerca de mfhi/mflo y no se puede corregir: {why}"));
        if i > 0 && !is_data[i - 1] && branch_target(words[i - 1], i - 1).is_some() {
            return cannot("el mfhi/mflo está en el hueco de un salto");
        }
        if moves_k && branch_target(words[e], e).is_some() {
            return cannot("hay un salto entre los dos");
        }
        if (i + 1..=last).any(|k| targets.contains(&k)) {
            return cannot("hay un destino de salto en medio de la secuencia");
        }
        if last + 1 >= n {
            return cannot("la secuencia está al final del código");
        }
        let trampoline = tramp_offset + 4 * code.len() as u32;
        code.extend_from_slice(&words[i..e]);
        if moves_k {
            fix.moved.push((4 * e as u32, tramp_offset + 4 * code.len() as u32));
            code.push(words[e]);
        } else {
            code.push(0);
        }
        fix.jumps.push(tramp_offset + 4 * code.len() as u32);
        code.push(jump(4 * (last as u32 + 1)));
        code.push(words[last]);
        fix.jumps.push(4 * i as u32);
        words[i] = jump(trampoline);
        for w in &mut words[i + 1..=last] {
            *w = 0;
        }
        fix.sites += 1;
        i = last + 1;
    }
    for (chunk, w) in text.chunks_exact_mut(4).zip(&words) {
        chunk.copy_from_slice(&w.to_le_bytes());
    }
    fix.code = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    Ok(fix)
}

pub fn check(text: &[u8], base: u32) -> Report {
    let words: Vec<u32> = text.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let mut is_data = vec![false; words.len()];
    for (_, _, range) in import_tables(text) {
        for i in range {
            is_data[i] = true;
        }
    }
    let mut rep = Report { scanned: 0, errors: Vec::new() };
    for (i, &w) in words.iter().enumerate() {
        if is_data[i] {
            continue;
        }
        rep.scanned += 1;
        let addr = base + 4 * i as u32;
        let op = w >> 26;
        let rs = (w >> 21) & 31;
        let rt = (w >> 16) & 31;
        let func = w & 63;
        let mut err = |msg: &str| rep.errors.push((addr, w, msg.to_string()));

        match op {
            0x00 => match func {
                0x00 | 0x02 | 0x03 | 0x04 | 0x06 | 0x07 | 0x08 | 0x09 | 0x0c | 0x0d | 0x10..=0x13 | 0x18..=0x1b | 0x20..=0x27 | 0x2a | 0x2b => {}
                0x0a | 0x0b => err("movz/movn no existen en MIPS I"),
                0x0f => err("sync no existe en MIPS I"),
                0x30..=0x36 => err("las trampas (teq, tge…) son de MIPS II"),
                _ => err("instrucción SPECIAL que no es de MIPS I"),
            },
            0x01 => {
                if !matches!(rt, 0x00 | 0x01 | 0x10 | 0x11) {
                    err("salto REGIMM que no es de MIPS I (likely o trampa)");
                }
            }
            0x02..=0x0f => {}
            0x10 => {}
            0x11 => err("el IOP no tiene FPU"),
            0x12 | 0x13 => err("el IOP no tiene coprocesador 2/3"),
            0x14..=0x17 => err("los saltos 'likely' son de MIPS II"),
            0x20..=0x26 | 0x28..=0x2b | 0x2e => {}
            0x30 | 0x38 => err("ll/sc son de MIPS II"),
            0x31 | 0x35 | 0x39 | 0x3d => err("el IOP no tiene FPU"),
            _ => err("opcode que no es de MIPS I"),
        }
        let _ = rs;

        // Retardo de carga.
        if is_load(w) && rt != 0 {
            let bit = 1u32 << rt;
            let uses = |j: usize| -> bool {
                let Some(&n) = words.get(j) else { return false };
                if is_data[j] {
                    return false;
                }
                // lwl + lwr sobre el mismo registro es la pareja habitual.
                let pair = matches!((op, n >> 26), (0x22, 0x26) | (0x26, 0x22)) && (n >> 16) & 31 == rt;
                !pair && reads(n) & bit != 0
            };
            let in_delay_slot = i > 0 && !is_data[i - 1] && branch_target(words[i - 1], i - 1).is_some();
            let mut bad = false;
            if in_delay_slot {
                let prev = words[i - 1];
                if let Some(Some(t)) = branch_target(prev, i - 1) {
                    bad |= uses(t);
                }
                // Si el salto es condicional, también se puede seguir recto.
                if matches!(prev >> 26, 0x01 | 0x04..=0x07) {
                    bad |= uses(i + 1);
                }
            } else {
                bad = uses(i + 1);
            }
            if bad {
                rep.errors.push((addr, w, "el valor cargado se usa en la instrucción siguiente (retardo de carga de MIPS I)".into()));
            }
        }

        // HI/LO: dos instrucciones de margen tras mfhi/mflo.
        if op == 0 && matches!(func, 0x10 | 0x12) {
            for j in i + 1..=i + 2 {
                if let Some(&n) = words.get(j) {
                    if !is_data[j] && n >> 26 == 0 && matches!(n & 63, 0x11 | 0x13 | 0x18..=0x1b) {
                        rep.errors.push((addr, w, "mult/div a menos de dos instrucciones de mfhi/mflo (riesgo HI/LO de MIPS I)".into()));
                    }
                }
            }
        }
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULT: u32 = 0x0085_0018; // mult a0, a1
    const MFLO: u32 = 0x0000_1012; // mflo v0
    const MFHI: u32 = 0x0000_1810; // mfhi v1
    const ADDU: u32 = 0x0043_2021; // addu a0, v0, v1
    const JR_RA: u32 = 0x03e0_0008;

    fn fixed(words: &[u32], base: u32) -> (Vec<u32>, HiLoFix) {
        let mut text: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let fix = fix_hilo(&mut text, base, 0x100, &HashSet::new()).unwrap();
        let mut all = text.clone();
        all.extend_from_slice(&fix.code);
        assert!(check(&all, 0).errors.is_empty(), "el resultado debe pasar el verificador");
        (text.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(), fix)
    }

    fn code(fix: &HiLoFix) -> Vec<u32> {
        fix.code.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }

    #[test]
    fn hilo_mult_right_after_mflo_mfhi_goes_through_a_trampoline() {
        // mult; mflo; mfhi; mult; mflo; jr ra; nop
        let (text, fix) = fixed(&[MULT, MFLO, MFHI, MULT, MFLO, JR_RA, 0], 0x2000);
        assert_eq!(fix.sites, 1);
        // j 0x2100; nop; nop; después sigue igual.
        assert_eq!(text, [MULT, 0x0800_0840, 0, 0, MFLO, JR_RA, 0]);
        // mflo; mfhi; nop; j 0x2010; mult
        assert_eq!(code(&fix), [MFLO, MFHI, 0, 0x0800_0804, MULT]);
        assert_eq!(fix.jumps, [0x10c, 0x4]);
        assert!(fix.moved.is_empty());
    }

    #[test]
    fn hilo_instruction_between_moves_and_its_relocation_follows() {
        // mflo; addu; mult; jr ra; nop
        let (text, fix) = fixed(&[MFLO, ADDU, MULT, JR_RA, 0], 0);
        assert_eq!(text, [0x0800_0040, 0, 0, JR_RA, 0]);
        // mflo; addu; j 0xc; mult
        assert_eq!(code(&fix), [MFLO, ADDU, 0x0800_0003, MULT]);
        assert_eq!(fix.moved, [(4, 0x104)]);
    }

    #[test]
    fn hilo_distant_mult_is_left_alone_and_bad_cases_are_errors() {
        let (text, fix) = fixed(&[MFLO, ADDU, ADDU, MULT, JR_RA, 0], 0);
        assert_eq!((fix.sites, text[0]), (0, MFLO));
        // mflo en el hueco de un salto.
        let mut text: Vec<u8> = [JR_RA, MFLO, MULT, 0].iter().flat_map(|w: &u32| w.to_le_bytes()).collect();
        assert!(fix_hilo(&mut text, 0, 0x100, &HashSet::new()).is_err());
        // Un salto cae sobre el mult: beq zero, zero, +1 ; nop ; mflo ; mult ; nop
        let mut text: Vec<u8> = [0x1000_0002, 0, MFLO, MULT, 0].iter().flat_map(|w: &u32| w.to_le_bytes()).collect();
        assert!(fix_hilo(&mut text, 0, 0x100, &HashSet::new()).is_err());
    }

    fn run(words: &[u32]) -> Vec<String> {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        check(&bytes, 0).errors.into_iter().map(|e| e.2).collect()
    }

    #[test]
    fn accepts_mips1_and_skips_import_tables() {
        // addiu, lw + nop + uso, jr ra, nop, y una tabla de importación.
        let table = [0x41e0_0000, 0, 0x0000_0101, 0x6964_7473, 0x0000_006f, 0x03e0_0008, 0x2400_0004, 0, 0];
        let mut code = vec![0x27bd_ffe8, 0x8fa2_0010, 0, 0x0040_1821, 0x03e0_0008, 0];
        code.extend_from_slice(&table);
        assert!(run(&code).is_empty(), "{:?}", run(&code));
    }

    #[test]
    fn rejects_mips2_and_fpu() {
        // beql, teq, sync, ll, lwc1, add.s
        let errs = run(&[0x5000_0001, 0, 0x0000_0034, 0x0000_000f, 0xc082_0000, 0xc480_0000, 0x4600_0000]);
        assert_eq!(errs.len(), 6);
    }

    #[test]
    fn load_delay_is_checked() {
        // lw $2,0($4); addu $3,$2,$2  -> error
        assert_eq!(run(&[0x8c82_0000, 0x0042_1821]).len(), 1);
        // lw $2,0($4); nop; addu -> bien
        assert!(run(&[0x8c82_0000, 0, 0x0042_1821]).is_empty());
        // lwl $2,3($4); lwr $2,0($4) -> bien (pareja)
        assert!(run(&[0x8882_0003, 0x9882_0000, 0]).is_empty());
        // beq $0,$0,+1 con lw $2 en el hueco; el destino usa $2 -> error
        assert_eq!(run(&[0x1000_0002, 0x8c82_0000, 0, 0x0042_1821]).len(), 1);
    }

    #[test]
    fn hilo_hazard_is_checked() {
        // mflo $2; mult $2,$3 -> error
        assert_eq!(run(&[0x0000_1012, 0x0043_0018]).len(), 1);
        // mflo $2; nop; nop; mult -> bien
        assert!(run(&[0x0000_1012, 0, 0, 0x0043_0018]).is_empty());
    }
}

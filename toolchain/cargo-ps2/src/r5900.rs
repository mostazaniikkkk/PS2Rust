//! Legalizador y verificador de código máquina para el R5900 (Emotion Engine).
//!
//! LLVM no conoce el R5900. Generamos MIPS II + FPU de precisión simple, que es
//! casi un subconjunto del R5900, y aquí cerramos la brecha sobre el ELF ya
//! enlazado:
//!
//! * **Legalización** (parches in situ, misma longitud):
//!   - `ll`/`sc` no existen en el R5900. La secuencia que emite LLVM
//!     (`ll … sc rt; beqz rt, retry`) se reescribe como `lw … sw rt; li rt, 1`.
//!   - Comparaciones FPU "unordered"/"signaling" (`c.un.s`, `c.ult.s`,
//!     `c.ule.s`, …) no existen: el R5900 sólo tiene C.F, C.EQ, C.LT y C.LE.
//!     Como su FPU no tiene NaN, la variante ordenada es exactamente equivalente.
//!   - `trunc.w.s` no existe, pero `cvt.w.s` del R5900 siempre trunca.
//!   - `sqrt.s` usa el campo `ft` como operando en vez de `fs`.
//! * **Verificación**: cualquier instrucción que el R5900 no implemente
//!   (`dmult`, `ddiv`, `movf`, dobles en FPU, `ldc1`, …) es un error, igual que
//!   los bucles cortos con delay slot ocupado (errata de hardware del R5900).

use std::collections::BTreeMap;

pub struct Issue {
    pub addr: u32,
    pub word: u32,
    pub msg: String,
}

#[derive(Default)]
pub struct Report {
    pub scanned: usize,
    pub patched: BTreeMap<&'static str, usize>,
    pub errors: Vec<Issue>,
}

impl Report {
    fn patch(&mut self, what: &'static str) {
        *self.patched.entry(what).or_insert(0) += 1;
    }
    fn error(&mut self, addr: u32, word: u32, msg: impl Into<String>) {
        self.errors.push(Issue { addr, word, msg: msg.into() });
    }
    pub fn total_patches(&self) -> usize {
        self.patched.values().sum()
    }
}

fn rd32(code: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([code[i * 4], code[i * 4 + 1], code[i * 4 + 2], code[i * 4 + 3]])
}

fn wr32(code: &mut [u8], i: usize, w: u32) {
    code[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
}

/// Si `w` es un salto condicional relativo, devuelve su desplazamiento en palabras.
fn branch_offset(w: u32) -> Option<i32> {
    let op = w >> 26;
    let rs = (w >> 21) & 31;
    let rt = (w >> 16) & 31;
    let is_branch = match op {
        0x04..=0x07 | 0x14..=0x17 => true,
        0x01 => matches!(rt, 0x00..=0x03 | 0x10..=0x13),
        0x10 | 0x11 | 0x12 => rs == 0x08,
        _ => false,
    };
    if is_branch {
        Some((w & 0xffff) as u16 as i16 as i32)
    } else {
        None
    }
}

/// Legaliza y verifica una sección ejecutable. `base` es su dirección virtual.
/// Con `apply == false` sólo verifica: lo que necesitaría parche es un error.
pub fn process(code: &mut [u8], base: u32, apply: bool, rep: &mut Report) {
    let n = code.len() / 4;
    if code.len() % 4 != 0 {
        rep.error(base, 0, "la sección de código no es múltiplo de 4 bytes");
    }
    let mut i = 0;
    while i < n {
        let w = rd32(code, i);
        let addr = base.wrapping_add((i * 4) as u32);
        rep.scanned += 1;
        let op = w >> 26;
        let rt = (w >> 16) & 31;
        let func = w & 63;

        // Errata del R5900: un bucle de pocas instrucciones cuyo salto hacia
        // atrás lleva algo distinto de NOP en el delay slot puede ejecutarse
        // sólo una o dos veces en hardware real.
        if let Some(off) = branch_offset(w) {
            if off < 0 && off >= -7 && i + 1 < n && rd32(code, i + 1) != 0 {
                rep.error(addr, w, "bucle corto con delay slot ocupado (errata 'short loop' del R5900)");
            }
        }

        match op {
            0x00 => match func {
                0x01 => rep.error(addr, w, "movf/movt no existen en el R5900"),
                0x1c..=0x1f => rep.error(addr, w, "dmult/dmultu/ddiv/ddivu no existen en el R5900"),
                0x05 | 0x0e | 0x15 | 0x35 | 0x37 | 0x39 | 0x3d => {
                    rep.error(addr, w, format!("SPECIAL funct {func:#04x} reservada en el R5900"))
                }
                _ => {}
            },
            0x01 => match rt {
                0x00..=0x03 | 0x08..=0x0c | 0x0e | 0x10..=0x13 | 0x18 | 0x19 => {}
                _ => rep.error(addr, w, format!("REGIMM rt {rt:#04x} reservada en el R5900")),
            },
            0x11 => cop1(code, i, addr, w, apply, rep),
            0x1c => match func {
                0x00 | 0x01 | 0x04 | 0x08 | 0x09 | 0x10..=0x13 | 0x18..=0x1b | 0x20 | 0x21
                | 0x28 | 0x29 | 0x30 | 0x31 | 0x34 | 0x36 | 0x37 | 0x3c | 0x3e | 0x3f => {}
                _ => rep.error(addr, w, format!("MMI funct {func:#04x} reservada (¿instrucción MIPS32 SPECIAL2?)")),
            },
            0x30 => {
                // ll rt, off(base) -> lw rt, off(base)
                if apply {
                    wr32(code, i, (w & 0x03ff_ffff) | (0x23 << 26));
                    rep.patch("ll -> lw");
                } else {
                    rep.error(addr, w, "ll no existe en el R5900");
                }
            }
            0x38 => {
                // sc rt, off(base); beqz rt, retry  ->  sw rt, off(base); addiu rt, $zero, 1
                let next = if i + 1 < n { rd32(code, i + 1) } else { 0 };
                let nop = next >> 26;
                let nrs = (next >> 21) & 31;
                let nrt = (next >> 16) & 31;
                let is_retry = nop == 0x04 && rt != 0 && ((nrs == rt && nrt == 0) || (nrs == 0 && nrt == rt));
                if !apply {
                    rep.error(addr, w, "sc no existe en el R5900");
                } else if is_retry {
                    wr32(code, i, (w & 0x03ff_ffff) | (0x2b << 26));
                    wr32(code, i + 1, (0x09 << 26) | (rt << 16) | 1);
                    rep.patch("sc+beqz -> sw+li");
                    rep.scanned += 1;
                    i += 1;
                } else {
                    rep.error(addr, w, "sc sin el 'beqz' de reintento esperado: no se puede legalizar");
                }
            }
            0x13 | 0x1d | 0x32 | 0x3a | 0x3b => {
                rep.error(addr, w, format!("opcode {op:#04x} reservado en el R5900"))
            }
            0x34 | 0x3c => rep.error(addr, w, "lld/scd no existen en el R5900"),
            0x35 | 0x3d => rep.error(addr, w, "ldc1/sdc1: la FPU del R5900 es sólo de precisión simple"),
            _ => {}
        }
        i += 1;
    }
}

fn cop1(code: &mut [u8], i: usize, addr: u32, w: u32, apply: bool, rep: &mut Report) {
    let rs = (w >> 21) & 31;
    let ft = (w >> 16) & 31;
    let fs = (w >> 11) & 31;
    let func = w & 63;
    match rs {
        0x00 | 0x02 | 0x04 | 0x06 => {}
        0x08 => {
            if ft > 3 {
                rep.error(addr, w, "BC1 con código de condición distinto de 0");
            }
        }
        0x10 => match func {
            0x00..=0x03 | 0x05..=0x07 => {}
            0x04 => {
                // MIPS: sqrt.s fd, fs.   R5900: sqrt.s fd, ft (fs = 0).
                if fs != 0 && ft == 0 {
                    if apply {
                        wr32(code, i, (w & !(0x3ff << 11)) | (fs << 16));
                        rep.patch("sqrt.s fs -> ft");
                    } else {
                        rep.error(addr, w, "sqrt.s con codificación MIPS estándar (el R5900 usa ft)");
                    }
                } else if fs != 0 {
                    rep.error(addr, w, "sqrt.s con fs y ft a la vez");
                }
            }
            0x0d => {
                if apply {
                    wr32(code, i, (w & !63) | 0x24);
                    rep.patch("trunc.w.s -> cvt.w.s");
                } else {
                    rep.error(addr, w, "trunc.w.s no existe en el R5900");
                }
            }
            0x0c | 0x0e | 0x0f => rep.error(addr, w, "round/ceil/floor.w.s no existen en el R5900"),
            0x16 | 0x18..=0x1a | 0x1c..=0x1f | 0x24 | 0x28 | 0x29 => {}
            0x30..=0x3f => {
                if (w >> 8) & 7 != 0 {
                    rep.error(addr, w, "comparación FPU con código de condición distinto de 0");
                }
                let legal = 0x30 | (func & 0x06);
                if legal != func {
                    if apply {
                        wr32(code, i, (w & !63) | legal);
                        rep.patch("c.<unordered>.s -> c.<ordered>.s");
                    } else {
                        rep.error(addr, w, "comparación FPU unordered/signaling: no existe en el R5900");
                    }
                }
            }
            0x21 => rep.error(addr, w, "cvt.d.s: la FPU del R5900 no tiene dobles"),
            _ => rep.error(addr, w, format!("COP1.S funct {func:#04x} no existe en el R5900")),
        },
        0x14 => {
            if func != 0x20 {
                rep.error(addr, w, format!("COP1.W funct {func:#04x} no existe en el R5900"));
            }
        }
        0x11 | 0x15 => rep.error(addr, w, "FPU de doble precisión / formato L: no existe en el R5900"),
        0x01 | 0x05 => rep.error(addr, w, "dmfc1/dmtc1 no existen en el R5900"),
        _ => rep.error(addr, w, format!("COP1 rs {rs:#04x} reservado en el R5900")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(words: &[u32], apply: bool) -> (Vec<u32>, Report) {
        let mut bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut rep = Report::default();
        process(&mut bytes, 0x0010_0000, apply, &mut rep);
        let out = bytes.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        (out, rep)
    }

    #[test]
    fn llsc_becomes_lw_sw_li() {
        // ll $7,0($3); addu $8,$7,$1; sc $8,0($3); beqz $8,-4; nop
        let (out, rep) = run(&[0xc067_0000, 0x00e1_4021, 0xe068_0000, 0x1100_fffc, 0], true);
        assert!(rep.errors.is_empty());
        assert_eq!(out[0], 0x8c67_0000); // lw $7,0($3)
        assert_eq!(out[2], 0xac68_0000); // sw $8,0($3)
        assert_eq!(out[3], 0x2408_0001); // addiu $8,$zero,1
    }

    #[test]
    fn fpu_compares_are_ordered() {
        // c.ult.s $f12,$f14 ; c.ule.s ; c.un.s ; c.ueq.s
        let (out, rep) = run(&[0x460e_6035, 0x460e_6037, 0x460e_6031, 0x460e_6033], true);
        assert!(rep.errors.is_empty());
        assert_eq!(out, vec![0x460e_6034, 0x460e_6036, 0x460e_6030, 0x460e_6032]);
    }

    #[test]
    fn trunc_and_sqrt() {
        // trunc.w.s $f0,$f12 ; sqrt.s $f0,$f12
        let (out, rep) = run(&[0x4600_600d, 0x4600_6004], true);
        assert!(rep.errors.is_empty());
        assert_eq!(out[0], 0x4600_6024); // cvt.w.s $f0,$f12
        assert_eq!(out[1], 0x460c_0004); // sqrt.s $f0 <- ft=$f12
    }

    #[test]
    fn rejects_what_cannot_be_fixed() {
        // dmult ; ldc1 ; add.d ; bne hacia atrás con delay slot ocupado
        let (_, rep) = run(&[0x0085_001c, 0xd480_0000, 0x4620_0000, 0x0000_0000, 0x1420_fffe, 0x2442_0001], true);
        assert_eq!(rep.errors.len(), 4);
    }

    #[test]
    fn check_only_mode_reports_instead_of_patching() {
        let (out, rep) = run(&[0xc067_0000], false);
        assert_eq!(out[0], 0xc067_0000);
        assert_eq!(rep.errors.len(), 1);
    }
}

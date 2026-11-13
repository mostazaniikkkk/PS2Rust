//! Cargas no alineadas (`lwl`/`lwr`) y los 32 bits altos de los registros.
//!
//! El R5900 tiene registros de 64 bits (128 en realidad). El código MIPS de
//! 32 bits funciona porque cada instrucción de 32 bits deja el resultado
//! extendido en signo ("canónico"), y las instrucciones que miran los 64 bits
//! (`and`, `or`, `xor`, `nor`, `slt`, `sltu`, saltos condicionales) dan el
//! mismo resultado con valores canónicos.
//!
//! El recompilador del EE de PCSX2 no extiende el signo en las cargas no
//! alineadas `lwl` + `lwr` (deja ceros arriba). Con ello, por ejemplo, el
//! bucle de búsqueda de `HashMap` no termina nunca. El intérprete de PCSX2
//! (y el hardware) sí lo hacen.
//!
//! Corrección, sobre el ELF ya enlazado:
//!
//! * Si el valor cargado sólo se escribe a memoria (`sw`/`swl`/`swr`/`sh`/`sb`,
//!   que usan los 32 bits bajos) antes de redefinirse, por todos los caminos,
//!   no hace falta nada. Es el caso de las copias de memoria.
//! * Si no, la pareja se desvía a un trampolín de la sección `.ps2tramp`
//!   (8 palabras por trampolín):
//!
//!   ```text
//!   original:  lwl rt, o+3(b)        parcheado:  j   T
//!              lwr rt, o(b)                      nop            (hueco del salto)
//!   T:         lwl rt, o+3(b)
//!              lwr rt, o(b)
//!              sll rt, rt, 0         <- vuelve a extender el signo
//!              j   (siguiente)
//!              nop
//!   ```
//!
//!   Si la segunda mitad es destino de algún salto no se puede vaciar; se usa
//!   entonces la variante que deja la segunda mitad en el hueco del salto (se
//!   ejecuta antes que la primera), sólo si la primera no usa `rt` como base.
//! * Si entre las dos mitades hay otras instrucciones que no tocan `rt` ni la
//!   base, la primera mitad se acerca a la segunda antes de desviarla.

use std::collections::{BTreeSet, HashSet};

const OP_LWL: u32 = 0x22;
const OP_LWR: u32 = 0x26;
const OP_J: u32 = 0x02;
const SLOT: usize = 8;

pub struct Fix {
    pub trampolines: usize,
    pub moved: usize,
    pub store_only: usize,
    pub errors: Vec<(u32, String)>,
}

/// Lo que una instrucción hace con los registros enteros.
struct Regs {
    reads: u32,
    write: Option<u32>,
    store_data: Option<u32>,
    control: bool,
}

fn decode(w: u32) -> Regs {
    let op = w >> 26;
    let rs = (w >> 21) & 31;
    let rt = (w >> 16) & 31;
    let rd = (w >> 11) & 31;
    let func = w & 63;
    let bit = |r: u32| if r == 0 { 0 } else { 1u32 << r };
    let mut r = Regs { reads: 0, write: None, store_data: None, control: false };
    match op {
        0x00 => match func {
            0x00 | 0x02 | 0x03 => {
                r.reads = bit(rt);
                r.write = Some(rd);
            }
            0x08 => {
                r.reads = bit(rs);
                r.control = true;
            }
            0x09 => {
                r.reads = bit(rs);
                r.write = Some(rd);
                r.control = true;
            }
            0x0c | 0x0d => r.control = true,
            0x0f => {}
            0x10 | 0x12 => r.write = Some(rd),
            0x11 | 0x13 => r.reads = bit(rs),
            0x18..=0x1b => {
                r.reads = bit(rs) | bit(rt);
                if rd != 0 {
                    r.write = Some(rd);
                }
            }
            0x04 | 0x06 | 0x07 | 0x20..=0x27 | 0x2a | 0x2b => {
                r.reads = bit(rs) | bit(rt);
                r.write = Some(rd);
            }
            _ => {
                r.reads = bit(rs) | bit(rt) | bit(rd);
                r.control = true;
            }
        },
        0x01 | 0x04..=0x07 | 0x14..=0x17 => {
            r.reads = bit(rs) | bit(rt);
            r.control = true;
        }
        0x02 | 0x03 => r.control = true,
        0x08..=0x0e => {
            r.reads = bit(rs);
            r.write = Some(rt);
        }
        0x0f => r.write = Some(rt),
        0x10..=0x12 => match rs {
            0x00 | 0x02 => r.write = Some(rt),
            0x04 | 0x06 => r.reads = bit(rt),
            0x08 => r.control = true,
            _ => {}
        },
        0x20 | 0x21 | 0x23..=0x25 | 0x27 => {
            r.reads = bit(rs);
            r.write = Some(rt);
        }
        OP_LWL | OP_LWR => {
            // Escritura parcial: lee también el registro destino.
            r.reads = bit(rs) | bit(rt);
        }
        0x28 | 0x29 | 0x2a | 0x2b | 0x2e => {
            r.reads = bit(rs);
            r.store_data = Some(rt);
        }
        0x31 | 0x35 | 0x39 | 0x3d => r.reads = bit(rs),
        _ => {
            r.reads = bit(rs) | bit(rt);
            r.control = true;
        }
    }
    if r.write == Some(0) {
        r.write = None;
    }
    r
}

fn is_unaligned_load(w: u32) -> bool {
    matches!(w >> 26, OP_LWL | OP_LWR)
}

fn rt_of(w: u32) -> u32 {
    (w >> 16) & 31
}

fn base_of(w: u32) -> u32 {
    (w >> 21) & 31
}

/// ¿Las dos instrucciones forman una carga no alineada de la misma palabra?
fn partners(a: u32, b: u32) -> bool {
    let (op_a, op_b) = (a >> 26, b >> 26);
    if !(op_a == OP_LWL && op_b == OP_LWR || op_a == OP_LWR && op_b == OP_LWL) {
        return false;
    }
    if rt_of(a) != rt_of(b) {
        return false;
    }
    if base_of(a) == base_of(b) {
        let (lwl, lwr) = if op_a == OP_LWL { (a, b) } else { (b, a) };
        return (lwl & 0xffff).wrapping_sub(lwr & 0xffff) & 0xffff == 3;
    }
    // Copias que recalculan la base en otro registro: basta con el destino.
    true
}

fn j_to(target: u32) -> u32 {
    (OP_J << 26) | ((target >> 2) & 0x03ff_ffff)
}

fn sll0(reg: u32) -> u32 {
    (reg << 16) | (reg << 11)
}

/// La sección de trampolines.
struct Tramp {
    addr: u32,
    words: Vec<u32>,
}

/// Lo que hace un trampolín: registro que define y registros que lee.
struct TrampUse {
    rt: u32,
    reads: u32,
}

impl Tramp {
    fn contains(&self, a: u32) -> bool {
        a >= self.addr && a < self.addr + 4 * self.words.len() as u32
    }

    /// Si `w` (con `delay` en su hueco) es el salto a un trampolín, qué hace.
    fn usage(&self, w: u32, delay: u32) -> Option<TrampUse> {
        if w >> 26 != OP_J {
            return None;
        }
        let t = (w & 0x03ff_ffff) << 2;
        if !self.contains(t) {
            return None;
        }
        let i = ((t - self.addr) / 4) as usize;
        let first = *self.words.get(i)?;
        let second = *self.words.get(i + 1)?;
        if !is_unaligned_load(first) {
            return None;
        }
        let other = if is_unaligned_load(second) { second } else { delay };
        let bit = |r: u32| if r == 0 { 0 } else { 1u32 << r };
        Some(TrampUse { rt: rt_of(first), reads: bit(base_of(first)) | bit(base_of(other)) })
    }

    /// Primer hueco libre (en palabras).
    fn used(&self) -> usize {
        self.words.chunks(SLOT).take_while(|c| c.iter().any(|&w| w != 0)).count() * SLOT
    }
}

/// Desde la instrucción `from` (excluida), ¿el registro `reg` sólo se usa
/// como dato de escrituras a memoria antes de redefinirse, por todos los
/// caminos? Sigue saltos y las dos ramas de los condicionales dentro de la
/// sección, con un presupuesto acotado; ante la duda, responde que no.
fn store_only(words: &[u32], base: u32, from: usize, reg: u32, tramp: &Tramp) -> bool {
    // Registros que una llamada destruye (y que no son argumentos), y los que
    // quedan muertos al volver de la función (ABI o32).
    const CLOBBERED_BY_CALL: u32 = 1 << 1 | 1 << 2 | 1 << 3 | 0xff << 8 | 1 << 24 | 1 << 25;
    const DEAD_AT_RETURN: u32 = 1 << 1 | 0xf << 4 | 0xff << 8 | 1 << 24 | 1 << 25;
    let bit = 1u32 << reg;
    let plain_ok = |w: u32| {
        let r = decode(w);
        !r.control && (r.reads & bit == 0 || (r.store_data == Some(reg) && base_of(w) != reg))
    };
    let mut stack = vec![from + 1];
    let mut visited = HashSet::new();
    let mut budget = 600usize;
    while let Some(mut i) = stack.pop() {
        loop {
            if i >= words.len() || budget == 0 {
                return false;
            }
            if !visited.insert(i) {
                break; // camino ya analizado (o bucle sin usos)
            }
            budget -= 1;
            let w = words[i];
            if let Some(u) = tramp.usage(w, words.get(i + 1).copied().unwrap_or(0)) {
                if u.reads & bit != 0 {
                    return false;
                }
                if u.rt == reg {
                    break;
                }
                i += 2;
                continue;
            }
            // Otra carga no alineada al mismo registro empieza un valor nuevo
            // (la pareja reescribe los cuatro bytes).
            if is_unaligned_load(w) && rt_of(w) == reg && base_of(w) != reg {
                break;
            }
            let r = decode(w);
            if !r.control {
                if r.store_data == Some(reg) && base_of(w) != reg {
                    i += 1;
                    continue;
                }
                if r.reads & bit != 0 {
                    return false;
                }
                if r.write == Some(reg) {
                    break;
                }
                i += 1;
                continue;
            }
            // Instrucción de control: su hueco también se ejecuta.
            if r.reads & bit != 0 || !words.get(i + 1).is_some_and(|&d| plain_ok(d)) {
                return false;
            }
            if words.get(i + 1).is_some_and(|&d| decode(d).write == Some(reg)) {
                break;
            }
            let op = w >> 26;
            let func = w & 63;
            let index_of = |addr: u32| -> Option<usize> {
                let off = addr.wrapping_sub(base);
                if off % 4 == 0 && (off / 4) < words.len() as u32 { Some((off / 4) as usize) } else { None }
            };
            match op {
                OP_J => {
                    let target = (base + 4 * i as u32 & 0xf000_0000) | (w & 0x03ff_ffff) << 2;
                    match index_of(target) {
                        Some(t) => i = t,
                        None => return false,
                    }
                }
                0x03 => {
                    if CLOBBERED_BY_CALL & bit == 0 {
                        return false;
                    }
                    break;
                }
                0x00 if func == 0x09 => {
                    if CLOBBERED_BY_CALL & bit == 0 {
                        return false;
                    }
                    break;
                }
                0x00 if func == 0x08 => {
                    // Sólo `jr $ra` (retorno); los saltos por tabla, no.
                    if base_of(w) != 31 || DEAD_AT_RETURN & bit == 0 {
                        return false;
                    }
                    break;
                }
                0x01 | 0x04..=0x07 | 0x14..=0x17 | 0x11 => {
                    let off = (w & 0xffff) as u16 as i16 as i64;
                    let target = i as i64 + 1 + off;
                    if target < 0 || target as usize >= words.len() {
                        return false;
                    }
                    stack.push(target as usize);
                    i += 2;
                }
                _ => return false,
            }
        }
    }
    true
}

/// Busca la pareja de la carga en `k`: devuelve el índice de la otra mitad.
/// Lo que hay en medio no puede tocar `rt`.
fn find_partner(words: &[u32], k: usize, tramp: &Tramp) -> Option<usize> {
    let w = words[k];
    let rt = rt_of(w);
    let mut j = k + 1;
    while j < words.len() && j <= k + 64 {
        let x = words[j];
        if partners(w, x) {
            return Some(j);
        }
        if let Some(u) = tramp.usage(x, words.get(j + 1).copied().unwrap_or(0)) {
            if u.rt == rt || u.reads & (1 << rt) != 0 {
                return None;
            }
            j += 2;
            continue;
        }
        let r = decode(x);
        if r.control || r.write == Some(rt) || r.reads & (1 << rt) != 0 {
            return None;
        }
        j += 1;
    }
    None
}

/// Direcciones de código que pueden ser destino de un salto: destinos de
/// saltos relativos y absolutos, retornos de llamadas, y cualquier palabra de
/// los datos que caiga dentro del código (tablas de saltos, punteros a
/// función).
fn jump_targets(code: &[(u32, Vec<u32>)], data: &[u32]) -> HashSet<u32> {
    let mut set = HashSet::new();
    let in_code = |a: u32| code.iter().any(|(b, w)| a >= *b && a < *b + 4 * w.len() as u32);
    for (base, words) in code {
        for (i, &w) in words.iter().enumerate() {
            let pc = base + 4 * i as u32;
            let op = w >> 26;
            if !decode(w).control {
                continue;
            }
            if op == OP_J || op == 0x03 {
                set.insert((pc & 0xf000_0000) | (w & 0x03ff_ffff) << 2);
            } else if op != 0 {
                let off = (w & 0xffff) as u16 as i16 as i32;
                set.insert(pc.wrapping_add(4).wrapping_add((off * 4) as u32));
            }
            set.insert(pc + 8); // vuelta de una llamada
        }
    }
    for &d in data {
        if d % 4 == 0 && in_code(d) {
            set.insert(d);
        }
    }
    set
}

/// Aplica la corrección (o, con `apply == false`, sólo verifica) sobre todas
/// las secciones de código. `data` son las palabras de las secciones de
/// datos; `tramp_in` es la sección de trampolines (dirección, palabras).
pub fn fix(code: &mut [(u32, Vec<u32>)], data: &[u32], tramp_in: Option<&mut (u32, Vec<u32>)>, apply: bool) -> Fix {
    let mut out = Fix { trampolines: 0, moved: 0, store_only: 0, errors: Vec::new() };
    let targets = jump_targets(code, data);
    let has_tramp = tramp_in.is_some();
    let mut tramp = match &tramp_in {
        Some(t) => Tramp { addr: t.0, words: t.1.clone() },
        None => Tramp { addr: 0, words: Vec::new() },
    };
    let mut used = tramp.used();

    if !apply {
        // Cada trampolín: las cargas de la pareja, sll rt,rt,0, j, nop.
        for (n, slot) in tramp.words.chunks(SLOT).enumerate().take(used / SLOT) {
            let addr = tramp.addr + (n * SLOT * 4) as u32;
            let rt = rt_of(slot[0]);
            let ok = if is_unaligned_load(slot[1]) {
                is_unaligned_load(slot[0]) && partners(slot[0], slot[1]) && slot[2] == sll0(rt) && slot[3] >> 26 == OP_J && slot[4] == 0
            } else {
                is_unaligned_load(slot[0]) && slot[1] == sll0(rt) && slot[2] >> 26 == OP_J && slot[3] == 0
            };
            if !ok {
                out.errors.push((addr, "trampolín mal formado".into()));
            }
        }
    }

    for (base, words) in code.iter_mut() {
        let mut done: BTreeSet<usize> = BTreeSet::new();
        let mut k = 0;
        while k < words.len() {
            let w = words[k];
            let addr = *base + 4 * k as u32;
            if tramp.usage(w, words.get(k + 1).copied().unwrap_or(0)).is_some() {
                k += 2;
                continue;
            }
            if !is_unaligned_load(w) || done.contains(&k) {
                k += 1;
                continue;
            }
            let Some(p) = find_partner(words, k, &tramp) else {
                out.errors.push((addr, "carga no alineada (lwl/lwr) sin su pareja".into()));
                k += 1;
                continue;
            };
            done.insert(p);
            let reg = rt_of(w);
            if store_only(words, *base, p, reg, &tramp) {
                out.store_only += 1;
                k += 1;
                continue;
            }
            if !apply {
                out.errors.push((addr, "carga no alineada usada en cálculos sin volver a extender el signo".into()));
                k += 1;
                continue;
            }
            if k > 0 && decode(words[k - 1]).control {
                out.errors.push((addr, "carga no alineada en el hueco de un salto".into()));
                k += 1;
                continue;
            }
            let adjacent = p == k + 1;
            let mut first = k;
            if !adjacent {
                // Se acerca la primera mitad a la segunda: nada en medio toca
                // el registro (lo garantiza find_partner) ni la base, y nada
                // de lo que se desplaza es destino de un salto.
                let b = base_of(w);
                let movable = (k + 1..p).all(|j| match tramp.usage(words[j], words.get(j + 1).copied().unwrap_or(0)) {
                    Some(u) => u.rt != b,
                    None => decode(words[j]).write != Some(b),
                }) && (k + 1..=p).all(|j| !targets.contains(&(*base + 4 * j as u32)));
                if !movable {
                    if std::env::var_os("PS2_DEBUG_UNALIGNED").is_some() {
                        for j in k + 1..=p {
                            let a = *base + 4 * j as u32;
                            eprintln!("  {a:#x} {:08x} escribe={:?} destino={} tramp={}", words[j], decode(words[j]).write, targets.contains(&a), tramp.usage(words[j], 0).is_some());
                        }
                    }
                    out.errors.push((addr, "carga no alineada que no se puede desviar a un trampolín".into()));
                    k += 1;
                    continue;
                }
                words[k..p].rotate_left(1);
                // Una pareja ya desviada que sube una posición: su trampolín
                // debe volver una instrucción antes.
                for j in k..p - 1 {
                    if tramp.usage(words[j], words[j + 1]).is_some() {
                        let t = ((words[j] & 0x03ff_ffff) << 2) - tramp.addr;
                        let slot = (t / 4) as usize;
                        let back = j_to(*base + 4 * j as u32 + 8);
                        if let Some(x) = tramp.words[slot..slot + SLOT].iter_mut().find(|x| **x >> 26 == OP_J) {
                            *x = back;
                        }
                    }
                }
                done = done.into_iter().map(|i| if i > k && i < p { i - 1 } else { i }).collect();
                first = p - 1;
                out.moved += 1;
            }
            let addr = *base + 4 * first as u32;
            if !has_tramp {
                out.errors.push((addr, "falta la sección .ps2tramp en el linker script".into()));
                k = p + 1;
                continue;
            }
            if used + SLOT > tramp.words.len() {
                out.errors.push((addr, "no caben más trampolines: agranda .ps2tramp".into()));
                k = p + 1;
                continue;
            }
            let (a, b) = (words[first], words[first + 1]);
            let target = tramp.addr + 4 * used as u32;
            let back = addr + 8;
            let slot = &mut tramp.words[used..used + SLOT];
            if !targets.contains(&(addr + 4)) {
                // Las dos mitades, en su orden, dentro del trampolín.
                slot[..5].copy_from_slice(&[a, b, sll0(reg), j_to(back), 0]);
                words[first] = j_to(target);
                words[first + 1] = 0;
            } else if base_of(a) != reg {
                // La segunda mitad se queda en el hueco del salto y se ejecuta
                // antes: válido porque la primera no usa rt como base.
                slot[..4].copy_from_slice(&[a, sll0(reg), j_to(back), 0]);
                words[first] = j_to(target);
            } else {
                out.errors.push((addr, "carga no alineada que no se puede desviar a un trampolín".into()));
                k = p + 1;
                continue;
            }
            used += SLOT;
            out.trampolines += 1;
            // Si se movió, lo que había en medio ahora empieza en `k`.
            if adjacent {
                k = p + 1;
            }
        }
    }
    if let Some(t) = tramp_in {
        t.1 = tramp.words;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lwl(rt: u32, base: u32, off: u32) -> u32 {
        (OP_LWL << 26) | (base << 21) | (rt << 16) | off
    }
    fn lwr(rt: u32, base: u32, off: u32) -> u32 {
        (OP_LWR << 26) | (base << 21) | (rt << 16) | off
    }
    fn xor(rd: u32, rs: u32, rt: u32) -> u32 {
        (rs << 21) | (rt << 16) | (rd << 11) | 0x26
    }
    fn sw(rt: u32, base: u32, off: u32) -> u32 {
        (0x2b << 26) | (base << 21) | (rt << 16) | off
    }
    fn addiu(rt: u32, rs: u32, imm: u32) -> u32 {
        (0x09 << 26) | (rs << 21) | (rt << 16) | imm
    }
    fn beq_back(n: i32) -> u32 {
        (0x04 << 26) | ((n as u32) & 0xffff)
    }

    #[test]
    fn computed_value_goes_through_trampoline_in_order() {
        let mut code = vec![(0x0010_0000, vec![lwl(17, 1, 3), lwr(17, 1, 0), xor(1, 17, 3), 0])];
        let mut tramp = (0x0020_0000, vec![0u32; 32]);
        let r = fix(&mut code, &[], Some(&mut tramp), true);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.trampolines, 1);
        assert_eq!(code[0].1[..2], [j_to(0x0020_0000), 0]);
        assert_eq!(tramp.1[..5], [lwl(17, 1, 3), lwr(17, 1, 0), sll0(17), j_to(0x0010_0008), 0]);
        let check = fix(&mut code, &[], Some(&mut tramp), false);
        assert!(check.errors.is_empty(), "{:?}", check.errors);
    }

    #[test]
    fn base_equal_to_destination_keeps_order() {
        // lwl $v1, 3($v1); lwr $v1, 0($v0): la primera usa rt como base.
        let mut code = vec![(0x0010_0000, vec![lwl(3, 3, 3), lwr(3, 2, 0), xor(1, 3, 4), 0])];
        let mut tramp = (0x0020_0000, vec![0u32; 32]);
        let r = fix(&mut code, &[], Some(&mut tramp), true);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(tramp.1[..2], [lwl(3, 3, 3), lwr(3, 2, 0)]);
    }

    #[test]
    fn second_half_as_jump_target_uses_swapped_form() {
        // Un salto apunta a la segunda mitad (índice 2): no se puede vaciar.
        let mut code = vec![(0x0010_0000, vec![0, lwl(17, 1, 3), lwr(17, 1, 0), xor(1, 17, 3), beq_back(-3), 0])];
        let mut tramp = (0x0020_0000, vec![0u32; 32]);
        let r = fix(&mut code, &[], Some(&mut tramp), true);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(code[0].1[1..3], [j_to(0x0020_0000), lwr(17, 1, 0)]);
        assert_eq!(tramp.1[..4], [lwl(17, 1, 3), sll0(17), j_to(0x0010_000c), 0]);
        // Con rt como base de la primera mitad, eso no sería válido: error.
        let mut code = vec![(0x0010_0000, vec![0, lwl(3, 3, 3), lwr(3, 2, 0), xor(1, 3, 4), beq_back(-3), 0])];
        let mut tramp = (0x0020_0000, vec![0u32; 32]);
        assert_eq!(fix(&mut code, &[], Some(&mut tramp), true).errors.len(), 1);
    }

    #[test]
    fn copies_are_left_alone() {
        let mut code = vec![(
            0x0010_0000,
            vec![lwl(3, 2, 7), lwl(6, 2, 11), lwr(6, 2, 8), sw(6, 5, 8), lwr(3, 2, 4), sw(3, 5, 4), addiu(3, 0, 1), addiu(6, 0, 2)],
        )];
        let r = fix(&mut code, &[], None, false);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.store_only, 2);
    }

    #[test]
    fn separated_pair_is_moved_together() {
        // lwl $a2; sw $a1 (no toca $a2 ni la base); lwr $a2; xor usa $a2.
        let mut code = vec![(0x0010_0000, vec![lwl(6, 1, 7), sw(5, 3, 0), lwr(6, 1, 4), xor(2, 6, 3), 0])];
        let mut tramp = (0x0020_0000, vec![0u32; 32]);
        let r = fix(&mut code, &[], Some(&mut tramp), true);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!((r.moved, r.trampolines), (1, 1));
        assert_eq!(code[0].1[..3], [sw(5, 3, 0), j_to(0x0020_0000), 0]);
        assert_eq!(tramp.1[..4], [lwl(6, 1, 7), lwr(6, 1, 4), sll0(6), j_to(0x0010_000c)]);
    }

    #[test]
    fn interleaved_pairs_both_go_through_trampolines() {
        // Como en SipHash: lwl t8; lwl t9; (otras); lwr t8; lwr t9; usos.
        let mut code = vec![(
            0x0010_0000,
            vec![lwl(24, 1, 7), lwl(25, 1, 3), addiu(13, 0, 5), lwr(24, 1, 4), lwr(25, 1, 0), xor(12, 24, 12), xor(2, 25, 15), 0],
        )];
        let mut tramp = (0x0020_0000, vec![0u32; 64]);
        let r = fix(&mut code, &[], Some(&mut tramp), true);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.trampolines, 2);
        // Cada salto al trampolín vuelve justo después de su hueco.
        for (i, &w) in code[0].1.iter().enumerate() {
            if w >> 26 == OP_J {
                let slot = (((w & 0x03ff_ffff) << 2) - 0x0020_0000) as usize / 4;
                assert_eq!(tramp.1[slot + 3], j_to(0x0010_0000 + 4 * i as u32 + 8));
            }
        }
        let check = fix(&mut code, &[], Some(&mut tramp), false);
        assert!(check.errors.is_empty(), "{:?}", check.errors);
    }

    #[test]
    fn unpatched_computation_is_an_error() {
        let mut code = vec![(0x0010_0000, vec![lwl(17, 1, 3), lwr(17, 1, 0), xor(1, 17, 3)])];
        let r = fix(&mut code, &[], None, false);
        assert_eq!(r.errors.len(), 1);
    }
}

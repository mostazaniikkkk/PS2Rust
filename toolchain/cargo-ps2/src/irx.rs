//! Conversión de un ELF enlazado en la dirección 0 (con `--emit-relocs`) a un
//! módulo IRX del IOP.
//!
//! Un IRX es un ELF reubicable de tipo 0xFF80 con dos cabeceras de programa
//! (la de `.iopmod`, con los datos del módulo, y el segmento a cargar) y
//! secciones `.rel.*` con relocaciones relativas a la base de carga. El
//! cargador de la BIOS (LOADCORE) sólo entiende cuatro tipos:
//!
//! * `R_MIPS_32`: suma la base a una palabra de datos.
//! * `R_MIPS_26`: suma la base al destino de un `j`/`jal`.
//! * `R_MIPS_HI16` + `R_MIPS_LO16`, **siempre en pareja**: recalcula los 16
//!   bits altos de una instrucción (`lui`) mirando los 16 bits bajos de la
//!   entrada siguiente de la tabla, y suma la base a esos 16 bits bajos.
//!
//! El LOADCORE de la BIOS ignora un `LO16` que no vaya justo detrás de un
//! `HI16` (medido en PCSX2 con la BIOS real; la reimplementación abierta de
//! PS2SDK sí lo aplicaría). Las herramientas de Sony y de PS2SDK generan
//! siempre un `lui` por acceso, pero LLVM reutiliza un `lui` para varios. El
//! conversor lo arregla con palabras auxiliares que añade al final del módulo:
//!
//! * A un `LO16` suelto se le antepone un `HI16` que apunta a una palabra
//!   auxiliar: el cargador "reubica" esa palabra (da igual) y luego el `LO16`.
//! * A un `HI16` cuyo `LO16` no es la entrada siguiente se le pone detrás un
//!   `LO16` sobre otra palabra auxiliar con los mismos 16 bits bajos, para que
//!   el cálculo del `lui` salga bien.
//!
//! Además, aquí se aplica la corrección de HI/LO de MIPS I (ver
//! `mips1::fix_hilo`). Sus trampolines y las palabras auxiliares se colocan
//! detrás de `.bss` (símbolo `__irxtramp_start` del linker script), en las
//! secciones `.tramp` y `.relfix` del IRX; `.bss` pasa entonces a estar en el
//! archivo, como ceros.

use crate::mips1;
use std::collections::{HashMap, HashSet};

const R_MIPS_NONE: u32 = 0;
const R_MIPS_32: u32 = 2;
const R_MIPS_26: u32 = 4;
const R_MIPS_HI16: u32 = 5;
const R_MIPS_LO16: u32 = 6;
const R_MIPS_PC16: u32 = 10;
const R_MIPS_JALR: u32 = 37;

const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_NOBITS: u32 = 8;
const SHT_REL: u32 = 9;
const SHT_IOPMOD: u32 = 0x7000_0080;
const SHF_ALLOC: u32 = 2;
const SHN_ABS: u16 = 0xfff1;
const ET_IOPRELEXEC: u16 = 0xff80;

pub struct Irx {
    pub bytes: Vec<u8>,
    pub name: String,
    pub version: (u8, u8),
    pub text_size: u32,
    pub data_size: u32,
    pub bss_size: u32,
    pub relocs: usize,
    pub helpers: usize,
    /// Secuencias mfhi/mflo + mult/div corregidas con trampolín.
    pub hilo_fixes: usize,
    pub imports: Vec<(String, usize)>,
}

struct Section {
    name: String,
    ty: u32,
    flags: u32,
    addr: u32,
    offset: usize,
    size: usize,
    link: u32,
    info: u32,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn cstr(b: &[u8], o: usize) -> String {
    let end = b[o..].iter().position(|&c| c == 0).map(|n| o + n).unwrap_or(b.len());
    String::from_utf8_lossy(&b[o..end]).into_owned()
}

fn sections(b: &[u8]) -> Result<Vec<Section>, String> {
    if b.len() < 52 || &b[0..4] != b"\x7fELF" || b[4] != 1 || b[5] != 1 || u16le(b, 18) != 8 {
        return Err("no es un ELF MIPS de 32 bits little-endian".into());
    }
    let shoff = u32le(b, 32) as usize;
    let shentsize = u16le(b, 46) as usize;
    let shnum = u16le(b, 48) as usize;
    let shstrndx = u16le(b, 50) as usize;
    if shoff == 0 || shoff + shnum * shentsize > b.len() {
        return Err("el ELF no tiene tabla de secciones".into());
    }
    let strtab = u32le(b, shoff + shstrndx * shentsize + 16) as usize;
    Ok((0..shnum)
        .map(|i| {
            let s = shoff + i * shentsize;
            Section {
                name: cstr(b, strtab + u32le(b, s) as usize),
                ty: u32le(b, s + 4),
                flags: u32le(b, s + 8),
                addr: u32le(b, s + 12),
                offset: u32le(b, s + 16) as usize,
                size: u32le(b, s + 20) as usize,
                link: u32le(b, s + 24),
                info: u32le(b, s + 28),
            }
        })
        .collect())
}

/// Tablas de importación dentro de `.text`: (nombre, nº de funciones, rango en palabras).
pub fn import_tables(text: &[u8]) -> Vec<(String, usize, std::ops::Range<usize>)> {
    let words: Vec<u32> = text.chunks_exact(4).map(|c| u32le(c, 0)).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 5 <= words.len() {
        if words[i] == 0x41e0_0000 && words[i + 1] == 0 {
            let name = cstr(&text[i * 4 + 12..i * 4 + 20], 0);
            let mut j = i + 5;
            let mut stubs = 0;
            while j + 1 < words.len() && !(words[j] == 0 && words[j + 1] == 0) {
                stubs += 1;
                j += 2;
            }
            let end = (j + 2).min(words.len());
            out.push((name, stubs, i..end));
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// Convierte el ELF a IRX. También devuelve el rango de `.text`.
pub fn convert(elf: &[u8]) -> Result<Irx, String> {
    let secs = sections(elf)?;
    let entry = u32le(elf, 24);
    let e_flags = u32le(elf, 36);

    let alloc: Vec<&Section> = secs.iter().filter(|s| s.flags & SHF_ALLOC != 0 && s.size > 0).collect();
    let text = alloc.iter().find(|s| s.name == ".text").ok_or("el módulo no tiene sección .text")?;
    if text.addr != 0 {
        return Err("el módulo debe enlazarse en la dirección 0 (linker script del IOP)".into());
    }
    let text_size = (text.size as u32 + 15) & !15;
    let file_end = alloc
        .iter()
        .filter(|s| s.ty == SHT_PROGBITS)
        .map(|s| s.addr + s.size as u32)
        .max()
        .unwrap_or(text_size)
        .max(text_size);
    let mem_end = alloc.iter().map(|s| s.addr + s.size as u32).max().unwrap_or(file_end).max(file_end);

    // Imagen cargable (antes de añadir palabras auxiliares).
    let mut image = vec![0u8; ((file_end + 15) & !15) as usize];
    for s in alloc.iter().filter(|s| s.ty == SHT_PROGBITS) {
        if s.offset + s.size > elf.len() {
            return Err(format!("la sección {} se sale del archivo", s.name));
        }
        image[s.addr as usize..s.addr as usize + s.size].copy_from_slice(&elf[s.offset..s.offset + s.size]);
    }

    // Símbolos: (valor, sección).
    let symtab = secs.iter().find(|s| s.ty == SHT_SYMTAB).ok_or("el ELF no tiene tabla de símbolos")?;
    let strtab = secs.get(symtab.link as usize).filter(|s| s.ty == SHT_STRTAB).ok_or("tabla de cadenas inválida")?;
    let nsyms = symtab.size / 16;
    let sym = |i: usize| -> (u32, u16) {
        let o = symtab.offset + i * 16;
        (u32le(elf, o + 4), u16le(elf, o + 14))
    };
    let mut by_name: HashMap<String, u32> = HashMap::new();
    for i in 0..nsyms {
        let name = cstr(elf, strtab.offset + u32le(elf, symtab.offset + i * 16) as usize);
        if !name.is_empty() && sym(i).1 != 0 {
            by_name.insert(name, sym(i).0);
        }
    }

    let read_entries = |rel: &Section| -> Vec<(u32, u32, usize)> {
        (0..rel.size / 8)
            .map(|i| {
                let o = rel.offset + i * 8;
                let info = u32le(elf, o + 4);
                (u32le(elf, o), info & 0xff, (info >> 8) as usize)
            })
            .collect()
    };

    // Corrección de HI/LO. Destinos de salto conocidos desde fuera del
    // código: símbolos y palabras de datos que apuntan al texto (tablas de
    // saltos).
    let mut targets: HashSet<usize> = (0..nsyms).map(sym).filter(|&(v, sh)| sh != 0 && sh != SHN_ABS && v < text_size).map(|(v, _)| v as usize / 4).collect();
    for rel in secs.iter().filter(|s| s.ty == SHT_REL) {
        for (offset, ty, _) in read_entries(rel) {
            if ty == R_MIPS_32 && offset as usize + 4 <= image.len() && u32le(&image, offset as usize) < text_size {
                targets.insert(u32le(&image, offset as usize) as usize / 4);
            }
        }
    }
    let tramp_offset = by_name.get("__irxtramp_start").copied().unwrap_or(0);
    let hilo = mips1::fix_hilo(&mut image[..text.size & !3], 0, tramp_offset, &targets)?;
    let tramp_size = (hilo.code.len() as u32 + 15) & !15;
    let no_room = "falta el símbolo __irxtramp_start del linker script (detrás de .bss)";
    let has_room = tramp_offset >= mem_end && tramp_offset % 16 == 0;
    // Palabras auxiliares de las relocaciones: detrás de los trampolines.
    let fix_base = tramp_offset + tramp_size;
    let mut fix_words: Vec<u32> = Vec::new();
    if hilo.sites > 0 {
        if !has_room {
            return Err(no_room.into());
        }
        // Los trampolines van detrás de .bss, que pasa a estar en el archivo.
        image.resize((tramp_offset + tramp_size) as usize, 0);
        image[tramp_offset as usize..tramp_offset as usize + hilo.code.len()].copy_from_slice(&hilo.code);
    }
    let moved: HashMap<u32, u32> = hilo.moved.iter().copied().collect();

    // Relocaciones.
    // (desplazamiento, tipo, ¿viene de una sección de datos?). El orden se
    // conserva: cada HI16 debe ir seguido de su LO16 en la misma tabla.
    let mut out_rel: Vec<(u32, u32, bool)> = Vec::new();
    let mut helpers = 0;
    // Palabra auxiliar (compartida) para el HI16 que precede a un LO16 suelto.
    let mut dummy_hi: Option<u32> = None;
    for rel in secs.iter().filter(|s| s.ty == SHT_REL) {
        let Some(target) = secs.get(rel.info as usize) else { continue };
        if target.flags & SHF_ALLOC == 0 {
            continue;
        }
        let in_data = target.addr >= text_size;
        // Las relocaciones siguen a las instrucciones que se fueron a un trampolín.
        let entries: Vec<(u32, u32, usize)> =
            read_entries(rel).into_iter().map(|(offset, ty, s)| (moved.get(&offset).copied().unwrap_or(offset), ty, s)).collect();
        // Índice (en esta tabla) del LO16 que va emparejado con el HI16 anterior.
        let mut natural_lo: Option<usize> = None;
        for (i, &(offset, ty, symidx)) in entries.iter().enumerate() {
            if ty == R_MIPS_NONE || ty == R_MIPS_PC16 || ty == R_MIPS_JALR {
                continue;
            }
            if symidx >= nsyms {
                return Err(format!("relocación en {offset:#x} con símbolo inválido"));
            }
            let (_, shndx) = sym(symidx);
            // Los símbolos absolutos y los débiles sin definir no se mueven.
            if shndx == SHN_ABS || shndx == 0 {
                continue;
            }
            if offset as usize + 4 > image.len() {
                return Err(format!("relocación en {offset:#x} fuera de la imagen"));
            }
            match ty {
                R_MIPS_32 | R_MIPS_26 => out_rel.push((offset, ty, in_data)),
                R_MIPS_LO16 => {
                    if natural_lo != Some(i) {
                        // LO16 suelto: se le antepone un HI16 auxiliar.
                        let word = match dummy_hi {
                            Some(w) => w,
                            None => {
                                let w = fix_base + 4 * fix_words.len() as u32;
                                fix_words.push(0);
                                dummy_hi = Some(w);
                                w
                            }
                        };
                        out_rel.push((word, R_MIPS_HI16, in_data));
                        helpers += 1;
                    }
                    out_rel.push((offset, R_MIPS_LO16, in_data));
                }
                R_MIPS_HI16 => {
                    // Su LO16: la siguiente entrada LO16 con el mismo símbolo.
                    let pair = entries[i + 1..].iter().position(|&(_, t, s)| t == R_MIPS_LO16 && s == symidx);
                    let Some(p) = pair else {
                        return Err(format!("R_MIPS_HI16 en {offset:#x} sin su R_MIPS_LO16"));
                    };
                    out_rel.push((offset, R_MIPS_HI16, in_data));
                    if p == 0 {
                        natural_lo = Some(i + 1);
                    } else {
                        // No es la entrada siguiente: palabra auxiliar con
                        // los mismos 16 bits bajos.
                        let lo_offset = entries[i + 1 + p].0 as usize;
                        let lo = u32le(&image, lo_offset) & 0xffff;
                        out_rel.push((fix_base + 4 * fix_words.len() as u32, R_MIPS_LO16, in_data));
                        fix_words.push(lo);
                        helpers += 1;
                    }
                }
                other => {
                    return Err(format!(
                        "relocación de tipo {other} en {offset:#x}: el cargador del IOP no la admite \
                         (¿código dependiente de $gp o PIC?)"
                    ));
                }
            }
        }
    }
    // Los saltos a los trampolines y de vuelta.
    for &offset in &hilo.jumps {
        out_rel.push((offset, R_MIPS_26, false));
    }
    if !fix_words.is_empty() {
        if !has_room {
            return Err(no_room.into());
        }
        image.resize(fix_base as usize, 0);
        image.extend(fix_words.iter().flat_map(|w| w.to_le_bytes()));
        image.resize((image.len() + 15) & !15, 0);
    }
    let file_size = image.len() as u32;
    let mem_size = ((mem_end.max(file_size)) + 15) & !15;
    let data_size = file_size - text_size;
    // Para las cabeceras de sección: datos propiamente dichos, trampolines y
    // palabras auxiliares.
    let appended = hilo.sites > 0 || !fix_words.is_empty();
    let (plain_data, tramp_addr, fix_addr) = if appended { (tramp_offset - text_size, tramp_offset, fix_base) } else { (data_size, file_size, file_size) };
    let bss_size = mem_size - file_size;

    // Identidad del módulo: struct { *const u8 nombre; u16 versión }.
    let (module_info, name, version) = match by_name.get("_irx_id") {
        Some(&addr) if addr as usize + 8 <= image.len() => {
            let name_ptr = u32le(&image, addr as usize) as usize;
            let v = u16le(&image, addr as usize + 4);
            let name = if name_ptr < image.len() { cstr(&image, name_ptr) } else { String::new() };
            (addr, name, ((v >> 8) as u8, v as u8))
        }
        _ => return Err("falta el símbolo _irx_id (usa ps2_iop::module!)".into()),
    };
    if name.is_empty() || name.len() > 55 {
        return Err("el nombre del módulo debe tener entre 1 y 55 caracteres".into());
    }

    let imports = import_tables(&image[..text_size as usize]).into_iter().map(|(n, c, _)| (n, c)).collect();

    // .iopmod
    let mut iopmod = vec![0u8; 26];
    put32(&mut iopmod, 0, module_info);
    put32(&mut iopmod, 4, entry);
    put32(&mut iopmod, 8, 0); // gp: no se usa
    put32(&mut iopmod, 12, text_size);
    put32(&mut iopmod, 16, data_size);
    put32(&mut iopmod, 20, bss_size);
    put16(&mut iopmod, 24, (version.0 as u16) << 8 | version.1 as u16);
    iopmod.extend_from_slice(name.as_bytes());
    iopmod.push(0);
    while iopmod.len() % 4 != 0 {
        iopmod.push(0);
    }

    let (rel_text, rel_data): (Vec<_>, Vec<_>) = out_rel.iter().partition(|r| !r.2);
    let names = ["", ".text", ".data", ".bss", ".iopmod", ".rel.text", ".rel.data", ".tramp", ".relfix", ".shstrtab"];
    let mut shstr = Vec::new();
    let mut name_off = Vec::new();
    for n in names {
        name_off.push(shstr.len() as u32);
        shstr.extend_from_slice(n.as_bytes());
        shstr.push(0);
    }
    while shstr.len() % 4 != 0 {
        shstr.push(0);
    }

    // Disposición del archivo.
    let ph_base = 52usize;
    let alloc_base = (ph_base + 2 * 32 + 15) & !15;
    let iopmod_base = alloc_base + image.len();
    let sh_base = iopmod_base + iopmod.len();
    let rel_base = sh_base + names.len() * 40;
    let rel_data_base = rel_base + rel_text.len() * 8;
    let shstr_base = rel_data_base + rel_data.len() * 8;
    let total = shstr_base + shstr.len();
    let mut out = vec![0u8; total];

    // Cabecera ELF.
    out[..16].copy_from_slice(&elf[..16]);
    put16(&mut out, 16, ET_IOPRELEXEC);
    put16(&mut out, 18, 8);
    put32(&mut out, 20, 1);
    put32(&mut out, 24, entry);
    put32(&mut out, 28, ph_base as u32);
    put32(&mut out, 32, sh_base as u32);
    put32(&mut out, 36, e_flags);
    put16(&mut out, 40, 52);
    put16(&mut out, 42, 32);
    put16(&mut out, 44, 2);
    put16(&mut out, 46, 40);
    put16(&mut out, 48, names.len() as u16);
    put16(&mut out, 50, names.len() as u16 - 1);

    // Cabeceras de programa: .iopmod y el segmento cargable.
    let ph = |out: &mut [u8], i: usize, v: [u32; 8]| {
        for (k, x) in v.iter().enumerate() {
            put32(out, ph_base + i * 32 + k * 4, *x);
        }
    };
    ph(&mut out, 0, [SHT_IOPMOD, iopmod_base as u32, 0, 0, iopmod.len() as u32, 0, 4, 4]);
    ph(&mut out, 1, [1, alloc_base as u32, 0, 0, file_size, mem_size, 7, 16]);

    out[alloc_base..alloc_base + image.len()].copy_from_slice(&image);
    out[iopmod_base..iopmod_base + iopmod.len()].copy_from_slice(&iopmod);

    // Cabeceras de sección: nombre, tipo, flags, addr, offset, size, link, info, align, entsize.
    let sh = |out: &mut [u8], i: usize, v: [u32; 10]| {
        for (k, x) in v.iter().enumerate() {
            put32(out, sh_base + i * 40 + k * 4, *x);
        }
    };
    sh(&mut out, 1, [name_off[1], SHT_PROGBITS, 6, 0, alloc_base as u32, text_size, 0, 0, 16, 0]);
    sh(&mut out, 2, [name_off[2], SHT_PROGBITS, 3, text_size, alloc_base as u32 + text_size, plain_data, 0, 0, 16, 0]);
    sh(&mut out, 3, [name_off[3], SHT_NOBITS, 3, file_size, alloc_base as u32 + file_size, bss_size, 0, 0, 16, 0]);
    sh(&mut out, 4, [name_off[4], SHT_IOPMOD, 0, 0, iopmod_base as u32, iopmod.len() as u32, 0, 0, 4, 0]);
    sh(&mut out, 5, [name_off[5], SHT_REL, 0, 0, rel_base as u32, rel_text.len() as u32 * 8, 0, 1, 4, 8]);
    sh(&mut out, 6, [name_off[6], SHT_REL, 0, 0, rel_data_base as u32, rel_data.len() as u32 * 8, 0, 2, 4, 8]);
    sh(&mut out, 7, [name_off[7], SHT_PROGBITS, 6, tramp_addr, alloc_base as u32 + tramp_addr, tramp_size, 0, 0, 16, 0]);
    sh(&mut out, 8, [name_off[8], SHT_PROGBITS, 3, fix_addr, alloc_base as u32 + fix_addr, file_size - fix_addr, 0, 0, 4, 0]);
    sh(&mut out, 9, [name_off[9], SHT_STRTAB, 0, 0, shstr_base as u32, shstr.len() as u32, 0, 0, 1, 0]);

    for (i, (offset, ty, _)) in rel_text.iter().chain(rel_data.iter()).enumerate() {
        put32(&mut out, rel_base + i * 8, *offset);
        put32(&mut out, rel_base + i * 8 + 4, *ty);
    }
    out[shstr_base..shstr_base + shstr.len()].copy_from_slice(&shstr);

    Ok(Irx {
        bytes: out,
        name,
        version,
        text_size,
        data_size,
        bss_size,
        relocs: out_rel.len(),
        helpers,
        hilo_fixes: hilo.sites,
        imports,
    })
}

/// Lee de un IRX el rango de archivo de `.text` y su tamaño.
#[cfg(test)]
pub fn text_range(irx: &[u8]) -> Result<(usize, usize), String> {
    Ok(code_ranges(irx)?[0])
}

/// Rangos de archivo (desplazamiento, tamaño) con código: `.text` y, si los
/// hay, los trampolines de `.tramp`.
pub fn code_ranges(irx: &[u8]) -> Result<Vec<(usize, usize)>, String> {
    let secs = sections(irx)?;
    if u16le(irx, 16) != ET_IOPRELEXEC {
        return Err("no es un módulo IRX".into());
    }
    let text = secs.iter().find(|s| s.name == ".text").ok_or("IRX sin .text")?;
    let mut ranges = vec![(text.offset, text.size)];
    if let Some(tramp) = secs.iter().find(|s| s.name == ".tramp" && s.size > 0) {
        ranges.push((tramp.offset, tramp.size));
    }
    if ranges.iter().any(|&(offset, size)| offset + size > irx.len()) {
        return Err("IRX truncado".into());
    }
    Ok(ranges)
}

/// Aplica las relocaciones de un IRX como lo hace el cargador de la BIOS y
/// devuelve la imagen cargada en `base`. Sirve para comprobar el conversor.
pub fn load_at(irx: &[u8], base: u32) -> Vec<u8> {
    let secs = sections(irx).unwrap();
    let ph = u32le(irx, 28) as usize + 32;
    let (off, filesz, memsz) = (u32le(irx, ph + 4) as usize, u32le(irx, ph + 16) as usize, u32le(irx, ph + 20) as usize);
    let mut mem = vec![0u8; memsz];
    mem[..filesz].copy_from_slice(&irx[off..off + filesz]);
    for rel in secs.iter().filter(|s| s.ty == SHT_REL) {
        let entries: Vec<(usize, u32)> =
            (0..rel.size / 8).map(|i| (u32le(irx, rel.offset + i * 8) as usize, u32le(irx, rel.offset + i * 8 + 4) & 0xff)).collect();
        for (i, &(o, ty)) in entries.iter().enumerate() {
            let w = u32le(&mem, o);
            let new = match ty {
                R_MIPS_32 => w.wrapping_add(base),
                R_MIPS_26 => (w & 0xfc00_0000) | ((((w & 0x03ff_ffff) << 2).wrapping_add(base) >> 2) & 0x03ff_ffff),
                R_MIPS_HI16 => {
                    let lo = u32le(&mem, entries[i + 1].0) as u16 as i16 as i32;
                    let v = ((w & 0xffff) << 16).wrapping_add(lo as u32).wrapping_add(base);
                    (w & 0xffff_0000) | (((v >> 15) + 1) >> 1) & 0xffff
                }
                // Como la BIOS real: sólo si va justo detrás de un HI16.
                R_MIPS_LO16 if i > 0 && entries[i - 1].1 == R_MIPS_HI16 => (w & 0xffff_0000) | (w.wrapping_add(base) & 0xffff),
                R_MIPS_LO16 => w,
                _ => panic!("tipo {ty}"),
            };
            put32(&mut mem, o, new);
        }
    }
    mem
}

/// Comprueba el conversor contra el enlazador: `rebased` es el mismo módulo
/// enlazado en `base`; el IRX reubicado en `base` con las reglas del cargador
/// debe dar exactamente los mismos bytes (fuera de la zona de palabras
/// auxiliares). Devuelve cuántas palabras se compararon.
pub fn verify_relocation(irx: &[u8], rebased: &[u8], base: u32) -> Result<usize, String> {
    let loaded = load_at(irx, base);
    let secs = sections(rebased)?;
    let mut reference = vec![0u8; loaded.len()];
    for s in secs.iter().filter(|s| s.ty == SHT_PROGBITS && s.flags & SHF_ALLOC != 0 && s.size > 0) {
        let start = s.addr.checked_sub(base).ok_or("el ELF de referencia no está enlazado en la base indicada")? as usize;
        if start + s.size > reference.len() || s.offset + s.size > rebased.len() {
            return Err(format!("la sección {} del ELF de referencia no cabe en la imagen del IRX", s.name));
        }
        reference[start..start + s.size].copy_from_slice(&rebased[s.offset..s.offset + s.size]);
    }
    // La misma corrección de HI/LO que lleva el IRX, sobre la referencia.
    let irx_secs = sections(irx)?;
    let text_size = irx_secs.iter().find(|s| s.name == ".text").map_or(0, |s| s.size).min(reference.len());
    if let Some(tramp) = irx_secs.iter().find(|s| s.name == ".tramp" && s.size > 0) {
        let fix = mips1::fix_hilo(&mut reference[..text_size], base, tramp.addr, &HashSet::new())?;
        let start = tramp.addr as usize;
        if start + fix.code.len() > reference.len() {
            return Err("los trampolines de la referencia no caben en la imagen del IRX".into());
        }
        reference[start..start + fix.code.len()].copy_from_slice(&fix.code);
    }
    // Las palabras auxiliares no existen en la referencia.
    let fix = irx_secs.iter().find(|s| s.name == ".relfix").map_or(0..0, |s| s.addr as usize..s.addr as usize + s.size);
    let mut compared = 0;
    let mut different = Vec::new();
    for o in (0..loaded.len() & !3).step_by(4) {
        if fix.contains(&o) {
            continue;
        }
        compared += 1;
        if loaded[o..o + 4] != reference[o..o + 4] {
            different.push(format!("{o:#x}: IRX {:08x}, enlazador {:08x}", u32le(&loaded, o), u32le(&reference, o)));
        }
    }
    if different.is_empty() {
        Ok(compared)
    } else {
        Err(format!(
            "el IRX reubicado en {base:#x} difiere del enlazador en {} palabras ({})",
            different.len(),
            different.iter().take(8).cloned().collect::<Vec<_>>().join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ELF mínimo enlazado en 0, con las relocaciones que emite `--emit-relocs`.
    fn sample_elf() -> Vec<u8> {
        let mut text = vec![0u8; 0x40];
        for (offset, word) in [
            (0x10, 0x3c04_0000u32), // lui   a0, %hi(d)
            (0x14, 0x2484_0048),    // addiu a0, a0, %lo(d)
            (0x18, 0x8c85_004c),    // lw    a1, %lo(d+4)(a0)   <- LO16 sin HI16 propio
            (0x1c, 0x0c00_0004),    // jal   t
            (0x20, 0x3c06_0000),    // lui   a2, %hi(d+8)
            (0x24, 0x0c00_0004),    // jal   t                  <- separa el HI16 de su LO16
            (0x28, 0x24c6_0050),    // addiu a2, a2, %lo(d+8)
        ] {
            put32(&mut text, offset, word);
        }
        let mut data = vec![0u8; 0x40];
        put32(&mut data, 0x00, 0x48); // _irx_id.name -> d
        put16(&mut data, 0x04, 0x0102); // _irx_id.version
        data[0x08..0x0b].copy_from_slice(b"mod");

        let strtab = b"\0_irx_id\0d\0__irxtramp_start\0t\0".to_vec();
        let name_at = |n: &str| strtab.windows(n.len() + 1).position(|w| &w[..n.len()] == n.as_bytes() && w[n.len()] == 0).unwrap() as u32;
        let mut symtab = vec![0u8; 16];
        for (name, value, section) in [("_irx_id", 0x40u32, 2u16), ("d", 0x48, 2), ("__irxtramp_start", 0x80, 2), ("t", 0x10, 1)] {
            let mut sym = vec![0u8; 16];
            put32(&mut sym, 0, name_at(name));
            put32(&mut sym, 4, value);
            put16(&mut sym, 14, section);
            symtab.extend_from_slice(&sym);
        }
        let rel = |entries: &[(u32, u32, u32)]| -> Vec<u8> {
            entries.iter().flat_map(|&(offset, ty, sym)| [offset.to_le_bytes(), (sym << 8 | ty).to_le_bytes()].concat()).collect()
        };
        let rel_text = rel(&[
            (0x10, R_MIPS_HI16, 2),
            (0x14, R_MIPS_LO16, 2),
            (0x18, R_MIPS_LO16, 2),
            (0x1c, R_MIPS_26, 4),
            (0x20, R_MIPS_HI16, 2),
            (0x24, R_MIPS_26, 4),
            (0x28, R_MIPS_LO16, 2),
        ]);
        let rel_data = rel(&[(0x40, R_MIPS_32, 2)]);
        let shstr = b"\0.text\0.data\0.rel.text\0.rel.data\0.symtab\0.strtab\0.shstrtab\0".to_vec();
        let sh_name = |n: &str| shstr.windows(n.len() + 1).position(|w| &w[..n.len()] == n.as_bytes() && w[n.len()] == 0).unwrap() as u32;

        // (nombre, tipo, flags, addr, contenido, link, info)
        let parts: Vec<(&str, u32, u32, u32, Vec<u8>, u32, u32)> = vec![
            (".text", SHT_PROGBITS, 6, 0, text, 0, 0),
            (".data", SHT_PROGBITS, 3, 0x40, data, 0, 0),
            (".rel.text", SHT_REL, 0, 0, rel_text, 5, 1),
            (".rel.data", SHT_REL, 0, 0, rel_data, 5, 2),
            (".symtab", SHT_SYMTAB, 0, 0, symtab, 6, 0),
            (".strtab", SHT_STRTAB, 0, 0, strtab, 0, 0),
            (".shstrtab", SHT_STRTAB, 0, 0, shstr.clone(), 0, 0),
        ];
        let mut elf = vec![0u8; 52];
        elf[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
        put16(&mut elf, 16, 2);
        put16(&mut elf, 18, 8);
        put32(&mut elf, 24, 0x10); // entrada
        let mut headers = vec![0u8; 40];
        for (name, ty, flags, addr, content, link, info) in &parts {
            let mut h = vec![0u8; 40];
            for (k, v) in [sh_name(name), *ty, *flags, *addr, elf.len() as u32, content.len() as u32, *link, *info].iter().enumerate() {
                put32(&mut h, k * 4, *v);
            }
            headers.extend_from_slice(&h);
            elf.extend_from_slice(content);
        }
        let shoff = elf.len() as u32;
        put32(&mut elf, 32, shoff);
        put16(&mut elf, 46, 40);
        put16(&mut elf, 48, parts.len() as u16 + 1);
        put16(&mut elf, 50, parts.len() as u16);
        elf.extend_from_slice(&headers);
        elf
    }

    #[test]
    fn irx_header_and_module_info() {
        let irx = convert(&sample_elf()).unwrap();
        assert_eq!(u16le(&irx.bytes, 16), ET_IOPRELEXEC);
        assert_eq!(irx.name, "mod");
        assert_eq!(irx.version, (1, 2));
        // Dos palabras auxiliares detrás de los datos.
        assert_eq!((irx.text_size, irx.data_size, irx.bss_size), (0x40, 0x50, 0));
        // Primera cabecera de programa: .iopmod, con la dirección de _irx_id.
        let ph = u32le(&irx.bytes, 28) as usize;
        assert_eq!(u32le(&irx.bytes, ph), SHT_IOPMOD);
        let iopmod = u32le(&irx.bytes, ph + 4) as usize;
        assert_eq!(u32le(&irx.bytes, iopmod), 0x40);
        assert_eq!(u32le(&irx.bytes, iopmod + 4), 0x10);
        assert_eq!(text_range(&irx.bytes).unwrap().1, 0x40);
    }

    #[test]
    fn every_lo16_follows_a_hi16() {
        let irx = convert(&sample_elf()).unwrap();
        // LO16 suelto, HI16 separado de su LO16 y el LO16 que quedó suelto.
        assert_eq!(irx.helpers, 3);
        let secs = sections(&irx.bytes).unwrap();
        for rel in secs.iter().filter(|s| s.ty == SHT_REL) {
            let types: Vec<u32> = (0..rel.size / 8).map(|i| u32le(&irx.bytes, rel.offset + i * 8 + 4) & 0xff).collect();
            for (i, &ty) in types.iter().enumerate() {
                if ty == R_MIPS_LO16 {
                    assert_eq!(types[i - 1], R_MIPS_HI16, "LO16 sin HI16 delante en {}", rel.name);
                }
                if ty == R_MIPS_HI16 {
                    assert_eq!(types[i + 1], R_MIPS_LO16, "HI16 sin LO16 detrás en {}", rel.name);
                }
            }
        }
    }

    #[test]
    fn loader_relocation_matches_hand_computed_values() {
        let irx = convert(&sample_elf()).unwrap();
        // Base con acarreo de la mitad baja a la alta: d queda en 0x130038.
        let mem = load_at(&irx.bytes, 0x0012_fff0);
        let expected = [
            (0x10, 0x3c04_0013u32),
            (0x14, 0x2484_0038),
            (0x18, 0x8c85_003c),
            (0x1c, 0x0c04_c000),
            (0x20, 0x3c06_0013),
            (0x24, 0x0c04_c000),
            (0x28, 0x24c6_0040),
            (0x40, 0x0013_0038),
        ];
        for (offset, word) in expected {
            assert_eq!(u32le(&mem, offset), word, "palabra en {offset:#x}");
        }
        // La base 0 no cambia nada.
        let same = load_at(&irx.bytes, 0);
        let (text, _) = text_range(&irx.bytes).unwrap();
        assert_eq!(&same[..0x80], &irx.bytes[text..text + 0x80]);
    }

    #[test]
    fn rejects_modules_without_identity() {
        let mut elf = sample_elf();
        // Renombra `_irx_id` en la tabla de cadenas.
        let pos = elf.windows(7).position(|w| w == b"_irx_id").unwrap();
        elf[pos] = b'x';
        assert!(convert(&elf).err().unwrap().contains("_irx_id"));
    }
}

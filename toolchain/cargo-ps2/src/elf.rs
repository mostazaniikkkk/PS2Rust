//! Lector mínimo de ELF32 little-endian, suficiente para localizar el código.

pub struct CodeRange {
    pub name: String,
    pub offset: usize,
    pub size: usize,
    pub addr: u32,
}

pub struct Elf {
    pub entry: u32,
    pub code: Vec<CodeRange>,
    pub load_start: u32,
    pub load_end: u32,
}

fn u16le(b: &[u8], o: usize) -> Result<u16, String> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| "ELF truncado".to_string())
}

fn u32le(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| "ELF truncado".to_string())
}

pub fn parse(b: &[u8]) -> Result<Elf, String> {
    if b.len() < 52 || &b[0..4] != b"\x7fELF" {
        return Err("no es un archivo ELF".into());
    }
    if b[4] != 1 || b[5] != 1 {
        return Err("se esperaba ELF de 32 bits little-endian".into());
    }
    if u16le(b, 16)? != 2 {
        return Err("se esperaba un ejecutable (ET_EXEC)".into());
    }
    if u16le(b, 18)? != 8 {
        return Err("la arquitectura del ELF no es MIPS".into());
    }
    let entry = u32le(b, 24)?;
    let phoff = u32le(b, 28)? as usize;
    let shoff = u32le(b, 32)? as usize;
    let phentsize = u16le(b, 42)? as usize;
    let phnum = u16le(b, 44)? as usize;
    let shentsize = u16le(b, 46)? as usize;
    let shnum = u16le(b, 48)? as usize;
    let shstrndx = u16le(b, 50)? as usize;

    let mut load_start = u32::MAX;
    let mut load_end = 0u32;
    let mut exec_segments = Vec::new();
    for i in 0..phnum {
        let p = phoff + i * phentsize;
        if u32le(b, p)? != 1 {
            continue; // PT_LOAD
        }
        let offset = u32le(b, p + 4)? as usize;
        let vaddr = u32le(b, p + 8)?;
        let filesz = u32le(b, p + 16)? as usize;
        let memsz = u32le(b, p + 20)?;
        let flags = u32le(b, p + 24)?;
        load_start = load_start.min(vaddr);
        load_end = load_end.max(vaddr.wrapping_add(memsz));
        if flags & 1 != 0 {
            exec_segments.push(CodeRange { name: format!("segmento {i}"), offset, size: filesz, addr: vaddr });
        }
    }
    if load_end == 0 {
        return Err("el ELF no tiene segmentos cargables".into());
    }

    let mut code = Vec::new();
    if shoff != 0 && shnum != 0 {
        let strtab = {
            let s = shoff + shstrndx * shentsize;
            (u32le(b, s + 16)? as usize, u32le(b, s + 20)? as usize)
        };
        for i in 0..shnum {
            let s = shoff + i * shentsize;
            let name_off = u32le(b, s)? as usize;
            let ty = u32le(b, s + 4)?;
            let flags = u32le(b, s + 8)?;
            let addr = u32le(b, s + 12)?;
            let offset = u32le(b, s + 16)? as usize;
            let size = u32le(b, s + 20)? as usize;
            // SHT_PROGBITS con SHF_ALLOC | SHF_EXECINSTR
            if ty == 1 && flags & 4 != 0 && flags & 2 != 0 && size > 0 {
                let name = b
                    .get(strtab.0 + name_off..strtab.0 + strtab.1)
                    .and_then(|s| s.split(|&c| c == 0).next())
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .unwrap_or_default();
                code.push(CodeRange { name, offset, size, addr });
            }
        }
    }
    if code.is_empty() {
        // Sin tabla de secciones no se puede separar código de datos: sólo es
        // seguro si el segmento ejecutable no contiene datos.
        return Err("el ELF no tiene tabla de secciones (¿strip total?): no puedo distinguir código de datos".into());
    }
    let _ = exec_segments;
    for c in &code {
        if c.offset + c.size > b.len() {
            return Err(format!("la sección {} se sale del archivo", c.name));
        }
    }
    Ok(Elf { entry, code, load_start, load_end })
}

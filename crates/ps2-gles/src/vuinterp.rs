//! Intérprete de la VU1 para el PC.
//!
//! Ejecuta el microprograma de `vu1prog.rs` con las mismas reglas que el
//! hardware, para probar su lógica sin consola. Sólo conoce las instrucciones
//! que usa ese microprograma, y además vigila tres trampas de la VU real, en
//! las que falla en vez de dar un resultado dudoso:
//!
//! * leer Q sin haber esperado (`WAITQ`) a la división o la raíz;
//! * leer el registro de recorte antes de 4 instrucciones desde el `CLIP`;
//! * saltar según un registro entero escrito en la instrucción anterior.

use crate::vu::{E_BIT, LNOP, NOP};

pub struct Vu {
    pub vf: [[f32; 4]; 32],
    pub vi: [u16; 16],
    pub acc: [f32; 4],
    pub q: f32,
    /// Memoria de datos: 1024 palabras de 128 bits.
    pub mem: Vec<[u32; 4]>,
    /// Paquetes enviados al GS con XGKICK.
    pub kicked: Vec<Vec<[u32; 4]>>,
    clip_flags: u32,
    /// Instrucciones ejecutadas desde el último CLIP.
    since_clip: u32,
    /// Q pendiente de un `WAITQ`.
    q_pending: bool,
    /// Registro entero escrito por la instrucción anterior.
    last_int_write: Option<u32>,
    /// Instrucciones ejecutadas en total.
    pub executed: u64,
}

fn clamp(v: f32) -> f32 {
    // La VU no tiene infinitos ni NaN: satura.
    if v.is_nan() {
        f32::MAX
    } else {
        v.clamp(-f32::MAX, f32::MAX)
    }
}

impl Vu {
    pub fn new() -> Vu {
        let mut vf = [[0.0; 4]; 32];
        vf[0] = [0.0, 0.0, 0.0, 1.0];
        Vu { vf, vi: [0; 16], acc: [0.0; 4], q: 0.0, mem: vec![[0; 4]; 1024], kicked: Vec::new(), clip_flags: 0, since_clip: 100, q_pending: false, last_int_write: None, executed: 0 }
    }

    fn set_vf(&mut self, reg: u32, dest: u32, value: [f32; 4]) {
        if reg == 0 {
            return;
        }
        for i in 0..4 {
            if dest & (8 >> i) != 0 {
                self.vf[reg as usize][i] = clamp(value[i]);
            }
        }
    }

    fn set_vi(&mut self, reg: u32, value: u16) {
        if reg != 0 {
            self.vi[reg as usize] = value;
        }
    }

    fn q(&self) -> f32 {
        assert!(!self.q_pending, "se lee Q sin WAITQ");
        self.q
    }

    fn upper(&mut self, op: u32) {
        let (dest, ft, fs, fd) = ((op >> 21) & 15, ((op >> 16) & 31) as usize, ((op >> 11) & 31) as usize, (op >> 6) & 31);
        let (s, t, acc) = (self.vf[fs], self.vf[ft], self.acc);
        let each = |f: &dyn Fn(usize) -> f32| [f(0), f(1), f(2), f(3)];
        let code = op & 0x3F;
        if code < 0x3C {
            let bc = t[(code & 3) as usize];
            let value = match code {
                0x00..=0x03 => each(&|i| s[i] + bc),
                0x04..=0x07 => each(&|i| s[i] - bc),
                0x08..=0x0B => each(&|i| acc[i] + s[i] * bc),
                0x10..=0x13 => each(&|i| s[i].max(bc)),
                0x14..=0x17 => each(&|i| s[i].min(bc)),
                0x18..=0x1B => each(&|i| s[i] * bc),
                0x1C => {
                    let q = self.q();
                    each(&|i| s[i] * q)
                }
                0x20 => {
                    let q = self.q();
                    each(&|i| s[i] + q)
                }
                0x21 => {
                    let q = self.q();
                    each(&|i| acc[i] + s[i] * q)
                }
                0x28 => each(&|i| s[i] + t[i]),
                0x29 => each(&|i| acc[i] + s[i] * t[i]),
                0x2A => each(&|i| s[i] * t[i]),
                0x2B => each(&|i| s[i].max(t[i])),
                0x2C => each(&|i| s[i] - t[i]),
                0x2E => [acc[0] - s[1] * t[2], acc[1] - s[2] * t[0], acc[2] - s[0] * t[1], 0.0],
                0x2F => each(&|i| s[i].min(t[i])),
                _ => panic!("instrucción superior desconocida {op:#010x}"),
            };
            self.set_vf(fd, dest, value);
            return;
        }
        let index = (fd << 2) | (code & 3);
        let set_acc = |vu: &mut Vu, value: [f32; 4]| {
            for i in 0..4 {
                if dest & (8 >> i) != 0 {
                    vu.acc[i] = clamp(value[i]);
                }
            }
        };
        let bc = t[(index & 3) as usize];
        match index {
            0x00..=0x03 => set_acc(self, each(&|i| s[i] + bc)),
            0x08..=0x0B => set_acc(self, each(&|i| acc[i] + s[i] * bc)),
            0x18..=0x1B => set_acc(self, each(&|i| s[i] * bc)),
            0x14 => self.set_vf(ft as u32, dest, each(&|i| f32::from_bits(s[i] as i32 as u32))),
            0x15 => self.set_vf(ft as u32, dest, each(&|i| f32::from_bits((s[i] * 16.0) as i32 as u32))),
            0x1D => self.set_vf(ft as u32, dest, each(&|i| s[i].abs())),
            0x1F => {
                let w = t[3].abs();
                let mut flags = 0;
                for i in 0..3 {
                    flags |= ((s[i] > w) as u32) << (i * 2);
                    flags |= ((s[i] < -w) as u32) << (i * 2 + 1);
                }
                self.clip_flags = (self.clip_flags << 6 | flags) & 0xFF_FFFF;
                self.since_clip = 0;
            }
            0x2E => set_acc(self, [s[1] * t[2], s[2] * t[0], s[0] * t[1], 0.0]),
            0x2F => {}
            _ => panic!("instrucción superior desconocida {op:#010x}"),
        }
    }

    /// Ejecuta la instrucción inferior. Devuelve el destino si es un salto
    /// que se toma.
    fn lower(&mut self, op: u32, pc: usize) -> Option<usize> {
        let (dest, it, is) = ((op >> 21) & 15, (op >> 16) & 31, (op >> 11) & 31);
        let imm11 = ((op & 0x7FF) as i32) << 21 >> 21;
        let address = |vu: &Vu, base: u32| (vu.vi[base as usize] as i32 + imm11) as usize & 0x3FF;
        let imm15 = (op >> 10) & 0x7800 | op & 0x7FF;
        let branch_to = (pc as i32 + 1 + imm11) as usize;
        let check = |vu: &Vu, reg: u32| assert!(vu.last_int_write != Some(reg) || reg == 0, "salto según vi{reg}, escrito en la instrucción anterior");
        let mut int_write = None;
        let mut target = None;
        match op >> 25 {
            0x00 => {
                let value = self.mem[address(self, is)];
                self.set_vf(it, dest, value.map(f32::from_bits));
            }
            0x01 => {
                // sq fs, imm(it): fs va en el campo `is`.
                let at = address(self, it);
                for i in 0..4 {
                    if dest & (8 >> i) != 0 {
                        self.mem[at][i] = self.vf[is as usize][i].to_bits();
                    }
                }
            }
            0x04 => {
                let word = self.mem[address(self, is)];
                let i = (0..4).find(|i| dest & (8 >> i) != 0).unwrap();
                self.set_vi(it, word[i] as u16);
                int_write = Some(it);
            }
            0x05 => {
                let at = address(self, is);
                for i in 0..4 {
                    if dest & (8 >> i) != 0 {
                        self.mem[at][i] = self.vi[it as usize] as u32;
                    }
                }
            }
            0x08 => {
                self.set_vi(it, self.vi[is as usize].wrapping_add(imm15 as u16));
                int_write = Some(it);
            }
            0x09 => {
                self.set_vi(it, self.vi[is as usize].wrapping_sub(imm15 as u16));
                int_write = Some(it);
            }
            0x12 => {
                assert!(self.since_clip >= 5, "se lee el registro de recorte demasiado pronto tras CLIP");
                self.set_vi(1, (self.clip_flags & op & 0xFF_FFFF != 0) as u16);
                int_write = Some(1);
            }
            0x20 => target = Some(branch_to),
            0x21 => {
                self.set_vi(it, pc as u16 + 2);
                int_write = Some(it);
                target = Some(branch_to);
            }
            0x24 => {
                check(self, is);
                target = Some(self.vi[is as usize] as usize);
            }
            0x28 | 0x29 => {
                check(self, it);
                check(self, is);
                let equal = self.vi[it as usize] == self.vi[is as usize];
                if equal == (op >> 25 == 0x28) {
                    target = Some(branch_to);
                }
            }
            0x2C => {
                check(self, is);
                if (self.vi[is as usize] as i16) < 0 {
                    target = Some(branch_to);
                }
            }
            0x40 => {
                let id = (op >> 6) & 31;
                match op & 0x3F {
                    0x30 => {
                        self.set_vi(id, self.vi[is as usize].wrapping_add(self.vi[it as usize]));
                        int_write = Some(id);
                    }
                    0x31 => {
                        self.set_vi(id, self.vi[is as usize].wrapping_sub(self.vi[it as usize]));
                        int_write = Some(id);
                    }
                    0x35 => {
                        self.set_vi(id, self.vi[is as usize] | self.vi[it as usize]);
                        int_write = Some(id);
                    }
                    low @ 0x3C..=0x3F => {
                        let (fsf, ftf) = ((dest & 3) as usize, (dest >> 2) as usize);
                        match id << 2 | (low & 3) {
                            0x30 => {} // move vf0, vf0
                            0x38 => {
                                let (a, b) = (self.vf[is as usize][fsf], self.vf[it as usize][ftf]);
                                self.q = if b == 0.0 { f32::MAX.copysign(a * b.signum()) } else { clamp(a / b) };
                                self.q_pending = true;
                            }
                            0x3A => {
                                let (a, b) = (self.vf[is as usize][fsf], self.vf[it as usize][ftf].abs());
                                self.q = if b == 0.0 { f32::MAX.copysign(a) } else { clamp(a / b.sqrt()) };
                                self.q_pending = true;
                            }
                            0x3B => self.q_pending = false,
                            0x3C => {
                                self.set_vi(it, self.vf[is as usize][fsf].to_bits() as u16);
                                int_write = Some(it);
                            }
                            0x6C => {
                                // El paquete llega hasta su etiqueta con EOP.
                                let start = self.vi[is as usize] as usize;
                                let tag = self.mem[start];
                                assert!(tag[0] & 0x8000 != 0, "XGKICK de un paquete sin EOP");
                                let nreg = match tag[1] >> 28 {
                                    0 => 16,
                                    n => n as usize,
                                };
                                let len = 1 + (tag[0] & 0x7FFF) as usize * nreg;
                                self.kicked.push(self.mem[start..start + len].to_vec());
                            }
                            other => panic!("instrucción inferior desconocida {op:#010x} ({other:#x})"),
                        }
                    }
                    _ => panic!("instrucción inferior desconocida {op:#010x}"),
                }
            }
            _ => panic!("instrucción inferior desconocida {op:#010x}"),
        }
        self.last_int_write = int_write;
        target
    }

    /// Registros de coma flotante que lee o escribe una instrucción, para
    /// comprobar que las dos de una misma posición no se pisan.
    fn touches(lower: u32, upper: u32) -> bool {
        if lower == LNOP || upper & !E_BIT == NOP {
            return false;
        }
        let upper_regs = [(upper >> 16) & 31, (upper >> 11) & 31, (upper >> 6) & 31];
        let lower_writes = match lower >> 25 {
            0x00 => Some((lower >> 16) & 31),
            _ => None,
        };
        lower_writes.is_some_and(|r| r != 0 && upper_regs.contains(&r))
    }

    /// Ejecuta el microprograma desde `start` hasta su bit E.
    pub fn run(&mut self, code: &[u64], start: usize) {
        let mut pc = start;
        // Salto pendiente: se toma tras la instrucción siguiente.
        let mut pending: Option<usize> = None;
        let mut ending = false;
        loop {
            let (lower, upper) = (code[pc] as u32, (code[pc] >> 32) as u32);
            assert!(!Vu::touches(lower, upper), "las dos instrucciones de la posición {pc} usan el mismo registro");
            self.executed += 1;
            assert!(self.executed < 50_000_000, "el microprograma no termina");
            // La superior lee los registros antes de que la inferior escriba.
            self.upper(upper & !E_BIT);
            self.since_clip += 1;
            let jump = self.lower(lower, pc);
            if ending {
                return;
            }
            ending = upper & E_BIT != 0;
            let next = pending.take().unwrap_or(pc + 1);
            if jump.is_some() {
                assert!(pending.is_none(), "salto en el hueco de otro salto");
                pending = jump;
            }
            pc = next;
        }
    }
}

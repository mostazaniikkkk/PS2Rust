//! Ensamblador de microcódigo para las unidades vectoriales (VU) del
//! Emotion Engine.
//!
//! LLVM no sabe generar código para las VU, así que el microprograma de
//! `ps2-gles` se monta con este ensamblador mínimo, escrito en Rust: una
//! función por instrucción, que devuelve su codificación, y un [`Asm`] que
//! las va colocando y resuelve las etiquetas de los saltos.
//!
//! Cada posición del microprograma lleva dos instrucciones que se ejecutan a
//! la vez: una "superior" (aritmética de vectores en coma flotante) y una
//! "inferior" (enteros, memoria, saltos, divisiones).

/// Componentes de destino de una instrucción.
pub const X: u32 = 8;
pub const Y: u32 = 4;
pub const Z: u32 = 2;
pub const W: u32 = 1;
pub const XY: u32 = X | Y;
pub const XYZ: u32 = X | Y | Z;
pub const XYZW: u32 = X | Y | Z | W;

/// Índice de un componente, para las instrucciones que difunden uno solo.
pub const BX: u32 = 0;
pub const BY: u32 = 1;
pub const BZ: u32 = 2;
pub const BW: u32 = 3;

/// Instrucción superior que no hace nada.
pub const NOP: u32 = 0x0000_02FF;
/// Instrucción inferior que no hace nada (`move vf0, vf0`).
pub const LNOP: u32 = 0x8000_033C;
/// Bit E de la instrucción superior: el microprograma termina tras la
/// instrucción siguiente.
pub const E_BIT: u32 = 0x4000_0000;

const fn upper(dest: u32, ft: u32, fs: u32, fd: u32, op: u32) -> u32 {
    dest << 21 | ft << 16 | fs << 11 | fd << 6 | op
}

/// Instrucciones superiores del segundo grupo (el código va repartido).
const fn upper2(dest: u32, ft: u32, fs: u32, index: u32) -> u32 {
    dest << 21 | ft << 16 | fs << 11 | (index >> 2) << 6 | 0x3C | (index & 3)
}

// ---------------------------------------------------------------- superiores

/// `fd = fs + ft`
pub const fn add(dest: u32, fd: u32, fs: u32, ft: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x28)
}
/// `fd = fs - ft`
pub const fn sub(dest: u32, fd: u32, fs: u32, ft: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x2C)
}
/// `fd = fs * ft`
pub const fn mul(dest: u32, fd: u32, fs: u32, ft: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x2A)
}
/// `fd = ACC + fs * ft`
pub const fn madd(dest: u32, fd: u32, fs: u32, ft: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x29)
}
/// `fd = max(fs, ft)`
pub const fn max(dest: u32, fd: u32, fs: u32, ft: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x2B)
}
/// `fd = min(fs, ft)`
pub const fn mini(dest: u32, fd: u32, fs: u32, ft: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x2F)
}
/// `fd = fs + ft.bc`
pub const fn addbc(dest: u32, fd: u32, fs: u32, ft: u32, bc: u32) -> u32 {
    upper(dest, ft, fs, fd, bc)
}
/// `fd = fs - ft.bc`
pub const fn subbc(dest: u32, fd: u32, fs: u32, ft: u32, bc: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x04 + bc)
}
/// `fd = ACC + fs * ft.bc`
pub const fn maddbc(dest: u32, fd: u32, fs: u32, ft: u32, bc: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x08 + bc)
}
/// `fd = max(fs, ft.bc)`
pub const fn maxbc(dest: u32, fd: u32, fs: u32, ft: u32, bc: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x10 + bc)
}
/// `fd = min(fs, ft.bc)`
pub const fn minibc(dest: u32, fd: u32, fs: u32, ft: u32, bc: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x14 + bc)
}
/// `fd = fs * ft.bc`
pub const fn mulbc(dest: u32, fd: u32, fs: u32, ft: u32, bc: u32) -> u32 {
    upper(dest, ft, fs, fd, 0x18 + bc)
}
/// `fd = fs * Q`
pub const fn mulq(dest: u32, fd: u32, fs: u32) -> u32 {
    upper(dest, 0, fs, fd, 0x1C)
}
/// `fd = fs + Q`
pub const fn addq(dest: u32, fd: u32, fs: u32) -> u32 {
    upper(dest, 0, fs, fd, 0x20)
}
/// `fd = ACC + fs * Q`
pub const fn maddq(dest: u32, fd: u32, fs: u32) -> u32 {
    upper(dest, 0, fs, fd, 0x21)
}
/// `fd.xyz = ACC - fs × ft` (segunda mitad del producto vectorial).
pub const fn opmsub(fd: u32, fs: u32, ft: u32) -> u32 {
    upper(XYZ, ft, fs, fd, 0x2E)
}
/// `ACC = fs + ft.bc`
pub const fn addabc(dest: u32, fs: u32, ft: u32, bc: u32) -> u32 {
    upper2(dest, ft, fs, bc)
}
/// `ACC = ACC + fs * ft.bc`
pub const fn maddabc(dest: u32, fs: u32, ft: u32, bc: u32) -> u32 {
    upper2(dest, ft, fs, 0x08 + bc)
}
/// `ACC = fs * ft.bc`
pub const fn mulabc(dest: u32, fs: u32, ft: u32, bc: u32) -> u32 {
    upper2(dest, ft, fs, 0x18 + bc)
}
/// `ft = |fs|`
pub const fn abs(dest: u32, ft: u32, fs: u32) -> u32 {
    upper2(dest, ft, fs, 0x1D)
}
/// `ft = entero(fs)`, truncando.
pub const fn ftoi0(dest: u32, ft: u32, fs: u32) -> u32 {
    upper2(dest, ft, fs, 0x14)
}
/// `ft = entero(fs * 16)`, truncando.
pub const fn ftoi4(dest: u32, ft: u32, fs: u32) -> u32 {
    upper2(dest, ft, fs, 0x15)
}
/// Compara `fs.xyz` con `±|ft.w|` y mete los seis resultados en el registro
/// de recorte (que guarda los de las cuatro últimas comparaciones).
pub const fn clipw(fs: u32, ft: u32) -> u32 {
    upper2(XYZ, ft, fs, 0x1F)
}
/// `ACC.xyz = fs × ft` (primera mitad del producto vectorial).
pub const fn opmula(fs: u32, ft: u32) -> u32 {
    upper2(XYZ, ft, fs, 0x2E)
}

// ---------------------------------------------------------------- inferiores

const fn lower(op: u32, dest: u32, it: u32, is: u32, imm11: i32) -> u32 {
    op << 25 | dest << 21 | it << 16 | is << 11 | (imm11 as u32 & 0x7FF)
}

const fn lower2(dest: u32, it: u32, is: u32, index: u32) -> u32 {
    0x8000_0000 | dest << 21 | it << 16 | is << 11 | (index >> 2) << 6 | 0x3C | (index & 3)
}

/// `ft = memoria[is + offset]` (direcciones en palabras de 128 bits).
pub const fn lq(dest: u32, ft: u32, offset: i32, is: u32) -> u32 {
    lower(0x00, dest, ft, is, offset)
}
/// `memoria[it + offset] = fs`
pub const fn sq(dest: u32, fs: u32, offset: i32, it: u32) -> u32 {
    lower(0x01, dest, it, fs, offset)
}
/// `it = memoria[is + offset].componente` (los 16 bits bajos).
pub const fn ilw(dest: u32, it: u32, offset: i32, is: u32) -> u32 {
    lower(0x04, dest, it, is, offset)
}
/// `memoria[is + offset].componente = it`
pub const fn isw(dest: u32, it: u32, offset: i32, is: u32) -> u32 {
    lower(0x05, dest, it, is, offset)
}
/// `it = is + imm` (hasta 0x7FFF).
pub const fn iaddiu(it: u32, is: u32, imm: u32) -> u32 {
    0x08 << 25 | ((imm >> 11) & 0xF) << 21 | it << 16 | is << 11 | (imm & 0x7FF)
}
/// `it = is - imm`
pub const fn isubiu(it: u32, is: u32, imm: u32) -> u32 {
    0x09 << 25 | ((imm >> 11) & 0xF) << 21 | it << 16 | is << 11 | (imm & 0x7FF)
}
/// `VI01 = (registro de recorte & imm) != 0`
pub const fn fcand(imm: u32) -> u32 {
    0x12 << 25 | (imm & 0xFF_FFFF)
}
/// `id = is + it`
pub const fn iadd(id: u32, is: u32, it: u32) -> u32 {
    0x8000_0000 | it << 16 | is << 11 | id << 6 | 0x30
}
/// `id = is - it`
pub const fn isub(id: u32, is: u32, it: u32) -> u32 {
    0x8000_0000 | it << 16 | is << 11 | id << 6 | 0x31
}
/// `id = is | it`
pub const fn ior(id: u32, is: u32, it: u32) -> u32 {
    0x8000_0000 | it << 16 | is << 11 | id << 6 | 0x35
}
/// `Q = fs.fsf / ft.ftf` (el resultado tarda 7 ciclos: ver [`WAITQ`]).
pub const fn div(fs: u32, fsf: u32, ft: u32, ftf: u32) -> u32 {
    lower2(ftf << 2 | fsf, ft, fs, 0x38)
}
/// `Q = fs.fsf / sqrt(ft.ftf)` (tarda 13 ciclos).
pub const fn rsqrt(fs: u32, fsf: u32, ft: u32, ftf: u32) -> u32 {
    lower2(ftf << 2 | fsf, ft, fs, 0x3A)
}
/// Espera a que la división o la raíz en curso dejen su resultado en Q.
pub const WAITQ: u32 = lower2(0, 0, 0, 0x3B);
/// `it = fs.fsf` (los 16 bits bajos).
pub const fn mtir(it: u32, fs: u32, fsf: u32) -> u32 {
    lower2(fsf, it, fs, 0x3C)
}
/// Envía al GS el paquete que empieza en `memoria[is]`.
pub const fn xgkick(is: u32) -> u32 {
    lower2(0, 0, is, 0x6C)
}
/// Salta al microprograma de dirección `is` (en instrucciones).
pub const fn jr(is: u32) -> u32 {
    0x24 << 25 | is << 11
}

/// Saltos, cuyo destino resuelve [`Asm`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch {
    /// Siempre.
    Always,
    /// Guardando en el registro la dirección de vuelta (llamada).
    Link(u32),
    /// Si `it == is`.
    Equal(u32, u32),
    /// Si `it != is`.
    NotEqual(u32, u32),
    /// Si `is < 0`.
    Negative(u32),
}

impl Branch {
    const fn encode(self, offset: i32) -> u32 {
        match self {
            Branch::Always => lower(0x20, 0, 0, 0, offset),
            Branch::Link(it) => lower(0x21, 0, it, 0, offset),
            Branch::Equal(it, is) => lower(0x28, 0, it, is, offset),
            Branch::NotEqual(it, is) => lower(0x29, 0, it, is, offset),
            Branch::Negative(is) => lower(0x2C, 0, 0, is, offset),
        }
    }
}

/// Una etiqueta: un punto del microprograma al que se puede saltar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label(usize);

/// Va montando un microprograma.
#[derive(Default)]
pub struct Asm {
    /// (inferior, superior) de cada posición.
    code: alloc::vec::Vec<(u32, u32)>,
    labels: alloc::vec::Vec<Option<usize>>,
    /// Saltos por resolver: (posición, salto, etiqueta).
    fixups: alloc::vec::Vec<(usize, Branch, Label)>,
}

impl Asm {
    pub fn new() -> Asm {
        Asm::default()
    }

    /// Una instrucción superior (la inferior no hace nada).
    pub fn u(&mut self, upper: u32) {
        self.code.push((LNOP, upper));
    }

    /// Una instrucción inferior.
    pub fn l(&mut self, lower: u32) {
        self.code.push((lower, NOP));
    }

    /// Una posición vacía.
    pub fn nop(&mut self) {
        self.code.push((LNOP, NOP));
    }

    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() - 1)
    }

    /// Coloca la etiqueta en la posición actual.
    pub fn place(&mut self, label: Label) {
        assert!(self.labels[label.0].is_none(), "etiqueta colocada dos veces");
        self.labels[label.0] = Some(self.code.len());
    }

    /// Un salto y su hueco de retardo vacío. El salto lee los registros
    /// enteros con un ciclo de retraso, así que delante se deja una posición
    /// vacía cuando `settle` es cierto (el registro se acaba de escribir).
    pub fn branch(&mut self, branch: Branch, target: Label, settle: bool) {
        if settle {
            self.nop();
        }
        self.fixups.push((self.code.len(), branch, target));
        self.code.push((0, NOP));
        self.nop();
    }

    /// Llama a una subrutina, que vuelve con `jr link`.
    pub fn call(&mut self, link: u32, target: Label) {
        self.branch(Branch::Link(link), target, false);
    }

    /// Vuelve de una subrutina.
    pub fn ret(&mut self, link: u32) {
        self.l(jr(link));
        self.nop();
    }

    /// Fin del microprograma.
    pub fn end(&mut self) {
        self.code.push((LNOP, NOP | E_BIT));
        self.nop();
    }

    /// El microprograma terminado: una palabra de 64 bits por posición, con
    /// la instrucción inferior en los 32 bits bajos.
    pub fn finish(mut self) -> alloc::vec::Vec<u64> {
        for (at, branch, label) in core::mem::take(&mut self.fixups) {
            let target = self.labels[label.0].expect("etiqueta sin colocar") as i32;
            // Relativo a la instrucción que sigue al salto.
            let offset = target - (at as i32 + 1);
            assert!((-1024..1024).contains(&offset), "salto demasiado largo");
            self.code[at].0 = branch.encode(offset);
        }
        self.code.iter().map(|&(lower, upper)| lower as u64 | (upper as u64) << 32).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_encodings() {
        // Valores conocidos de las instrucciones equivalentes del COP2.
        assert_eq!(clipw(0, 0) & 0x7FF, 0x1FF);
        assert_eq!(opmula(0, 0) & 0x7FF, 0x2FE);
        assert_eq!(div(0, 0, 0, 0) & 0x7FF, 0x3BC);
        assert_eq!(rsqrt(0, 0, 0, 0) & 0x7FF, 0x3BE);
        assert_eq!(WAITQ, 0x8000_03BF);
        assert_eq!(mtir(0, 0, 0) & 0x7FF, 0x3FC);
        assert_eq!(xgkick(0) & 0x7FF, 0x6FC);
        assert_eq!(ftoi4(0, 0, 0) & 0x7FF, 0x17D);
        assert_eq!(abs(0, 0, 0) & 0x7FF, 0x1FD);
        // add.xyzw vf3, vf1, vf2
        assert_eq!(add(XYZW, 3, 1, 2), 0x01E2_08E8);
        // lq.xyzw vf5, 7(vi2) y iaddiu vi4, vi0, 0x1234
        assert_eq!(lq(XYZW, 5, 7, 2), 0x01E5_1007);
        assert_eq!(iaddiu(4, 0, 0x1234), 0x1044_0234);
    }

    #[test]
    fn branches_are_relative_to_the_next_instruction() {
        let mut a = Asm::new();
        let top = a.label();
        let skip = a.label();
        a.place(top);
        a.branch(Branch::Equal(1, 0), skip, false); // 0, 1
        a.nop(); // 2
        a.place(skip);
        a.branch(Branch::Always, top, false); // 3, 4
        let code = a.finish();
        assert_eq!(code[0] as u32, 0x28 << 25 | 1 << 16 | 2);
        assert_eq!(code[3] as u32, 0x20 << 25 | (-4i32 as u32 & 0x7FF));
        assert_eq!((code[1] >> 32) as u32, NOP);
    }
}

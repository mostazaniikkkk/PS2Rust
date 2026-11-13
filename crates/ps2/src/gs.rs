//! Graphics Synthesizer: modo de vídeo, doble búfer y primitivas 2D.
//!
//! Las primitivas se acumulan en un paquete GIF (modo PACKED, registros A+D)
//! que se envía al GS por el canal 2 de DMA.

use crate::font;
use crate::kernel::{self, read32, read64, write32, write64};
use core::cell::UnsafeCell;

// Registros privilegiados del GS (64 bits).
const GS_PMODE: u32 = 0x1200_0000;
const GS_DISPFB2: u32 = 0x1200_0090;
const GS_DISPLAY2: u32 = 0x1200_00A0;
const GS_BGCOLOR: u32 = 0x1200_00E0;
const GS_CSR: u32 = 0x1200_1000;

// Canal 2 de DMA (GIF) y control del GIF.
const D2_CHCR: u32 = 0x1000_A000;
const D2_MADR: u32 = 0x1000_A010;
const D2_QWC: u32 = 0x1000_A020;
const D_CTRL: u32 = 0x1000_E000;
const GIF_CTRL: u32 = 0x1000_3000;

// Registros generales del GS (direcciones A+D).
const REG_PRIM: u64 = 0x00;
const REG_RGBAQ: u64 = 0x01;
const REG_XYZ2: u64 = 0x05;
const REG_XYOFFSET_1: u64 = 0x18;
const REG_PRMODECONT: u64 = 0x1A;
const REG_SCISSOR_1: u64 = 0x40;
const REG_ALPHA_1: u64 = 0x42;
const REG_DTHE: u64 = 0x45;
const REG_COLCLAMP: u64 = 0x46;
const REG_TEST_1: u64 = 0x47;
const REG_PABE: u64 = 0x49;
const REG_FBA_1: u64 = 0x4A;
const REG_FRAME_1: u64 = 0x4C;
const REG_ZBUF_1: u64 = 0x4E;
const REG_FINISH: u64 = 0x61;

const PRIM_LINE: u64 = 1;
const PRIM_TRIANGLE: u64 = 3;
const PRIM_SPRITE: u64 = 6;
const PRIM_GOURAUD: u64 = 1 << 3;
const PRIM_BLEND: u64 = 1 << 6;

/// Origen de la ventana de dibujo dentro del espacio de 4096×4096 del GS;
/// deja margen para que las coordenadas negativas se recorten bien.
const ORIGIN: i32 = 1024;

/// Capacidad del paquete GIF, en palabras de 128 bits.
const PACKET_QWORDS: usize = 16384;

#[repr(C, align(64))]
struct PacketBuffer(UnsafeCell<[u64; PACKET_QWORDS * 2]>);

// Sólo lo toca la única instancia de `Gs`.
unsafe impl Sync for PacketBuffer {}

static PACKET: PacketBuffer = PacketBuffer(UnsafeCell::new([0; PACKET_QWORDS * 2]));

/// Color RGBA. Para la mezcla alfa, `a = 0x80` es opaco.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 0x80 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }

    fn bits(self) -> u64 {
        self.r as u64 | (self.g as u64) << 8 | (self.b as u64) << 16 | (self.a as u64) << 24
    }
}

/// Estándar de televisión.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoMode {
    /// 640×448 entrelazado, 60 Hz.
    Ntsc,
    /// 640×512 entrelazado, 50 Hz.
    Pal,
}

/// Vértice 2D con color, para primitivas con sombreado Gouraud.
#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub color: Color,
}

impl Vertex {
    pub const fn new(x: f32, y: f32, color: Color) -> Vertex {
        Vertex { x, y, color }
    }
}

/// Cambia el color de fondo del CRTC (visible fuera del área de imagen).
pub fn set_background(color: Color) {
    unsafe { write64(GS_BGCOLOR, color.bits() & 0x00ff_ffff) };
}

/// Contexto de dibujo. Créalo una sola vez con [`Gs::init`].
pub struct Gs {
    width: i32,
    height: i32,
    /// Punteros de página de los dos framebuffers (en unidades de 8192 bytes).
    fbp: [u32; 2],
    /// Framebuffer en el que se está dibujando.
    draw: usize,
    /// Registros A+D acumulados en el paquete actual.
    len: usize,
    frames: u32,
    blend: bool,
    /// Paquete GIF, visto a través del espejo sin caché de la RAM.
    buf: *mut u64,
}

impl Gs {
    /// Inicializa el GS, enciende la salida de vídeo y deja ambos búferes en negro.
    pub fn init(mode: VideoMode) -> Gs {
        let (width, height, crt_mode, dx, dy) = match mode {
            VideoMode::Ntsc => (640i32, 448i32, 2u32, 652u64, 50u64),
            VideoMode::Pal => (640i32, 512i32, 3u32, 680u64, 72u64),
        };
        // Páginas de 8192 bytes; cada framebuffer de 32 bits ocupa w*h*4 bytes.
        let pages = ((width * height * 4 + 8191) / 8192) as u32;
        let mut gs = Gs {
            width,
            height,
            fbp: [0, pages],
            draw: 0,
            len: 0,
            frames: 0,
            blend: false,
            // 0x2000_0000: espejo sin caché, para que el DMA vea lo que escribimos.
            buf: (PACKET.0.get() as usize | 0x2000_0000) as *mut u64,
        };
        unsafe {
            write32(D_CTRL, read32(D_CTRL) | 1); // habilita el DMA
            write32(GIF_CTRL, 1); // reinicia el GIF
            write64(GS_CSR, 0x200); // reinicia el GS
            kernel::gs_put_imr(0xff00); // sin interrupciones del GS
            kernel::set_gs_crt(true, crt_mode, false);

            // Circuito de lectura 2 encendido.
            write64(GS_PMODE, 0x8046);
            gs.show(1);
            // DX, DY | MAGH = 3 (2560/640 - 1) | DW = 2559, DH = alto - 1.
            let display = dx | dy << 12 | 3 << 23 | 2559 << 32 | ((height - 1) as u64) << 44;
            write64(GS_DISPLAY2, display);
            write64(GS_BGCOLOR, 0);
        }

        gs.push(REG_PRMODECONT, 1);
        gs.push(REG_XYOFFSET_1, (ORIGIN as u64) << 4 | ((ORIGIN as u64) << 4) << 32);
        gs.push(
            REG_SCISSOR_1,
            ((width - 1) as u64) << 16 | ((height - 1) as u64) << 48,
        );
        // Sin búfer Z: pruebas de profundidad siempre pasan y no se escribe Z.
        gs.push(REG_ZBUF_1, (2 * pages) as u64 | 1 << 32);
        gs.push(REG_TEST_1, 1 << 16 | 1 << 17);
        gs.push(REG_COLCLAMP, 1);
        gs.push(REG_DTHE, 0);
        gs.push(REG_PABE, 0);
        gs.push(REG_FBA_1, 0);
        // Mezcla alfa estándar: Cs*As + Cd*(1-As).
        gs.push(REG_ALPHA_1, 0x44);

        // Limpia los dos framebuffers.
        for target in [1usize, 0] {
            gs.draw = target;
            gs.select_frame();
            gs.clear(Color::BLACK);
        }
        gs.flush();
        gs
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// Frames presentados hasta ahora.
    pub fn frame_count(&self) -> u32 {
        self.frames
    }

    /// Activa o desactiva la mezcla alfa para las primitivas siguientes.
    pub fn set_blend(&mut self, on: bool) {
        self.blend = on;
    }

    /// Rellena todo el framebuffer.
    pub fn clear(&mut self, color: Color) {
        let (w, h) = (self.width, self.height);
        let blend = core::mem::replace(&mut self.blend, false);
        self.rect(0, 0, w, h, color);
        self.blend = blend;
    }

    /// Rectángulo relleno de color plano.
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color) {
        self.prim(PRIM_SPRITE);
        self.push(REG_RGBAQ, color.bits());
        self.xy_int(x, y);
        self.xy_int(x + w, y + h);
    }

    /// Línea de un píxel de ancho.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color) {
        self.prim(PRIM_LINE);
        self.push(REG_RGBAQ, color.bits());
        self.xy(x0, y0);
        self.xy(x1, y1);
    }

    /// Triángulo con un color por vértice (sombreado Gouraud).
    pub fn triangle(&mut self, a: Vertex, b: Vertex, c: Vertex) {
        self.prim(PRIM_TRIANGLE | PRIM_GOURAUD);
        for v in [a, b, c] {
            self.push(REG_RGBAQ, v.color.bits());
            self.xy(v.x, v.y);
        }
    }

    /// Cuadrilátero convexo (vértices en orden alrededor del perímetro).
    pub fn quad(&mut self, a: Vertex, b: Vertex, c: Vertex, d: Vertex) {
        self.triangle(a, b, c);
        self.triangle(a, c, d);
    }

    /// Dibuja texto con la fuente de 5×7 escalada `scale` veces. `\n` baja de línea.
    pub fn text(&mut self, x: i32, y: i32, scale: i32, color: Color, text: &str) {
        self.prim(PRIM_SPRITE);
        self.push(REG_RGBAQ, color.bits());
        let mut cx = x;
        let mut cy = y;
        for ch in text.chars() {
            if ch == '\n' {
                cx = x;
                cy += (font::GLYPH_H as i32 + 2) * scale;
                continue;
            }
            let rows = font::glyph(ch);
            for (row, bits) in rows.iter().enumerate() {
                // Une los píxeles consecutivos de la fila en un solo sprite.
                let mut col = 0;
                while col < font::GLYPH_W {
                    if bits & (0x10 >> col) == 0 {
                        col += 1;
                        continue;
                    }
                    let run_start = col;
                    while col < font::GLYPH_W && bits & (0x10 >> col) != 0 {
                        col += 1;
                    }
                    let y0 = cy + row as i32 * scale;
                    self.xy_int(cx + run_start as i32 * scale, y0);
                    self.xy_int(cx + col as i32 * scale, y0 + scale);
                }
            }
            cx += font::ADVANCE as i32 * scale;
        }
    }

    /// Ancho en píxeles de una línea de texto.
    pub fn text_width(text: &str, scale: i32) -> i32 {
        text.chars().count() as i32 * font::ADVANCE as i32 * scale
    }

    /// Envía lo dibujado, espera al retrazo vertical y muestra el frame.
    pub fn present(&mut self) {
        unsafe { write64(GS_CSR, 2) }; // limpia la señal FINISH anterior
        self.push(REG_FINISH, 0);
        self.flush();
        unsafe {
            // Espera a que el GS termine de dibujar…
            let mut spins = 0u32;
            while read64(GS_CSR) & 2 == 0 && spins < 4_000_000 {
                spins += 1;
            }
            // …y al siguiente retrazo vertical.
            write64(GS_CSR, 8);
            while read64(GS_CSR) & 8 == 0 {}
            self.show(self.draw);
        }
        self.draw ^= 1;
        self.select_frame();
        self.frames = self.frames.wrapping_add(1);
        crate::time::ticks();
    }

    /// Envía al GS las primitivas acumuladas sin cambiar de frame.
    pub fn flush(&mut self) {
        if self.len == 0 {
            return;
        }
        unsafe {
            // GIFtag: NLOOP = len, EOP = 1, modo PACKED, 1 registro (A+D).
            self.buf.write_volatile(self.len as u64 | 1 << 15 | 1 << 60);
            self.buf.add(1).write_volatile(0xE);
            while read32(D2_CHCR) & 0x100 != 0 {}
            write32(D2_MADR, PACKET.0.get() as u32 & 0x1fff_ffff);
            write32(D2_QWC, self.len as u32 + 1);
            write32(D2_CHCR, 0x101); // de memoria al GIF, modo normal, arrancar
            while read32(D2_CHCR) & 0x100 != 0 {}
        }
        self.len = 0;
    }

    /// Elige qué framebuffer lee el CRTC.
    unsafe fn show(&self, index: usize) {
        let fbw = (self.width / 64) as u64;
        write64(GS_DISPFB2, self.fbp[index] as u64 | fbw << 9);
    }

    /// Dirige el dibujo al framebuffer `self.draw`.
    fn select_frame(&mut self) {
        let fbw = (self.width / 64) as u64;
        self.push(REG_FRAME_1, self.fbp[self.draw] as u64 | fbw << 16);
    }

    fn prim(&mut self, kind: u64) {
        let blend = if self.blend { PRIM_BLEND } else { 0 };
        self.push(REG_PRIM, kind | blend);
    }

    fn xy_int(&mut self, x: i32, y: i32) {
        let fx = ((x + ORIGIN).clamp(0, 4095) as u64) << 4;
        let fy = ((y + ORIGIN).clamp(0, 4095) as u64) << 4;
        self.push(REG_XYZ2, fx | fy << 16);
    }

    fn xy(&mut self, x: f32, y: f32) {
        // Coma fija 12.4.
        let fx = (((x + ORIGIN as f32) * 16.0) as i32).clamp(0, 0xffff) as u64;
        let fy = (((y + ORIGIN as f32) * 16.0) as i32).clamp(0, 0xffff) as u64;
        self.push(REG_XYZ2, fx | fy << 16);
    }

    fn push(&mut self, reg: u64, value: u64) {
        unsafe {
            let slot = self.buf.add(2 * (self.len + 1));
            slot.write_volatile(value);
            slot.add(1).write_volatile(reg);
        }
        self.len += 1;
        if self.len == PACKET_QWORDS - 1 {
            self.flush();
        }
    }
}

// Lectura del GS hacia el EE: pasa por el VIF1 (canal 1 de DMA).
const GS_BUSDIR: u32 = 0x1200_1040;
const VIF1_STAT: u32 = 0x1000_3C00;
const D1_CHCR: u32 = 0x1000_9000;
const D1_MADR: u32 = 0x1000_9010;
const D1_QWC: u32 = 0x1000_9020;

const REG_BITBLTBUF: u64 = 0x50;
const REG_TRXPOS: u64 = 0x51;
const REG_TRXREG: u64 = 0x52;
const REG_TRXDIR: u64 = 0x53;

/// Acceso de bajo nivel, para construir otras bibliotecas gráficas encima
/// (por ejemplo `ps2-gles`). Los píxeles son siempre de 32 bits (R, G, B, A).
impl Gs {
    /// Escribe un registro general del GS (dirección A+D) en el paquete actual.
    pub fn register(&mut self, address: u8, value: u64) {
        self.push(address as u64, value);
    }

    /// Página de memoria (8 KiB) donde empieza el framebuffer en el que se
    /// dibuja ahora.
    pub fn draw_page(&self) -> u32 {
        self.fbp[self.draw]
    }

    /// Primera página libre detrás de los dos framebuffers. La memoria del GS
    /// tiene 512 páginas (4 MiB).
    pub fn first_free_page(&self) -> u32 {
        self.fbp[1] * 2
    }

    /// Copia píxeles a la memoria del GS: un rectángulo de `w`×`h` en
    /// (`x`, `y`) del búfer que empieza en el bloque `block` (256 bytes) y
    /// tiene `buffer_width` píxeles de ancho (múltiplo de 64).
    pub fn upload(&mut self, block: u32, buffer_width: u32, x: u32, y: u32, w: u32, h: u32, pixels: &[u8]) {
        assert!(pixels.len() >= (w * h * 4) as usize, "faltan píxeles");
        if w == 0 || h == 0 {
            return;
        }
        self.flush();
        let rows_per_chunk = (((PACKET_QWORDS - 8) * 16) as u32 / (w * 4)).max(1);
        let mut row = 0;
        while row < h {
            let rows = rows_per_chunk.min(h - row);
            let bytes = (rows * w * 4) as usize;
            let qwords = (bytes + 15) / 16;
            let start = (row * w * 4) as usize;
            unsafe {
                let b = self.buf;
                // Cuatro registros A+D que describen la transferencia…
                b.write_volatile(4 | 1 << 60);
                b.add(1).write_volatile(0xE);
                let regs = [
                    (REG_BITBLTBUF, (block as u64) << 32 | ((buffer_width / 64) as u64) << 48),
                    (REG_TRXPOS, (x as u64) << 32 | ((y + row) as u64) << 48),
                    (REG_TRXREG, w as u64 | (rows as u64) << 32),
                    (REG_TRXDIR, 0),
                ];
                for (i, (reg, value)) in regs.iter().enumerate() {
                    b.add(2 + 2 * i).write_volatile(*value);
                    b.add(3 + 2 * i).write_volatile(*reg);
                }
                // …y los datos, en modo IMAGE.
                b.add(10).write_volatile(qwords as u64 | 1 << 15 | 2 << 58);
                b.add(11).write_volatile(0);
                let data = b.add(12) as *mut u8;
                core::ptr::copy_nonoverlapping(pixels.as_ptr().add(start), data, bytes);
                core::ptr::write_bytes(data.add(bytes), 0, qwords * 16 - bytes);
                self.send(6 + qwords);
            }
            row += rows;
        }
    }

    /// Lee píxeles de la memoria del GS: el rectángulo de `w`×`h` en
    /// (`x`, `y`) del búfer que empieza en el bloque `block` y tiene
    /// `buffer_width` píxeles de ancho.
    pub fn download(&mut self, block: u32, buffer_width: u32, x: u32, y: u32, w: u32, h: u32, pixels: &mut [u8]) {
        assert!(pixels.len() >= (w * h * 4) as usize, "no caben los píxeles");
        if w == 0 || h == 0 {
            return;
        }
        self.flush();
        // El GS entrega palabras de 128 bits (4 píxeles): se lee un ancho
        // múltiplo de 4 que contenga lo pedido y se copian sus columnas.
        let read_w = (w + 3) & !3;
        let read_x = x.min(buffer_width.saturating_sub(read_w));
        let skip = ((x - read_x) * 4) as usize;
        let rows_per_chunk = (((PACKET_QWORDS - 8) * 16) as u32 / (read_w * 4)).max(1);
        let mut row = 0;
        while row < h {
            let rows = rows_per_chunk.min(h - row);
            let qwords = (rows * read_w / 4) as usize;
            unsafe {
                write64(GS_CSR, 2); // limpia FINISH
                self.push(REG_BITBLTBUF, block as u64 | ((buffer_width / 64) as u64) << 16);
                self.push(REG_TRXPOS, read_x as u64 | ((y + row) as u64) << 16);
                self.push(REG_TRXREG, read_w as u64 | (rows as u64) << 32);
                self.push(REG_FINISH, 0);
                self.push(REG_TRXDIR, 1);
                self.flush();
                let mut spins = 0u32;
                while read64(GS_CSR) & 2 == 0 && spins < 4_000_000 {
                    spins += 1;
                }
                // El GS pasa a enviar y el VIF1 recibe por DMA.
                write32(VIF1_STAT, 0x0080_0000);
                write64(GS_BUSDIR, 1);
                write32(D1_MADR, PACKET.0.get() as u32 & 0x1fff_ffff);
                write32(D1_QWC, qwords as u32);
                write32(D1_CHCR, 0x100);
                while read32(D1_CHCR) & 0x100 != 0 {}
                write32(VIF1_STAT, 0);
                write64(GS_BUSDIR, 0);
                write64(GS_CSR, 2);
                for r in 0..rows {
                    let from = (self.buf as *const u8).add((r * read_w * 4) as usize + skip);
                    let to = pixels.as_mut_ptr().add(((row + r) * w * 4) as usize);
                    core::ptr::copy_nonoverlapping(from, to, (w * 4) as usize);
                }
            }
            row += rows;
        }
    }

    /// Envía por DMA las primeras `qwords` palabras de 128 bits del paquete.
    unsafe fn send(&mut self, qwords: usize) {
        while read32(D2_CHCR) & 0x100 != 0 {}
        write32(D2_MADR, PACKET.0.get() as u32 & 0x1fff_ffff);
        write32(D2_QWC, qwords as u32);
        write32(D2_CHCR, 0x101);
        while read32(D2_CHCR) & 0x100 != 0 {}
    }
}

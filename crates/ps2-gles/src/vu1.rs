//! La unidad vectorial 1 (VU1) de la PlayStation 2, que hace la geometría de
//! los triángulos: se le envían por el VIF1 el microprograma, las constantes
//! de la llamada y los vértices en tandas, y ella manda el resultado al GS.
//!
//! Mientras la VU1 procesa una tanda, el Emotion Engine prepara la
//! siguiente. Al final de cada llamada de dibujo se espera a que termine,
//! para que el orden con lo que se dibuja por otros caminos sea el de GL.

use crate::raster::FastVertex;
use crate::vu1prog::{self, Key, BATCH, CONSTANTS, C_COUNT, INPUT};
use core::ptr::{addr_of, read_volatile, write_volatile};

const VIF1_STAT: u32 = 0x1000_3C00;
const VIF1_FBRST: u32 = 0x1000_3C10;
const VIF1_ERR: u32 = 0x1000_3C20;
const D1_CHCR: u32 = 0x1000_9000;
const D1_MADR: u32 = 0x1000_9010;
const D1_QWC: u32 = 0x1000_9020;

// Códigos del VIF.
const VIF_NOP: u32 = 0x0000_0000;
const VIF_STCYCL: u32 = 0x0100_0101;
/// Espera a que termine el microprograma en curso.
const VIF_FLUSHE: u32 = 0x1000_0000;
/// Espera al microprograma y a que el GS reciba todo lo que envió.
const VIF_FLUSH: u32 = 0x1100_0000;
const VIF_MSCAL: u32 = 0x1400_0000;
const VIF_MPG: u32 = 0x4A00_0000;
/// Copia a la memoria de la VU palabras de 128 bits tal cual.
const VIF_UNPACK_V4_32: u32 = 0x6C00_0000;

/// Palabras de 32 bits de cada paquete: cabe el microprograma entero (2048
/// instrucciones), las constantes y una tanda de vértices.
const PACKET_WORDS: usize = 6144;

#[repr(C, align(64))]
struct Packet([u32; PACKET_WORDS]);

static mut PACKETS: [Packet; 2] = [Packet([0; PACKET_WORDS]), Packet([0; PACKET_WORDS])];

unsafe fn read32(address: u32) -> u32 {
    read_volatile(address as *const u32)
}

unsafe fn write32(address: u32, value: u32) {
    write_volatile(address as *mut u32, value)
}

pub struct Vu1 {
    /// Microprograma cargado ahora en la VU1.
    program: Option<Key>,
    /// Paquete que toca rellenar.
    current: usize,
    /// Palabras escritas en él.
    length: usize,
}

impl Vu1 {
    pub fn new() -> Vu1 {
        unsafe {
            write32(VIF1_FBRST, 1);
            write32(VIF1_ERR, 0);
        }
        Vu1 { program: None, current: 0, length: 0 }
    }

    fn push(&mut self, word: u32) {
        // Por el espejo sin caché: el DMA lee de la RAM.
        unsafe {
            let base = (addr_of!(PACKETS[self.current].0) as usize | 0x2000_0000) as *mut u32;
            base.add(self.length).write_volatile(word);
        }
        self.length += 1;
    }

    /// Copia `words` a partir de la dirección `address` de la VU1.
    fn unpack(&mut self, address: usize, words: &[[u32; 4]]) {
        // Hasta 255 palabras de 128 bits por código.
        for (i, chunk) in words.chunks(255).enumerate() {
            self.push(VIF_UNPACK_V4_32 | (chunk.len() as u32) << 16 | (address + i * 255) as u32);
            for word in chunk {
                for part in word {
                    self.push(*part);
                }
            }
        }
    }

    /// Envía el paquete actual por DMA y pasa al otro.
    fn send(&mut self) {
        while self.length % 4 != 0 {
            self.push(VIF_NOP);
        }
        unsafe {
            // El DMA anterior (del otro paquete) ya puede haber terminado o no.
            while read32(D1_CHCR) & 0x100 != 0 {}
            write32(D1_MADR, addr_of!(PACKETS[self.current].0) as u32 & 0x1fff_ffff);
            write32(D1_QWC, (self.length / 4) as u32);
            // De memoria al VIF1, modo normal, arrancar.
            write32(D1_CHCR, 0x101);
        }
        self.current ^= 1;
        self.length = 0;
    }

    /// Espera a que la VU1 termine y el GS haya recibido todo.
    pub fn sync(&mut self) {
        unsafe {
            while read32(D1_CHCR) & 0x100 != 0 {}
            // VIF ocupado, microprograma en marcha o esperando al GS.
            while read32(VIF1_STAT) & 0xF != 0 {}
        }
    }

    /// Procesa `count` vértices (triángulos sueltos, que entrega `fill`) con
    /// el microprograma de `key` y las constantes dadas, y espera a que termine.
    pub fn draw(&mut self, key: &Key, constants: &[[u32; 4]; CONSTANTS], count: usize, fill: &mut dyn FnMut(&mut [FastVertex])) {
        self.push(VIF_STCYCL);
        if self.program != Some(*key) {
            let code = vu1prog::build(key);
            assert!(code.len() <= 2048, "el microprograma no cabe en la VU1");
            // Hasta 256 instrucciones por código, que debe quedar en una
            // palabra impar para que las instrucciones vayan alineadas a 64 bits.
            for (i, chunk) in code.chunks(256).enumerate() {
                if self.length % 2 == 0 {
                    self.push(VIF_NOP);
                }
                self.push(VIF_MPG | (chunk.len() as u32 & 0xFF) << 16 | (i * 256) as u32);
                for instruction in chunk {
                    self.push(*instruction as u32);
                    self.push((*instruction >> 32) as u32);
                }
            }
            self.program = Some(*key);
        }
        self.unpack(0, constants);
        let mut remaining = count;
        while remaining > 0 {
            let batch = remaining.min(BATCH * 3);
            remaining -= batch;
            // La VU1 debe haber acabado la tanda anterior antes de pisarla.
            self.push(VIF_FLUSHE);
            self.push(VIF_STCYCL);
            // Tres palabras de 128 bits por vértice; hasta 255 por código.
            // Los vértices se escriben directamente en el paquete.
            let mut done = 0;
            while done < batch {
                let chunk = (batch - done).min(84);
                self.push(VIF_UNPACK_V4_32 | (chunk as u32 * 3) << 16 | (INPUT + done * 3) as u32);
                unsafe {
                    let base = (addr_of!(PACKETS[self.current].0) as usize | 0x2000_0000) as *mut u32;
                    fill(core::slice::from_raw_parts_mut(base.add(self.length) as *mut FastVertex, chunk));
                }
                self.length += chunk * 12;
                done += chunk;
            }
            self.unpack(C_COUNT, &[vu1prog::batch_header(batch / 3)]);
            self.push(VIF_MSCAL);
            self.send();
        }
        self.push(VIF_FLUSH);
        self.send();
        self.sync();
    }
}

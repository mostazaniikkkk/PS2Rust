//! La unidad vectorial 1 (VU1) de la PlayStation 2, que hace la geometría de
//! los triángulos: se le envían por el VIF1 el microprograma, las constantes
//! de la llamada y los vértices en tandas, y ella manda el resultado al GS.
//!
//! Mientras la VU1 procesa una tanda, el Emotion Engine prepara la
//! siguiente. Al final de cada llamada de dibujo se espera a que termine,
//! para que el orden con lo que se dibuja por otros caminos sea el de GL.

use crate::vu1prog::{self, Key, CONSTANTS, C_COUNT, PALETTE};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::ptr::{addr_of, read_volatile, write_volatile};

const VIF1_STAT: u32 = 0x1000_3C00;
const VIF1_FBRST: u32 = 0x1000_3C10;
const VIF1_ERR: u32 = 0x1000_3C20;
const D1_CHCR: u32 = 0x1000_9000;
const D1_MADR: u32 = 0x1000_9010;
const D1_QWC: u32 = 0x1000_9020;
const D1_TADR: u32 = 0x1000_9030;

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
/// instrucciones), las constantes, la paleta de matrices y una tanda de
/// vértices.
const PACKET_WORDS: usize = 7168;

// Etiquetas del DMA en modo cadena (campo ID).
/// Los datos van a continuación de la etiqueta, y después otra etiqueta.
const DMA_CNT: u32 = 1 << 28;
/// Los datos están en la dirección indicada; la siguiente etiqueta, detrás
/// de ésta.
const DMA_REF: u32 = 3 << 28;
/// Como `CNT`, y ahí acaba la transferencia.
const DMA_END: u32 = 7 << 28;
/// Palabras de 128 bits que puede llevar una etiqueta.
const DMA_MAX: usize = 0xFFFF;

/// Una palabra de 128 bits, alineada como exige el DMA.
#[derive(Clone, Copy)]
#[repr(C, align(16))]
struct Quad([u32; 4]);

/// Geometría retenida: el trozo de paquete del VIF1 con todas sus tandas de
/// vértices, listo para reenviarlo cuantas veces haga falta.
struct Retained {
    /// Matrices por vértice con las que se armó.
    units: usize,
    /// Vértices que contiene.
    vertices: usize,
    data: Vec<Quad>,
}

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
    retained: BTreeMap<u32, Retained>,
    next_retained: u32,
}

impl Vu1 {
    pub fn new() -> Vu1 {
        unsafe {
            write32(VIF1_FBRST, 1);
            write32(VIF1_ERR, 0);
        }
        Vu1 { program: None, current: 0, length: 0, retained: BTreeMap::new(), next_retained: 1 }
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

    /// Código de VIF que copia `vertices` vértices de `words` palabras a
    /// partir del vértice `done` de la tanda.
    fn unpack_vertices(key: &Key, done: usize, vertices: usize) -> u32 {
        let words = key.vertex_words();
        VIF_UNPACK_V4_32 | ((vertices * words) as u32) << 16 | (key.input() + done * words) as u32
    }

    /// Microprograma (si hay que cambiarlo), constantes y paleta de una
    /// llamada de dibujo.
    fn begin(&mut self, key: &Key, constants: &[[u32; 4]; CONSTANTS], palette: Option<&[[u32; 4]]>) {
        self.push(VIF_STCYCL);
        if self.program != Some(*key) {
            let code = vu1prog::build(key);
            assert!(code.len() <= 2048, "el microprograma no cabe en la VU1");
            // El microprograma anterior debe haber acabado antes de pisarlo.
            self.push(VIF_FLUSHE);
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
        if let Some(palette) = palette {
            self.unpack(PALETTE, palette);
        }
    }

    /// Procesa `count` vértices (triángulos sueltos, que entrega `fill`) con
    /// el microprograma de `key`, las constantes y la paleta de matrices
    /// dadas, y espera a que termine.
    pub fn draw(&mut self, key: &Key, constants: &[[u32; 4]; CONSTANTS], palette: Option<&[[u32; 4]]>, count: usize, fill: &mut dyn FnMut(&mut [f32])) {
        self.begin(key, constants, palette);
        let floats = key.vertex_words() * 4;
        // Vértices por código de VIF: hasta 255 palabras de 128 bits.
        let most = 255 / key.vertex_words();
        let mut remaining = count;
        while remaining > 0 {
            let batch = remaining.min(key.batch() * 3);
            remaining -= batch;
            // La VU1 debe haber acabado la tanda anterior antes de pisarla.
            self.push(VIF_FLUSHE);
            self.push(VIF_STCYCL);
            // Los vértices se escriben directamente en el paquete.
            let mut done = 0;
            while done < batch {
                let chunk = (batch - done).min(most);
                self.push(Self::unpack_vertices(key, done, chunk));
                unsafe {
                    let base = (addr_of!(PACKETS[self.current].0) as usize | 0x2000_0000) as *mut u32;
                    fill(core::slice::from_raw_parts_mut(base.add(self.length) as *mut f32, chunk * floats));
                }
                self.length += chunk * floats;
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

    /// Arma, una sola vez, el paquete con las tandas de `count` vértices
    /// (con `units` matrices por vértice) y lo guarda. Devuelve su
    /// identificador.
    pub fn retain(&mut self, units: usize, count: usize, fill: &mut dyn FnMut(&mut [f32])) -> u32 {
        // Del microprograma sólo importa aquí el formato de los vértices.
        let key = Key { lighting: false, light_count: 0, positional: 0, normalize: false, fog: false, flat: false, lod: false, no_cull: false, skin: units as u8, const_color: false };
        let floats = key.vertex_words() * 4;
        let most = 255 / key.vertex_words();
        let mut words: Vec<u32> = Vec::new();
        let mut vertices: Vec<f32> = Vec::new();
        let mut remaining = count;
        while remaining > 0 {
            let batch = remaining.min(key.batch() * 3);
            remaining -= batch;
            words.push(VIF_FLUSHE);
            words.push(VIF_STCYCL);
            let mut done = 0;
            while done < batch {
                let chunk = (batch - done).min(most);
                words.push(Self::unpack_vertices(&key, done, chunk));
                vertices.clear();
                vertices.resize(chunk * floats, 0.0);
                fill(&mut vertices);
                words.extend(vertices.iter().map(|v| v.to_bits()));
                done += chunk;
            }
            words.push(VIF_UNPACK_V4_32 | 1 << 16 | C_COUNT as u32);
            words.extend(vu1prog::batch_header(batch / 3));
            words.push(VIF_MSCAL);
            while words.len() % 4 != 0 {
                words.push(VIF_NOP);
            }
        }
        let data: Vec<Quad> = words.chunks(4).map(|w| Quad([w[0], w[1], w[2], w[3]])).collect();
        // El DMA lee de la RAM: lo escrito tiene que salir de la caché.
        ps2::kernel::flush_cache(0);
        let id = self.next_retained;
        self.next_retained += 1;
        self.retained.insert(id, Retained { units, vertices: count, data });
        id
    }

    /// Vértices de una geometría retenida.
    pub fn retained_vertices(&self, id: u32) -> Option<usize> {
        self.retained.get(&id).map(|r| r.vertices)
    }

    pub fn release(&mut self, id: u32) {
        // Nada puede estar leyéndola: cada llamada de dibujo espera al final.
        self.retained.remove(&id);
    }

    /// Dibuja una geometría retenida: sólo viajan de nuevo el microprograma
    /// (si cambia), las constantes y la paleta; los vértices los lee el DMA
    /// de donde quedaron. Devuelve `false` si no existe o no es del formato
    /// de `key`.
    pub fn draw_retained(&mut self, key: &Key, constants: &[[u32; 4]; CONSTANTS], palette: Option<&[[u32; 4]]>, id: u32) -> bool {
        let (address, length) = match self.retained.get(&id) {
            Some(r) if r.units == key.skin as usize => (r.data.as_ptr() as u32 & 0x1fff_ffff, r.data.len()),
            _ => return false,
        };
        // Cadena del DMA: [CNT: preparación] [REF: vértices retenidos]…
        // [END: espera final].
        let start = self.length;
        debug_assert_eq!(start, 0);
        for _ in 0..4 {
            self.push(0);
        }
        self.begin(key, constants, palette);
        while self.length % 4 != 0 {
            self.push(VIF_NOP);
        }
        let quads = (self.length - start) / 4 - 1;
        self.patch(start, DMA_CNT | quads as u32);
        let mut sent = 0;
        while sent < length {
            let part = (length - sent).min(DMA_MAX);
            self.push(DMA_REF | part as u32);
            self.push(address + (sent * 16) as u32);
            self.push(0);
            self.push(0);
            sent += part;
        }
        self.push(DMA_END | 1);
        self.push(0);
        self.push(0);
        self.push(0);
        self.push(VIF_FLUSH);
        for _ in 0..3 {
            self.push(VIF_NOP);
        }
        unsafe {
            while read32(D1_CHCR) & 0x100 != 0 {}
            write32(D1_QWC, 0);
            write32(D1_TADR, addr_of!(PACKETS[self.current].0) as u32 & 0x1fff_ffff);
            // De memoria al VIF1, modo cadena (sin pasar las etiquetas al
            // VIF), arrancar.
            write32(D1_CHCR, 0x105);
        }
        self.current ^= 1;
        self.length = 0;
        self.sync();
        true
    }

    /// Reescribe la palabra `at` del paquete actual.
    fn patch(&mut self, at: usize, word: u32) {
        unsafe {
            let base = (addr_of!(PACKETS[self.current].0) as usize | 0x2000_0000) as *mut u32;
            base.add(at).write_volatile(word);
        }
    }
}

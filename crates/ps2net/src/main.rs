//! `ps2net.irx`: el driver del adaptador de red de la PlayStation 2, en Rust.
//!
//! Es la mitad del IOP de `std::net`. Enciende la interfaz de expansión
//! (DEV9), arranca el controlador Ethernet (SMAP: un MAC EMAC3 con dos colas
//! FIFO y dos anillos de descriptores) y a partir de ahí sólo mueve tramas:
//! la pila TCP/IP vive en el EE y habla con este módulo por RPC.
//!
//! No usa interrupciones: el EE pregunta (`EXCHANGE`) cada vez que mueve la
//! pila, entrega las tramas que quiere enviar y se lleva las recibidas. El
//! anillo de recepción guarda hasta 64 tramas (16 KiB) entre pregunta y
//! pregunta.
//!
//! Protocolo (servidor "PSNT"):
//!
//! * `INIT` (1): responde un `u32` de estado (0 = bien) y los 6 bytes de la
//!   dirección MAC.
//! * `EXCHANGE` (2): petición y respuesta son un `u32` con el número de
//!   tramas y, por cada una, un `u16` con su longitud y sus bytes.

#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use ps2_iop::{println, rpc, thread};

ps2_iop::module!(name: "ps2net", version: (1, 0), main: main, heap: 64 * 1024);

const SERVER_ID: u32 = 0x5053_4E54;
const FN_INIT: u32 = 1;
const FN_EXCHANGE: u32 = 2;
const BUFFER: usize = 16 * 1024;

// Bus de expansión e interfaz DEV9.
const SSBUS_1418: u32 = 0xBF80_1418;
const SSBUS_141C: u32 = 0xBF80_141C;
const SSBUS_1420: u32 = 0xBF80_1420;
const DEV9_1460: u32 = 0xBF80_1460;
const DEV9_1462: u32 = 0xBF80_1462;
const DEV9_1464: u32 = 0xBF80_1464;
const DEV9_1466: u32 = 0xBF80_1466;
const DEV9_POWER: u32 = 0xBF80_146C;
const DEV9_REV: u32 = 0xBF80_146E;

// Chip SPEED.
const SPD_REV_1: u32 = 0xB000_0002;
const SPD_CAPS: u32 = 0xB000_0004;
const SPD_INTR_MASK: u32 = 0xB000_002A;
const SPD_PIO_DIR: u32 = 0xB000_002C;
const SPD_PIO_DATA: u32 = 0xB000_002E;

// SMAP: colas y descriptores.
const SMAP_BD_MODE: u32 = 0xB000_0102;
const SMAP_INTR_CLR: u32 = 0xB000_0128;
const TXFIFO_CTRL: u32 = 0xB000_1000;
const TXFIFO_WR_PTR: u32 = 0xB000_1004;
const TXFIFO_FRAME_INC: u32 = 0xB000_1010;
const TXFIFO_DATA: u32 = 0xB000_1100;
const RXFIFO_CTRL: u32 = 0xB000_1030;
const RXFIFO_RD_PTR: u32 = 0xB000_1034;
const RXFIFO_FRAME_CNT: u32 = 0xB000_103C;
const RXFIFO_FRAME_DEC: u32 = 0xB000_1040;
const RXFIFO_DATA: u32 = 0xB000_1200;
const TX_BD: u32 = 0xB000_3000;
const RX_BD: u32 = 0xB000_3200;
const BD_COUNT: u32 = 64;
const BD_READY: u16 = 0x8000;
const BD_EMPTY: u16 = 0x8000;

// MAC EMAC3: registros de 32 bits partidos en dos mitades de 16.
const EMAC3: u32 = 0xB000_2000;
const MODE0: u32 = 0x00;
const MODE1: u32 = 0x04;
const TX_MODE0: u32 = 0x08;
const TX_MODE1: u32 = 0x0C;
const RX_MODE: u32 = 0x10;
const INTR_STAT: u32 = 0x14;
const INTR_ENABLE: u32 = 0x18;
const ADDR_HI: u32 = 0x1C;
const ADDR_LO: u32 = 0x20;
const PAUSE_TIMER: u32 = 0x2C;
const GROUP_HASH1: u32 = 0x40;
const INTER_FRAME_GAP: u32 = 0x58;
const STA_CTRL: u32 = 0x5C;
const TX_THRESHOLD: u32 = 0x60;
const RX_WATERMARK: u32 = 0x64;

// Códigos de error de INIT.
const ERR_NO_EXPANSION_BAY: u32 = 1;
const ERR_NO_DEVICE: u32 = 2;
const ERR_NO_ETHERNET: u32 = 3;
const ERR_OLD_CHIP: u32 = 4;
const ERR_BAD_EEPROM: u32 = 5;
const ERR_RESET: u32 = 6;

unsafe fn r8(addr: u32) -> u8 {
    read_volatile(addr as *const u8)
}

unsafe fn r16(addr: u32) -> u16 {
    read_volatile(addr as *const u16)
}

unsafe fn r32(addr: u32) -> u32 {
    read_volatile(addr as *const u32)
}

unsafe fn w8(addr: u32, value: u8) {
    write_volatile(addr as *mut u8, value)
}

unsafe fn w16(addr: u32, value: u16) {
    write_volatile(addr as *mut u16, value)
}

unsafe fn w32(addr: u32, value: u32) {
    write_volatile(addr as *mut u32, value)
}

/// Registro del EMAC3: la mitad alta está en la dirección más baja, y se
/// escribe primero (la baja es la que dispara las operaciones del PHY).
unsafe fn emac_write(register: u32, value: u32) {
    w16(EMAC3 + register, (value >> 16) as u16);
    w16(EMAC3 + register + 2, value as u16);
    let _ = emac_read(register);
}

unsafe fn emac_read(register: u32) -> u32 {
    (r16(EMAC3 + register) as u32) << 16 | r16(EMAC3 + register + 2) as u32
}

/// Espera a que un bit de reinicio se borre solo.
unsafe fn wait_clear(check: impl Fn() -> bool) -> bool {
    for _ in 0..100 {
        if !check() {
            return true;
        }
        thread::sleep_usec(1000);
    }
    false
}

/// Pausa de al menos un microsegundo entre flancos de la EEPROM.
fn tick() {
    for i in 0..64u32 {
        core::hint::black_box(i);
    }
}

/// Lee de la EEPROM de serie (por los pines del SPEED) las cuatro palabras
/// con la dirección MAC y su suma.
unsafe fn read_eeprom() -> Option<[u16; 4]> {
    const CS: u8 = 0x80;
    const CLOCK: u8 = 0x40;
    const DATA_IN: u8 = 0x20;
    const DATA_OUT: u8 = 0x10;
    let put = |value: u8| {
        w8(SPD_PIO_DATA, value);
        tick();
    };
    w8(SPD_PIO_DIR, 0xE1);
    put(CS);
    // Bit de inicio y orden de lectura (1, 1, 0) más seis bits de dirección 0.
    for _ in 0..2 {
        put(CS | DATA_IN);
        put(CS | DATA_IN | CLOCK);
    }
    for _ in 0..7 {
        put(CS);
        put(CS | CLOCK);
    }
    put(CS | CLOCK);
    // La EEPROM contesta primero con un bit a cero.
    let ok = r8(SPD_PIO_DATA) & DATA_OUT == 0;
    let mut words = [0u16; 4];
    if ok {
        put(CS);
        for word in &mut words {
            for bit in (0..16).rev() {
                put(CS | CLOCK);
                if r8(SPD_PIO_DATA) & DATA_OUT != 0 {
                    *word |= 1 << bit;
                }
                put(CS);
            }
        }
    }
    put(0);
    w8(SPD_PIO_DIR, 0x01);
    ok.then_some(words)
}

/// Lee un registro del PHY (dirección 1) por la interfaz MII del EMAC3.
unsafe fn phy_read(register: u32) -> u16 {
    emac_write(STA_CTRL, register | 0x1020);
    for _ in 0..100 {
        let value = emac_read(STA_CTRL);
        if value & 0x8000 != 0 {
            return (value >> 16) as u16;
        }
        thread::sleep_usec(1000);
    }
    0
}

unsafe fn phy_write(register: u32, value: u16) {
    emac_write(STA_CTRL, (value as u32) << 16 | 0x2020 | register);
    for _ in 0..100 {
        if emac_read(STA_CTRL) & 0x8000 != 0 {
            return;
        }
        thread::sleep_usec(1000);
    }
}

struct Adapter {
    mac: [u8; 6],
    /// Siguiente descriptor de envío y de recepción.
    tx_index: u32,
    rx_index: u32,
}

/// Enciende DEV9 y arranca el SMAP.
unsafe fn start() -> Result<Adapter, u32> {
    // Sólo la bahía de expansión de las consolas "fat" (no PCMCIA).
    if r16(DEV9_REV) & 0xF0 != 0x30 {
        return Err(ERR_NO_EXPANSION_BAY);
    }
    w32(SSBUS_1420, 0x0005_1011);
    w32(SSBUS_1418, 0xE01A_3043);
    w32(SSBUS_141C, 0xEF1A_3043);
    if r16(DEV9_POWER) & 0x04 == 0 {
        // Apagado: secuencia de encendido.
        w16(DEV9_1466, 1);
        w16(DEV9_1464, 0);
        w16(DEV9_1460, r16(DEV9_1464));
        if r16(DEV9_1462) & 1 != 0 {
            return Err(ERR_NO_DEVICE);
        }
        w16(DEV9_POWER, (r16(DEV9_POWER) & !1) | 0x04);
        thread::sleep_ms(500);
        w16(DEV9_1460, r16(DEV9_1460) | 1);
        w16(DEV9_POWER, r16(DEV9_POWER) | 1);
        thread::sleep_ms(500);
    }
    if r16(SPD_CAPS) & 0x01 == 0 {
        return Err(ERR_NO_ETHERNET);
    }
    if r16(SPD_REV_1) < 0x11 {
        return Err(ERR_OLD_CHIP);
    }
    w16(DEV9_1464, 0x03);
    // Sin interrupciones: el adaptador se consulta por sondeo.
    w16(SPD_INTR_MASK, 0);

    let words = read_eeprom().ok_or(ERR_BAD_EEPROM)?;
    let sum = words[0].wrapping_add(words[1]).wrapping_add(words[2]);
    if sum != words[3] || words[..3] == [0, 0, 0] {
        return Err(ERR_BAD_EEPROM);
    }
    let mac = [words[0] as u8, (words[0] >> 8) as u8, words[1] as u8, (words[1] >> 8) as u8, words[2] as u8, (words[2] >> 8) as u8];

    // Reinicio de las dos colas y del MAC.
    w8(TXFIFO_CTRL, 1);
    w8(RXFIFO_CTRL, 1);
    emac_write(MODE0, 0x2000_0000);
    if !wait_clear(|| r8(TXFIFO_CTRL) & 1 != 0) || !wait_clear(|| r8(RXFIFO_CTRL) & 1 != 0) || !wait_clear(|| emac_read(MODE0) & 0x2000_0000 != 0) {
        return Err(ERR_RESET);
    }
    w8(SMAP_BD_MODE, 0);
    for i in 0..BD_COUNT {
        for (base, control) in [(TX_BD, 0), (RX_BD, BD_EMPTY)] {
            let bd = base + i * 8;
            w16(bd, control);
            w16(bd + 2, 0);
            w16(bd + 4, 0);
            w16(bd + 6, 0);
        }
    }
    w16(SMAP_INTR_CLR, 0x7C);

    // Dúplex completo a 100 Mb, colas de 2K/1K, aceptar lo dirigido a
    // nuestra MAC más difusión y multidifusión, quitando relleno y CRC.
    emac_write(MODE1, 0x8164_8000);
    emac_write(TX_MODE1, 0x380F_0000);
    emac_write(RX_MODE, 0xC058_0000);
    emac_write(INTR_STAT, 0x0000_01C0);
    emac_write(INTR_ENABLE, 0x0000_01C0);
    emac_write(ADDR_HI, (mac[0] as u32) << 8 | mac[1] as u32);
    emac_write(ADDR_LO, (mac[2] as u32) << 24 | (mac[3] as u32) << 16 | (mac[4] as u32) << 8 | mac[5] as u32);
    emac_write(PAUSE_TIMER, 0xFFFF);
    for i in 0..4 {
        emac_write(GROUP_HASH1 + i * 4, 0);
    }
    emac_write(INTER_FRAME_GAP, 4);
    emac_write(TX_THRESHOLD, 0x6000_0000);
    emac_write(RX_WATERMARK, 0x0800_4000);

    // PHY: reinicio y negociación automática de velocidad y dúplex.
    phy_write(0, 0x8000);
    wait_clear(|| phy_read(0) & 0x8000 != 0);
    phy_write(0, 0);
    let advertise = (phy_read(4) & 0x1F) | 0x05E0;
    phy_write(4, advertise);
    phy_write(0, 0x1200);
    // Hasta 5 s para que haya enlace; sin él se sigue (se puede enchufar después).
    let mut link = false;
    for _ in 0..100 {
        let status = phy_read(1);
        if status & 0x34 == 0x24 {
            link = true;
            break;
        }
        thread::sleep_ms(50);
    }
    let common = phy_read(4) & phy_read(5);
    let full_duplex = common & 0x0100 != 0 || (common & 0x0080 == 0 && common & 0x0040 != 0);
    let mut mode1 = emac_read(MODE1) & 0x67FF_FFFF;
    if full_duplex || !link {
        mode1 |= 0x8000_0000;
        if common & 0x0400 != 0 {
            mode1 |= 0x1800_0000;
        }
    }
    emac_write(MODE1, mode1);
    // Transmisor y receptor en marcha.
    emac_write(MODE0, 0x1800_0000);
    thread::sleep_ms(10);
    println!(
        "ps2net: adaptador listo, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, enlace {}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        if link { "activo" } else { "sin detectar" }
    );
    Ok(Adapter { mac, tx_index: 0, rx_index: 0 })
}

impl Adapter {
    /// Envía una trama y espera a que el MAC la saque (hasta 20 ms).
    unsafe fn transmit(&mut self, frame: &[u8]) {
        if frame.len() < 14 || frame.len() > 1514 {
            return;
        }
        let pointer = r16(TXFIFO_WR_PTR).wrapping_add(0x1000);
        // La cola se escribe en palabras de 32 bits.
        for chunk in frame.chunks(4) {
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            w32(TXFIFO_DATA, u32::from_le_bytes(word));
        }
        let bd = TX_BD + (self.tx_index % BD_COUNT) * 8;
        w16(bd + 4, frame.len() as u16);
        w16(bd + 6, pointer);
        w8(TXFIFO_FRAME_INC, 0);
        // Listo para enviar; el MAC rellena hasta 60 bytes y añade el CRC.
        w16(bd, BD_READY | 0x0300);
        self.tx_index = self.tx_index.wrapping_add(1);
        emac_write(TX_MODE0, 0x8000_0000);
        for _ in 0..200 {
            if r16(bd) & BD_READY == 0 {
                break;
            }
            thread::sleep_usec(100);
        }
    }

    /// Copia la siguiente trama recibida a `out`. Devuelve su longitud;
    /// `None` si no hay más o no cabe (se queda para la próxima vez).
    unsafe fn receive(&mut self, out: &mut [u8]) -> Option<usize> {
        loop {
            if r8(RXFIFO_FRAME_CNT) == 0 {
                return None;
            }
            let bd = RX_BD + (self.rx_index % BD_COUNT) * 8;
            let status = r16(bd);
            if status & BD_EMPTY != 0 {
                return None;
            }
            let length = r16(bd + 4) as usize;
            let pointer = r16(bd + 6);
            let rounded = (length + 3) & !3;
            let good = status & 0x027F == 0 && (14..=1518).contains(&length);
            if good && length > out.len() {
                return None;
            }
            if good {
                w16(RXFIFO_RD_PTR, pointer);
                for chunk in out[..length].chunks_mut(4) {
                    let word = r32(RXFIFO_DATA).to_le_bytes();
                    chunk.copy_from_slice(&word[..chunk.len()]);
                }
            } else {
                // Trama con error: se salta.
                w16(RXFIFO_RD_PTR, pointer.wrapping_add(rounded as u16));
            }
            w8(RXFIFO_FRAME_DEC, 0);
            w16(bd, BD_EMPTY);
            self.rx_index = self.rx_index.wrapping_add(1);
            if good {
                return Some(length);
            }
        }
    }
}

fn main() {
    let mut adapter: Option<Adapter> = None;
    rpc::serve(SERVER_ID, BUFFER, move |function, data, received| unsafe {
        match function {
            FN_INIT => {
                if adapter.is_none() {
                    match start() {
                        Ok(a) => adapter = Some(a),
                        Err(code) => {
                            println!("ps2net: no se pudo arrancar el adaptador (error {})", code);
                            data[..4].copy_from_slice(&code.to_le_bytes());
                            return;
                        }
                    }
                }
                data[..4].copy_from_slice(&0u32.to_le_bytes());
                data[4..10].copy_from_slice(&adapter.as_ref().unwrap().mac);
            }
            FN_EXCHANGE => {
                let Some(adapter) = adapter.as_mut() else {
                    data[..4].copy_from_slice(&0u32.to_le_bytes());
                    return;
                };
                // Primero salen las tramas del EE…
                let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                let mut offset = 4;
                for _ in 0..count {
                    if offset + 2 > received {
                        break;
                    }
                    let length = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
                    if offset + 2 + length > received {
                        break;
                    }
                    adapter.transmit(&data[offset + 2..offset + 2 + length]);
                    offset += 2 + length;
                }
                // …y después se le devuelven las recibidas que quepan.
                let (mut count, mut offset) = (0u32, 4);
                while offset + 2 + 60 <= data.len() {
                    let Some(length) = adapter.receive(&mut data[offset + 2..]) else { break };
                    data[offset..offset + 2].copy_from_slice(&(length as u16).to_le_bytes());
                    offset += 2 + length;
                    count += 1;
                }
                data[..4].copy_from_slice(&count.to_le_bytes());
            }
            _ => data[..4].copy_from_slice(&u32::MAX.to_le_bytes()),
        }
    });
}

//! SIF: comandos y llamadas a procedimiento remoto (RPC) con el IOP.
//!
//! El IOP (el procesador de E/S, un R3000) atiende archivos, CD/DVD, tarjeta
//! de memoria, mandos y sonido. El EE le habla por el SIF: le envía paquetes
//! de comando por DMA (canal SIF1) y recibe los suyos en un búfer propio (canal
//! SIF0), que se procesan en una interrupción del DMAC.
//!
//! Encima va el protocolo RPC de la BIOS: `bind` localiza un servidor del IOP
//! por su id y `call` le envía argumentos y recibe la respuesta.
//!
//! Sólo hay una llamada RPC en vuelo a la vez (un semáforo la serializa); con
//! eso basta para la std y simplifica mucho el estado compartido con la
//! interrupción.

use super::kernel::{self, SifDma, uncached};
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

const CMD_CHANGE_SADDR: u32 = 0x8000_0000;
const CMD_SET_SREG: u32 = 0x8000_0001;
const CMD_INIT_CMD: u32 = 0x8000_0002;
const CMD_RESET: u32 = 0x8000_0003;
const CMD_RPC_END: u32 = 0x8000_0008;
const CMD_RPC_BIND: u32 = 0x8000_0009;
const CMD_RPC_CALL: u32 = 0x8000_000a;
const CMD_RPC_RDATA: u32 = 0x8000_000c;

const SYSREG_SUBADDR: u32 = 0x8000_0000;
const SYSREG_MAINADDR: u32 = 0x8000_0001;
const SYSREG_RPCINIT: u32 = 0x8000_0002;
const REG_SUBADDR: u32 = 2;
const REG_SMFLAG: u32 = 4;
const STAT_SIFINIT: u32 = 0x1_0000;
const STAT_CMDINIT: u32 = 0x2_0000;
const STAT_BOOTEND: u32 = 0x4_0000;

const DMA_INT_O: u32 = 0x4;
const DMA_ERT: u32 = 0x40;

const D_STAT: u32 = 0x1000_e010;
const D5_CHCR: u32 = 0x1000_c000;

#[repr(C, align(64))]
struct Packet([u32; 16]);

#[repr(C, align(64))]
struct RecvBuffer([u32; 32]);

/// Paquetes entrantes: el IOP escribe aquí por DMA.
static mut RECV: RecvBuffer = RecvBuffer([0; 32]);
/// Paquete saliente de la llamada en curso (bind/call).
static mut OUT: Packet = Packet([0; 16]);
/// Paquete del sistema (inicialización, cambio de dirección).
static mut SYS_OUT: Packet = Packet([0; 16]);
/// Paquete de reinicio del IOP (más largo que los demás).
static mut RESET_OUT: RecvBuffer = RecvBuffer([0; 32]);
/// Anillo de paquetes para responder a peticiones del IOP desde la interrupción.
static mut REPLY: [Packet; 8] = [const { Packet([0; 16]) }; 8];
static mut REPLY_NEXT: usize = 0;

static mut IOPBUF: u32 = 0;
static mut SREGS: [u32; 32] = [0; 32];
static mut RPC_ID: u32 = 1;
static mut READY: bool = false;
/// El manejador de interrupción y el semáforo se crean una sola vez,
/// aunque el IOP se reinicie.
static mut HANDLER_SET: bool = false;

/// Semáforo que serializa las llamadas RPC.
static mut LOCK_SEMA: i32 = -1;

/// Estado de un cliente RPC (vive en la RAM del EE; el IOP sólo nos devuelve
/// su dirección en el paquete de fin).
#[repr(C)]
pub struct Client {
    /// Dirección (en el IOP) de los datos del servidor; 0 si no está ligado.
    server: u32,
    /// Búfer de recepción del servidor en el IOP.
    buf: u32,
    done: u32,
    sema: i32,
}

impl Client {
    pub const fn new() -> Client {
        Client { server: 0, buf: 0, done: 0, sema: -1 }
    }

    pub fn is_bound(&self) -> bool {
        self.server != 0
    }
}

fn header(pkt: *mut u32, cid: u32, psize: u32, extra_size: u32, extra_dest: u32) {
    unsafe {
        write_volatile(pkt, (psize & 0xff) | (extra_size << 8));
        write_volatile(pkt.add(1), extra_dest);
        write_volatile(pkt.add(2), cid);
    }
}

/// Envía un paquete de comando (ya en memoria sin caché) y, opcionalmente,
/// datos extra que el DMA copia antes a `extra_dest` en el IOP.
fn send_cmd(cid: u32, pkt: *mut u32, psize: u32, extra: Option<(u32, u32, u32)>, from_interrupt: bool) -> bool {
    let (src, dest, size) = extra.unwrap_or((0, 0, 0));
    header(pkt, cid, psize, size, dest);
    kernel::sync();
    let iopbuf = unsafe { read_volatile(addr_of!(IOPBUF)) };
    let packet = SifDma { src: pkt as u32, dest: iopbuf, size: psize, attr: DMA_ERT | DMA_INT_O };
    let id = if size > 0 {
        let list = [SifDma { src, dest, size, attr: 0 }, packet];
        kernel::sif_set_dma(&list, from_interrupt)
    } else {
        kernel::sif_set_dma(core::slice::from_ref(&packet), from_interrupt)
    };
    id != 0
}

fn send_cmd_retry(cid: u32, pkt: *mut u32, psize: u32, extra: Option<(u32, u32, u32)>) {
    // La cola de DMA del SIF puede estar llena un instante.
    while !send_cmd(cid, pkt, psize, extra, false) {
        kernel::rotate_ready_queue(kernel::THREAD_PRIORITY);
    }
}

/// Manejador de la interrupción del canal SIF0 (paquetes del IOP).
extern "C" fn sif0_handler(_channel: i32, _arg: usize, _addr: usize) -> i32 {
    let recv = uncached(unsafe { addr_of!(RECV.0) } as *const u32);
    let size = unsafe { read_volatile(recv) } & 0xff;
    if size != 0 {
        let mut pkt = [0u32; 32];
        let words = (((size + 30) >> 4) * 4).min(32) as usize;
        for (i, w) in pkt.iter_mut().enumerate().take(words) {
            *w = unsafe { read_volatile(recv.add(i)) };
        }
        unsafe { write_volatile(recv, read_volatile(recv) & !0xff) };
        kernel::sif_set_dchain(true);
        dispatch(&pkt);
    }
    kernel::exit_handler();
    0
}

fn dispatch(pkt: &[u32; 32]) {
    match pkt[2] {
        CMD_CHANGE_SADDR => unsafe { write_volatile(addr_of_mut!(IOPBUF), pkt[4]) },
        CMD_SET_SREG => {
            let index = pkt[4] as usize;
            if index < 32 {
                unsafe { write_volatile(addr_of_mut!(SREGS[index]), pkt[5]) };
            }
        }
        CMD_RPC_END => {
            let cd = pkt[7] as *mut Client;
            if cd.is_null() {
                return;
            }
            unsafe {
                if pkt[8] == CMD_RPC_BIND {
                    write_volatile(addr_of_mut!((*cd).server), pkt[9]);
                    write_volatile(addr_of_mut!((*cd).buf), pkt[10]);
                }
                write_volatile(addr_of_mut!((*cd).done), 1);
                let sema = read_volatile(addr_of!((*cd).sema));
                if sema >= 0 {
                    kernel::i_signal_sema(sema);
                }
            }
        }
        CMD_RPC_RDATA => {
            // El IOP pide datos de la RAM del EE (por ejemplo, para escribir
            // un archivo): se los mandamos por DMA con el paquete de fin.
            unsafe {
                let slot = REPLY_NEXT % 8;
                REPLY_NEXT = REPLY_NEXT.wrapping_add(1);
                let rend = uncached(addr_of!(REPLY[slot].0) as *const u32);
                write_volatile(rend.add(5), pkt[5]); // pkt_addr
                write_volatile(rend.add(7), pkt[7]); // cd (recvbuf del IOP)
                write_volatile(rend.add(8), CMD_RPC_RDATA);
                let (src, dest, size) = (pkt[8], pkt[9], pkt[10]);
                send_cmd(CMD_RPC_END, rend, 64, Some((src, dest, size)), true);
            }
        }
        _ => {}
    }
}

/// Inicializa SIF CMD y SIF RPC. Idempotente.
pub fn init() {
    if unsafe { read_volatile(addr_of!(READY)) } {
        return;
    }
    kernel::flush_dcache();
    unsafe {
        if kernel::read32(D_STAT) & 0x20 != 0 {
            kernel::write32(D_STAT, 0x20);
        }
        if kernel::read32(D5_CHCR) & 0x100 == 0 {
            kernel::sif_set_dchain(false);
        }
    }
    if !unsafe { read_volatile(addr_of!(HANDLER_SET)) } {
        kernel::add_dmac_handler(kernel::DMAC_SIF0, sif0_handler);
        kernel::enable_dmac(kernel::DMAC_SIF0);
    }

    let recv_addr = uncached(unsafe { addr_of!(RECV.0) } as *const u32) as u32;
    let sys = uncached(unsafe { addr_of!(SYS_OUT.0) } as *const u32);
    let iopbuf = kernel::sif_get_reg(SYSREG_SUBADDR);
    if iopbuf != 0 {
        // El SIF CMD del IOP ya está en marcha: le damos nuestro búfer.
        unsafe {
            write_volatile(addr_of_mut!(IOPBUF), iopbuf);
            write_volatile(sys.add(4), recv_addr);
        }
        send_cmd_retry(CMD_CHANGE_SADDR, sys, 16 + 4, None);
    } else {
        while kernel::sif_get_reg(REG_SMFLAG) & STAT_CMDINIT == 0 {}
        let iopbuf = kernel::sif_get_reg(REG_SUBADDR);
        unsafe { write_volatile(addr_of_mut!(IOPBUF), iopbuf) };
        kernel::sif_set_reg(SYSREG_SUBADDR, iopbuf);
        kernel::sif_set_reg(SYSREG_MAINADDR, recv_addr);
        unsafe {
            write_volatile(sys.add(3), 0); // opt = 0: inicializar SIF CMD
            write_volatile(sys.add(4), recv_addr);
        }
        send_cmd_retry(CMD_INIT_CMD, sys, 16 + 4, None);
    }

    if kernel::sif_get_reg(SYSREG_RPCINIT) == 0 {
        unsafe { write_volatile(sys.add(3), 1) }; // opt = 1: inicializar RPC
        send_cmd_retry(CMD_INIT_CMD, sys, 16, None);
        while unsafe { read_volatile(addr_of!(SREGS[0])) } == 0 {
            kernel::rotate_ready_queue(kernel::THREAD_PRIORITY);
        }
        kernel::sif_set_reg(SYSREG_RPCINIT, 1);
    }
    unsafe {
        if !read_volatile(addr_of!(HANDLER_SET)) {
            LOCK_SEMA = kernel::create_sema(1, 1);
            write_volatile(addr_of_mut!(HANDLER_SET), true);
        }
        write_volatile(addr_of_mut!(READY), true);
    }
}

/// Reinicia el IOP con los módulos de la BIOS y vuelve a poner en marcha el
/// SIF. `arg` es la línea de arranque (normalmente
/// `rom0:UDNL rom0:EELOADCNF`). Los clientes RPC ligados antes dejan de valer.
pub fn reset_iop(arg: &[u8]) {
    init();
    let lock = unsafe { LOCK_SEMA };
    kernel::wait_sema(lock);
    let pkt = uncached(unsafe { addr_of!(RESET_OUT.0) } as *const u32);
    // Cabecera (16 bytes), longitud del argumento, modo y el argumento.
    const SIZE: u32 = 16 + 4 + 4 + 80;
    let len = arg.len().min(79);
    unsafe {
        for i in 0..32 {
            write_volatile(pkt.add(i), 0);
        }
        write_volatile(pkt.add(4), len as u32 + 1);
        let text = pkt.add(6) as *mut u8;
        for (i, &b) in arg[..len].iter().enumerate() {
            write_volatile(text.add(i), b);
        }
    }
    header(pkt, CMD_RESET, SIZE, 0, 0);
    kernel::sif_stop_dma();
    kernel::sync();
    kernel::sif_set_reg(REG_SMFLAG, STAT_BOOTEND);
    let iopbuf = kernel::sif_get_reg(SYSREG_SUBADDR);
    let packet = SifDma { src: pkt as u32, dest: iopbuf, size: SIZE, attr: DMA_ERT | DMA_INT_O };
    while kernel::sif_set_dma(core::slice::from_ref(&packet), false) == 0 {}
    kernel::sif_set_reg(REG_SMFLAG, STAT_SIFINIT);
    kernel::sif_set_reg(REG_SMFLAG, STAT_CMDINIT);
    kernel::sif_set_reg(SYSREG_RPCINIT, 0);
    kernel::sif_set_reg(SYSREG_SUBADDR, 0);
    // El IOP avisa cuando ha terminado de arrancar.
    while kernel::sif_get_reg(REG_SMFLAG) & STAT_BOOTEND == 0 {}
    unsafe {
        write_volatile(addr_of_mut!(READY), false);
        write_volatile(addr_of_mut!(IOPBUF), 0);
        for i in 0..32 {
            write_volatile(addr_of_mut!(SREGS[i]), 0);
        }
    }
    kernel::signal_sema(lock);
    init();
}

/// Prepara el paquete saliente común de bind y call.
fn prepare(client: &mut Client) -> *mut u32 {
    let pkt = uncached(unsafe { addr_of!(OUT.0) } as *const u32);
    unsafe {
        for i in 0..16 {
            write_volatile(pkt.add(i), 0);
        }
        let id = RPC_ID;
        RPC_ID = RPC_ID.wrapping_add(1).max(1);
        write_volatile(pkt.add(4), 0x04 | 0x01); // rec_id: paquete 0, asignado
        write_volatile(pkt.add(5), pkt as u32); // pkt_addr
        write_volatile(pkt.add(6), id); // rpc_id
        write_volatile(pkt.add(7), client as *mut Client as u32); // cd
    }
    pkt
}

fn wait_done(client: &mut Client, sema: i32) {
    kernel::wait_sema(sema);
    while unsafe { read_volatile(addr_of!(client.done)) } == 0 {
        kernel::rotate_ready_queue(kernel::THREAD_PRIORITY);
    }
}

fn locked<R>(f: impl FnOnce(i32) -> R) -> R {
    init();
    let lock = unsafe { LOCK_SEMA };
    kernel::wait_sema(lock);
    let done_sema = kernel::create_sema(0, 1);
    let r = f(done_sema);
    kernel::delete_sema(done_sema);
    kernel::signal_sema(lock);
    r
}

/// Liga `client` al servidor RPC `sid` del IOP. Reintenta hasta `attempts`
/// veces mientras el servidor no exista todavía; devuelve si lo consiguió.
pub fn bind(client: &mut Client, sid: u32, attempts: u32) -> bool {
    for _ in 0..attempts.max(1) {
        let bound = locked(|sema| {
            client.server = 0;
            client.done = 0;
            client.sema = sema;
            let pkt = prepare(client);
            unsafe { write_volatile(pkt.add(8), sid) };
            send_cmd_retry(CMD_RPC_BIND, pkt, 64, None);
            wait_done(client, sema);
            client.sema = -1;
            client.server != 0
        });
        if bound {
            return true;
        }
        for _ in 0..2000 {
            kernel::rotate_ready_queue(kernel::THREAD_PRIORITY);
        }
    }
    false
}

/// Llama a la función `fno` del servidor. `send` (alineado a 16 bytes, en
/// RAM) se copia al búfer del servidor; la respuesta (`recv_size` bytes) la
/// escribe el IOP en `recv` por DMA, así que `recv` debe estar alineado a 64
/// bytes y leerse sin caché.
pub fn call(client: &mut Client, fno: u32, send: *const u8, send_size: u32, recv: *mut u8, recv_size: u32) {
    locked(|sema| {
        client.done = 0;
        client.sema = sema;
        let pkt = prepare(client);
        unsafe {
            write_volatile(pkt.add(8), fno);
            write_volatile(pkt.add(9), send_size);
            write_volatile(pkt.add(10), recv as u32);
            write_volatile(pkt.add(11), recv_size);
            write_volatile(pkt.add(12), 1); // rmode: avisar al terminar
            write_volatile(pkt.add(13), client.server);
        }
        let extra = if send_size > 0 { Some((send as u32, client.buf, send_size)) } else { None };
        send_cmd_retry(CMD_RPC_CALL, pkt, 64, extra);
        wait_done(client, sema);
        client.sema = -1;
    });
}

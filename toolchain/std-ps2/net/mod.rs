//! `std::net` para la PlayStation 2.
//!
//! La pila TCP/IP (`stack.rs`) corre aquí, en el Emotion Engine. Del
//! adaptador de red se ocupa un módulo del IOP, `ps2net.irx` (también en
//! Rust), que sólo envía y recibe tramas Ethernet; se carga de la carpeta del
//! ejecutable la primera vez que se usa la red, y entonces se pide dirección
//! por DHCP.
//!
//! La pila avanza cuando el programa llama a la red (leer, escribir,
//! conectar…): no hay un hilo propio que la mueva. Sólo IPv4.

mod deps {
    pub use crate::collections::VecDeque;
    pub use crate::vec::Vec;
}

mod stack;

use self::stack::{Device, Handle, Mac, Stack, TcpState};
use crate::collections::VecDeque;
use crate::fmt;
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};
use crate::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, ToSocketAddrs};
use crate::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use crate::sync::{Arc, Mutex};
use crate::sys::os::{kernel, loadfile, sched, sif};
use crate::time::Duration;

/// Identificador del servidor RPC de `ps2net.irx`: "PSNT".
const SERVER_ID: u32 = 0x5053_4E54;
const FN_INIT: u32 = 1;
const FN_EXCHANGE: u32 = 2;
/// Tamaño de los búferes de intercambio de tramas con el IOP.
const BUFFER: usize = 16 * 1024;

#[repr(C, align(64))]
struct Aligned([u8; BUFFER]);

/// El adaptador de red, al otro lado del RPC.
struct Link {
    client: Box<sif::Client>,
    send: Box<Aligned>,
    recv: Box<Aligned>,
    mac: Mac,
    /// Tramas por enviar en el próximo intercambio.
    outgoing: VecDeque<Vec<u8>>,
    incoming: VecDeque<Vec<u8>>,
}

impl Link {
    /// Un viaje al IOP: entrega las tramas pendientes y recoge las recibidas.
    ///
    /// Mensajes: un `u32` con el número de tramas y, por cada una, un `u16`
    /// con su longitud seguido de sus bytes.
    fn exchange(&mut self) {
        let send = kernel::uncached(self.send.0.as_ptr());
        let recv = kernel::uncached(self.recv.0.as_ptr());
        unsafe {
            let (mut count, mut offset) = (0u32, 4usize);
            while let Some(frame) = self.outgoing.front() {
                if offset + 2 + frame.len() > BUFFER {
                    break;
                }
                send.add(offset).copy_from_nonoverlapping((frame.len() as u16).to_le_bytes().as_ptr(), 2);
                send.add(offset + 2).copy_from_nonoverlapping(frame.as_ptr(), frame.len());
                offset += 2 + frame.len();
                count += 1;
                self.outgoing.pop_front();
            }
            send.copy_from_nonoverlapping(count.to_le_bytes().as_ptr(), 4);
            sif::call(&mut self.client, FN_EXCHANGE, send, ((offset + 15) & !15) as u32, recv, BUFFER as u32);
            let mut header = [0u8; 4];
            recv.copy_to_nonoverlapping(header.as_mut_ptr(), 4);
            let mut offset = 4usize;
            for _ in 0..u32::from_le_bytes(header) {
                if offset + 2 > BUFFER {
                    break;
                }
                let mut len = [0u8; 2];
                recv.add(offset).copy_to_nonoverlapping(len.as_mut_ptr(), 2);
                let len = u16::from_le_bytes(len) as usize;
                if offset + 2 + len > BUFFER {
                    break;
                }
                let mut frame = vec![0u8; len];
                recv.add(offset + 2).copy_to_nonoverlapping(frame.as_mut_ptr(), len);
                self.incoming.push_back(frame);
                offset += 2 + len;
            }
        }
    }
}

impl Device for Link {
    fn mac(&self) -> Mac {
        self.mac
    }

    fn transmit(&mut self, frame: &[u8]) {
        if self.outgoing.len() < 64 {
            self.outgoing.push_back(frame.to_vec());
        }
    }

    fn receive(&mut self) -> Option<Vec<u8>> {
        self.incoming.pop_front()
    }
}

static NET: Mutex<Option<Stack<Link>>> = Mutex::new(None);

fn now_ms() -> u64 {
    (sched::cycles_to_nanos(sched::now()) / 1_000_000) as u64
}

fn sleep_ms(ms: u64) {
    sched::sleep_until(sched::now() + sched::nanos_to_cycles(ms as u128 * 1_000_000));
}

fn error(e: stack::Error) -> io::Error {
    use stack::Error::*;
    match e {
        NotConfigured => io::const_error!(io::ErrorKind::NetworkUnreachable, "the network interface has no address"),
        AddrInUse => io::const_error!(io::ErrorKind::AddrInUse, "address in use"),
        InvalidHandle | NotConnected => io::const_error!(io::ErrorKind::NotConnected, "socket is not connected"),
        ConnectionRefused => io::const_error!(io::ErrorKind::ConnectionRefused, "connection refused"),
        ConnectionReset => io::const_error!(io::ErrorKind::ConnectionReset, "connection reset by peer"),
        TimedOut => io::const_error!(io::ErrorKind::TimedOut, "connection timed out"),
        TooLarge => io::const_error!(io::ErrorKind::InvalidInput, "datagram too large"),
        NotFound => io::const_error!(io::ErrorKind::NotFound, "failed to lookup address information"),
        Protocol => io::const_error!(io::ErrorKind::InvalidData, "invalid DNS response"),
    }
}

/// Carga `ps2net.irx` de la carpeta del ejecutable y arranca el adaptador.
fn start_link() -> io::Result<Link> {
    let mut client = Box::new(sif::Client::new());
    // Puede que otro programa (o una llamada anterior) ya lo dejara cargado.
    if !sif::bind(&mut client, SERVER_ID, 1) {
        let dir = crate::sys::paths::exe_dir();
        let mut loaded = false;
        for name in ["ps2net.irx", "PS2NET.IRX", "PS2NET.IRX;1"] {
            let mut path = dir.clone().into_os_string().into_encoded_bytes();
            path.extend_from_slice(name.as_bytes());
            if loadfile::load_module(&path) >= 0 {
                loaded = true;
                break;
            }
        }
        if !loaded {
            return Err(io::const_error!(io::ErrorKind::NotFound, "ps2net.irx not found next to the executable"));
        }
        if !sif::bind(&mut client, SERVER_ID, 200) {
            return Err(io::const_error!(io::ErrorKind::Other, "ps2net.irx does not answer"));
        }
    }
    let mut link = Link { client, send: Box::new(Aligned([0; BUFFER])), recv: Box::new(Aligned([0; BUFFER])), mac: [0; 6], outgoing: VecDeque::new(), incoming: VecDeque::new() };
    // Los búferes se usan sólo por el espejo sin caché: que no quede nada
    // suyo en la caché pendiente de escribirse.
    kernel::flush_dcache();
    let recv = kernel::uncached(link.recv.0.as_ptr());
    let send = kernel::uncached(link.send.0.as_ptr());
    let mut reply = [0u8; 12];
    unsafe {
        sif::call(&mut link.client, FN_INIT, send, 16, recv, 64);
        recv.copy_to_nonoverlapping(reply.as_mut_ptr(), 12);
    }
    // Respuesta: estado (0 = bien) y dirección MAC.
    if u32::from_le_bytes([reply[0], reply[1], reply[2], reply[3]]) != 0 {
        return Err(io::const_error!(io::ErrorKind::NotFound, "no network adapter"));
    }
    link.mac.copy_from_slice(&reply[4..10]);
    Ok(link)
}

/// Mueve la pila: intercambia tramas con el IOP y procesa.
fn pump(stack: &mut Stack<Link>) {
    stack.device.exchange();
    stack.poll(now_ms());
    if !stack.device.outgoing.is_empty() {
        stack.device.exchange();
    }
}

/// Ejecuta `f` con la pila, arrancándola la primera vez (adaptador y DHCP).
fn with<R>(f: impl FnOnce(&mut Stack<Link>) -> io::Result<R>) -> io::Result<R> {
    let mut guard = NET.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let mut stack = Stack::new(start_link()?);
        stack.start_dhcp();
        *guard = Some(stack);
    }
    f(guard.as_mut().unwrap())
}

/// Repite `attempt` (moviendo la pila entre intentos) hasta que dé un valor.
/// Sin bloqueo o al pasar el plazo, falla con `WouldBlock` o `TimedOut`.
fn wait<R>(timeout: Option<Duration>, nonblocking: bool, mut attempt: impl FnMut(&mut Stack<Link>) -> io::Result<Option<R>>) -> io::Result<R> {
    let started = now_ms();
    let deadline = timeout.map(|t| started + t.as_millis().min(u64::MAX as u128 / 2) as u64);
    loop {
        let result = with(|stack| {
            pump(stack);
            attempt(stack)
        })?;
        if let Some(value) = result {
            return Ok(value);
        }
        if nonblocking {
            return Err(io::const_error!(io::ErrorKind::WouldBlock, "operation would block"));
        }
        if deadline.is_some_and(|d| now_ms() >= d) {
            return Err(io::const_error!(io::ErrorKind::TimedOut, "timed out"));
        }
        // Los primeros milisegundos se sondea cediendo la CPU a los demás
        // hilos (una respuesta rápida no espera al reloj, que avanza a saltos
        // de un frame); después se duerme entre sondeos.
        if now_ms() - started < 4 {
            crate::thread::yield_now();
        } else {
            sleep_ms(1);
        }
    }
}

/// Espera (hasta 15 s) a que la interfaz tenga dirección.
fn configured() -> io::Result<stack::Config> {
    wait(Some(Duration::from_secs(15)), false, |stack| Ok(stack.config())).map_err(|e| {
        if e.kind() == io::ErrorKind::TimedOut {
            io::const_error!(io::ErrorKind::NetworkUnreachable, "no DHCP answer")
        } else {
            e
        }
    })
}

fn v4(addr: &SocketAddr) -> io::Result<SocketAddrV4> {
    match addr {
        SocketAddr::V4(a) => Ok(*a),
        SocketAddr::V6(_) => Err(io::const_error!(io::ErrorKind::Unsupported, "IPv6 is not supported on the PS2")),
    }
}

fn local_ip() -> Ipv4Addr {
    with(|s| Ok(s.config().map(|c| Ipv4Addr::from(c.ip)).unwrap_or(Ipv4Addr::UNSPECIFIED))).unwrap_or(Ipv4Addr::UNSPECIFIED)
}

/// Configuración de red para `std::os::ps2`: (MAC, IP, máscara, puerta de
/// enlace, DNS). Arranca la red y espera al DHCP si hace falta.
pub fn ps2_network_info() -> io::Result<([u8; 6], [Ipv4Addr; 5])> {
    let c = configured()?;
    let mac = with(|s| Ok(s.mac()))?;
    Ok((mac, [c.ip.into(), c.mask.into(), c.gateway.into(), c.dns[0].into(), c.dns[1].into()]))
}

/// Fija la dirección a mano, sin DHCP.
pub fn ps2_network_configure(ip: Ipv4Addr, mask: Ipv4Addr, gateway: Ipv4Addr, dns: Ipv4Addr) -> io::Result<()> {
    with(|s| {
        s.set_config(stack::Config { ip: ip.octets(), mask: mask.octets(), gateway: gateway.octets(), dns: [dns.octets(), [0; 4]] });
        Ok(())
    })
}

/// Lo que comparten un socket y sus duplicados.
struct Shared {
    handle: Handle,
    nonblocking: AtomicBool,
    /// Plazos de lectura y escritura en milisegundos (0 = sin plazo).
    read_timeout: AtomicU32,
    write_timeout: AtomicU32,
    ttl: AtomicU32,
    flags: AtomicU32,
}

const FLAG_NODELAY: u32 = 1;
const FLAG_KEEPALIVE: u32 = 2;
const FLAG_BROADCAST: u32 = 4;
const FLAG_READ_CLOSED: u32 = 8;

impl Shared {
    fn new(handle: Handle) -> Arc<Shared> {
        Arc::new(Shared { handle, nonblocking: AtomicBool::new(false), read_timeout: AtomicU32::new(0), write_timeout: AtomicU32::new(0), ttl: AtomicU32::new(64), flags: AtomicU32::new(0) })
    }

    fn timeout(slot: &AtomicU32) -> Option<Duration> {
        match slot.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(Duration::from_millis(ms as u64)),
        }
    }

    fn set_timeout(slot: &AtomicU32, timeout: Option<Duration>) -> io::Result<()> {
        let ms = match timeout {
            None => 0,
            Some(t) if t.is_zero() => return Err(io::const_error!(io::ErrorKind::InvalidInput, "cannot set a 0 duration timeout")),
            Some(t) => t.as_millis().clamp(1, u32::MAX as u128) as u32,
        };
        slot.store(ms, Ordering::Relaxed);
        Ok(())
    }

    fn flag(&self, flag: u32) -> bool {
        self.flags.load(Ordering::Relaxed) & flag != 0
    }

    fn set_flag(&self, flag: u32, on: bool) {
        if on {
            self.flags.fetch_or(flag, Ordering::Relaxed);
        } else {
            self.flags.fetch_and(!flag, Ordering::Relaxed);
        }
    }

    fn nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Relaxed)
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        let handle = self.handle;
        let _ = with(|stack| {
            stack.release(handle);
            pump(stack);
            Ok(())
        });
    }
}

// ---------------------------------------------------------------- TCP

pub struct TcpStream(Arc<Shared>);

impl TcpStream {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<TcpStream> {
        super::each_addr(addr, |a| TcpStream::connect_one(a, None))
    }

    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        if timeout.is_zero() {
            return Err(io::const_error!(io::ErrorKind::InvalidInput, "cannot set a 0 duration timeout"));
        }
        TcpStream::connect_one(addr, Some(timeout))
    }

    fn connect_one(addr: &SocketAddr, timeout: Option<Duration>) -> io::Result<TcpStream> {
        let addr = v4(addr)?;
        if *addr.ip() != Ipv4Addr::LOCALHOST {
            configured()?;
        }
        let handle = with(|s| s.tcp_connect(addr.ip().octets(), addr.port()).map_err(error))?;
        let stream = TcpStream(Shared::new(handle));
        wait(timeout, false, |s| match s.tcp_state(handle).map_err(error)? {
            (TcpState::Established | TcpState::CloseWait, _) => Ok(Some(())),
            (TcpState::Closed, e) => Err(error(e.unwrap_or(stack::Error::ConnectionRefused))),
            _ => Ok(None),
        })?;
        Ok(stream)
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        Shared::set_timeout(&self.0.read_timeout, timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        Shared::set_timeout(&self.0.write_timeout, timeout)
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(Shared::timeout(&self.0.read_timeout))
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(Shared::timeout(&self.0.write_timeout))
    }

    fn receive(&self, buf: &mut [u8], peek: bool) -> io::Result<usize> {
        if buf.is_empty() || self.0.flag(FLAG_READ_CLOSED) {
            return Ok(0);
        }
        let handle = self.0.handle;
        wait(Shared::timeout(&self.0.read_timeout), self.0.nonblocking(), |s| s.tcp_recv(handle, buf, peek).map_err(error))
    }

    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.receive(buf, true)
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.receive(buf, false)
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        io::default_read_buf(|buf| self.read(buf), cursor)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        io::default_read_vectored(|buf| self.read(buf), bufs)
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let handle = self.0.handle;
        wait(Shared::timeout(&self.0.write_timeout), self.0.nonblocking(), |s| match s.tcp_send(handle, buf) {
            // Búfer de envío lleno: se espera a que el otro extremo confirme.
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(n)),
            Err(stack::Error::NotConnected) => Err(io::const_error!(io::ErrorKind::BrokenPipe, "the connection is closed for writing")),
            Err(e) => Err(error(e)),
        })
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        io::default_write_vectored(|buf| self.write(buf), bufs)
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        let handle = self.0.handle;
        let (ip, port) = with(|s| s.tcp_remote(handle).map_err(error))?;
        Ok(SocketAddr::V4(SocketAddrV4::new(ip.into(), port)))
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        let handle = self.0.handle;
        let (remote, port) = with(|s| Ok((s.tcp_remote(handle).map_err(error)?, s.local_port(handle).map_err(error)?)))?;
        let ip = if remote.0 == [127, 0, 0, 1] { Ipv4Addr::LOCALHOST } else { local_ip() };
        Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let handle = self.0.handle;
        if matches!(how, Shutdown::Read | Shutdown::Both) {
            self.0.set_flag(FLAG_READ_CLOSED, true);
        }
        if matches!(how, Shutdown::Write | Shutdown::Both) {
            with(|s| {
                s.tcp_shutdown_write(handle).map_err(error)?;
                pump(s);
                Ok(())
            })?;
        }
        Ok(())
    }

    pub fn duplicate(&self) -> io::Result<TcpStream> {
        Ok(TcpStream(self.0.clone()))
    }

    /// Al soltar el socket se cierra con orden en segundo plano en todo caso.
    pub fn set_linger(&self, _: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    pub fn set_keepalive(&self, on: bool) -> io::Result<()> {
        self.0.set_flag(FLAG_KEEPALIVE, on);
        Ok(())
    }

    pub fn keepalive(&self) -> io::Result<bool> {
        Ok(self.0.flag(FLAG_KEEPALIVE))
    }

    /// Los datos salen siempre de inmediato (no hay algoritmo de Nagle).
    pub fn set_nodelay(&self, on: bool) -> io::Result<()> {
        self.0.set_flag(FLAG_NODELAY, on);
        Ok(())
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        Ok(self.0.flag(FLAG_NODELAY))
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.0.ttl.store(ttl, Ordering::Relaxed);
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(self.0.ttl.load(Ordering::Relaxed))
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.0.nonblocking.store(nonblocking, Ordering::Relaxed);
        Ok(())
    }
}

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("TcpStream");
        if let Ok(addr) = self.socket_addr() {
            d.field("addr", &addr);
        }
        if let Ok(peer) = self.peer_addr() {
            d.field("peer", &peer);
        }
        d.finish()
    }
}

pub struct TcpListener(Arc<Shared>);

impl TcpListener {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<TcpListener> {
        super::each_addr(addr, |a| {
            let addr = v4(a)?;
            let handle = with(|s| s.tcp_listen(addr.port()).map_err(error))?;
            Ok(TcpListener(Shared::new(handle)))
        })
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        let handle = self.0.handle;
        let port = with(|s| s.local_port(handle).map_err(error))?;
        Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)))
    }

    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let handle = self.0.handle;
        let accepted = wait(None, self.0.nonblocking(), |s| s.tcp_accept(handle).map_err(error))?;
        let stream = TcpStream(Shared::new(accepted));
        let peer = stream.peer_addr()?;
        Ok((stream, peer))
    }

    pub fn duplicate(&self) -> io::Result<TcpListener> {
        Ok(TcpListener(self.0.clone()))
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.0.ttl.store(ttl, Ordering::Relaxed);
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(self.0.ttl.load(Ordering::Relaxed))
    }

    pub fn set_only_v6(&self, _: bool) -> io::Result<()> {
        Err(io::const_error!(io::ErrorKind::Unsupported, "IPv6 is not supported on the PS2"))
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        Ok(false)
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.0.nonblocking.store(nonblocking, Ordering::Relaxed);
        Ok(())
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("TcpListener");
        if let Ok(addr) = self.socket_addr() {
            d.field("addr", &addr);
        }
        d.finish()
    }
}

// ---------------------------------------------------------------- UDP

pub struct UdpSocket {
    shared: Arc<Shared>,
    /// Destino fijado con `connect`.
    peer: Arc<Mutex<Option<SocketAddrV4>>>,
}

impl UdpSocket {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<UdpSocket> {
        super::each_addr(addr, |a| {
            let addr = v4(a)?;
            let handle = with(|s| s.udp_bind(addr.port()).map_err(error))?;
            Ok(UdpSocket { shared: Shared::new(handle), peer: Arc::new(Mutex::new(None)) })
        })
    }

    fn peer(&self) -> Option<SocketAddrV4> {
        *self.peer.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.peer().map(SocketAddr::V4).ok_or(io::const_error!(io::ErrorKind::NotConnected, "socket is not connected"))
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        let handle = self.shared.handle;
        let port = with(|s| s.local_port(handle).map_err(error))?;
        Ok(SocketAddr::V4(SocketAddrV4::new(local_ip(), port)))
    }

    fn receive(&self, buf: &mut [u8], peek: bool) -> io::Result<(usize, SocketAddr)> {
        let handle = self.shared.handle;
        let peer = self.peer();
        wait(Shared::timeout(&self.shared.read_timeout), self.shared.nonblocking(), |s| loop {
            let Some((ip, port, data)) = s.udp_recv(handle, peek).map_err(error)? else { return Ok(None) };
            let from = SocketAddrV4::new(ip.into(), port);
            // Un socket conectado sólo ve lo que viene de su destino.
            if peer.is_some_and(|p| p != from) {
                if peek {
                    let _ = s.udp_recv(handle, false);
                }
                continue;
            }
            // Lo que no cabe en el búfer se pierde, como en cualquier UDP.
            let count = data.len().min(buf.len());
            buf[..count].copy_from_slice(&data[..count]);
            return Ok(Some((count, SocketAddr::V4(from))));
        })
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.receive(buf, false)
    }

    pub fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.receive(buf, true)
    }

    pub fn send_to(&self, buf: &[u8], addr: &SocketAddr) -> io::Result<usize> {
        let addr = v4(addr)?;
        if *addr.ip() != Ipv4Addr::LOCALHOST {
            configured()?;
        }
        if addr.ip().is_broadcast() && !self.shared.flag(FLAG_BROADCAST) {
            return Err(io::const_error!(io::ErrorKind::PermissionDenied, "broadcast is not enabled on this socket"));
        }
        let handle = self.shared.handle;
        with(|s| {
            s.udp_send(handle, addr.ip().octets(), addr.port(), buf).map_err(error)?;
            pump(s);
            Ok(buf.len())
        })
    }

    pub fn duplicate(&self) -> io::Result<UdpSocket> {
        Ok(UdpSocket { shared: self.shared.clone(), peer: self.peer.clone() })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        Shared::set_timeout(&self.shared.read_timeout, timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        Shared::set_timeout(&self.shared.write_timeout, timeout)
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(Shared::timeout(&self.shared.read_timeout))
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(Shared::timeout(&self.shared.write_timeout))
    }

    pub fn set_broadcast(&self, on: bool) -> io::Result<()> {
        self.shared.set_flag(FLAG_BROADCAST, on);
        Ok(())
    }

    pub fn broadcast(&self) -> io::Result<bool> {
        Ok(self.shared.flag(FLAG_BROADCAST))
    }

    pub fn set_multicast_loop_v4(&self, _: bool) -> io::Result<()> {
        no_multicast()
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        Ok(false)
    }

    pub fn set_multicast_ttl_v4(&self, _: u32) -> io::Result<()> {
        no_multicast()
    }

    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        Ok(1)
    }

    pub fn set_multicast_loop_v6(&self, _: bool) -> io::Result<()> {
        no_multicast()
    }

    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        Ok(false)
    }

    pub fn join_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        no_multicast()
    }

    pub fn join_multicast_v6(&self, _: &Ipv6Addr, _: u32) -> io::Result<()> {
        no_multicast()
    }

    pub fn leave_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        no_multicast()
    }

    pub fn leave_multicast_v6(&self, _: &Ipv6Addr, _: u32) -> io::Result<()> {
        no_multicast()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.shared.ttl.store(ttl, Ordering::Relaxed);
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(self.shared.ttl.load(Ordering::Relaxed))
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.shared.nonblocking.store(nonblocking, Ordering::Relaxed);
        Ok(())
    }

    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.receive(buf, false).map(|r| r.0)
    }

    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.receive(buf, true).map(|r| r.0)
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        match self.peer() {
            Some(peer) => self.send_to(buf, &SocketAddr::V4(peer)),
            None => Err(io::const_error!(io::ErrorKind::NotConnected, "socket is not connected")),
        }
    }

    pub fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<()> {
        super::each_addr(addr, |a| {
            *self.peer.lock().unwrap_or_else(|e| e.into_inner()) = Some(v4(a)?);
            Ok(())
        })
    }
}

fn no_multicast<T>() -> io::Result<T> {
    Err(io::const_error!(io::ErrorKind::Unsupported, "multicast is not supported on the PS2"))
}

impl fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("UdpSocket");
        if let Ok(addr) = self.socket_addr() {
            d.field("addr", &addr);
        }
        d.finish()
    }
}

// ---------------------------------------------------------------- DNS

pub struct LookupHost {
    addresses: crate::vec::IntoIter<SocketAddr>,
}

impl Iterator for LookupHost {
    type Item = SocketAddr;

    fn next(&mut self) -> Option<SocketAddr> {
        self.addresses.next()
    }
}

/// Resuelve un nombre de host a sus direcciones IPv4.
pub fn lookup_host(host: &str, port: u16) -> io::Result<LookupHost> {
    let found = if host.eq_ignore_ascii_case("localhost") {
        vec![[127, 0, 0, 1]]
    } else {
        configured()?;
        let lookup = with(|s| s.dns_start(host).map_err(error))?;
        wait(Some(Duration::from_secs(12)), false, |s| match s.dns_result(lookup) {
            None => Ok(None),
            Some(Ok(addresses)) => Ok(Some(addresses)),
            Some(Err(e)) => Err(error(e)),
        })?
    };
    let addresses: Vec<SocketAddr> = found.into_iter().map(|ip| SocketAddr::V4(SocketAddrV4::new(ip.into(), port))).collect();
    Ok(LookupHost { addresses: addresses.into_iter() })
}

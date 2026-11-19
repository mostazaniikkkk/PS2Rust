//! Pila TCP/IP de la PlayStation 2: Ethernet, ARP, IPv4, ICMP, UDP, TCP y
//! los clientes de DHCP y DNS.
//!
//! Es lógica pura: recibe y envía tramas por un [`Device`] y el tiempo le
//! llega como milisegundos en cada [`Stack::poll`]. No bloquea nunca; las
//! esperas las pone la capa de `std::net`. Así el mismo archivo se prueba en
//! el PC, con dos pilas conectadas por un cable simulado.
//!
//! Límites deliberados: sólo IPv4, sin fragmentación de IP, y un TCP sencillo
//! (retransmisión por temporizador, sin reordenar segmentos que llegan
//! adelantados ni control de congestión más allá de la ventana del otro
//! extremo).

use super::deps::{Vec, VecDeque};

pub type Ip = [u8; 4];
pub type Mac = [u8; 6];

/// El adaptador de red: envía y recibe tramas Ethernet completas (sin CRC).
pub trait Device {
    fn mac(&self) -> Mac;
    fn transmit(&mut self, frame: &[u8]);
    fn receive(&mut self) -> Option<Vec<u8>>;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// La interfaz aún no tiene dirección IP.
    NotConfigured,
    AddrInUse,
    InvalidHandle,
    NotConnected,
    ConnectionRefused,
    ConnectionReset,
    TimedOut,
    /// El datagrama no cabe en una trama.
    TooLarge,
    /// El nombre no existe (DNS).
    NotFound,
    /// Respuesta de DNS inválida o error del servidor.
    Protocol,
}

/// Identificador de un socket dentro de la pila.
pub type Handle = usize;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Config {
    pub ip: Ip,
    pub mask: Ip,
    pub gateway: Ip,
    pub dns: [Ip; 2],
}

const BROADCAST_MAC: Mac = [0xff; 6];
const BROADCAST_IP: Ip = [255; 4];
const LOOPBACK: Ip = [127, 0, 0, 1];
const ETH_ARP: u16 = 0x0806;
const ETH_IP: u16 = 0x0800;
const PROTO_ICMP: u8 = 1;
const PROTO_TCP: u8 = 6;
const PROTO_UDP: u8 = 17;
/// Carga útil máxima de una trama Ethernet.
const MTU: usize = 1500;
pub const UDP_MAX_PAYLOAD: usize = MTU - 20 - 8;
const TCP_MSS: usize = MTU - 20 - 20;
const TCP_RECV_BUFFER: usize = 32 * 1024;
const TCP_SEND_BUFFER: usize = 32 * 1024;
const TCP_RTO_MS: u64 = 500;
const TCP_MAX_RETRIES: u32 = 8;
const UDP_QUEUE: usize = 64;

const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn ip_at(b: &[u8], o: usize) -> Ip {
    [b[o], b[o + 1], b[o + 2], b[o + 3]]
}

/// Suma de comprobación de Internet (complemento a uno) de varios trozos.
pub fn checksum(parts: &[&[u8]]) -> u16 {
    let mut sum = 0u32;
    // Un byte suelto al final de un trozo se empareja con el primero del siguiente.
    let mut pending: Option<u8> = None;
    for part in parts {
        for &byte in *part {
            match pending.take() {
                Some(high) => sum += u16::from_be_bytes([high, byte]) as u32,
                None => pending = Some(byte),
            }
        }
    }
    if let Some(high) = pending {
        sum += (high as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// `a` va antes que `b` en el espacio circular de números de secuencia.
fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

fn seq_le(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) <= 0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TcpState {
    Closed,
    SynSent,
    SynReceived,
    Established,
    /// Hemos cerrado nuestro lado; aún se puede recibir.
    FinWait,
    /// El otro extremo cerró su lado; aún se puede enviar.
    CloseWait,
    /// Los dos lados han cerrado; falta que confirmen nuestro FIN.
    Closing,
}

struct Tcp {
    state: TcpState,
    local_port: u16,
    remote: (Ip, u16),
    /// Primer byte sin confirmar, siguiente a enviar y ventana del otro extremo.
    snd_una: u32,
    snd_nxt: u32,
    snd_wnd: u32,
    rcv_nxt: u32,
    /// Datos por confirmar y por enviar; el primero tiene el número `snd_una`.
    send_buf: VecDeque<u8>,
    recv_buf: VecDeque<u8>,
    /// Segmento máximo que acepta el otro extremo.
    mss: usize,
    /// La aplicación cerró la escritura: hay que enviar FIN tras los datos.
    fin_queued: bool,
    fin_sent: bool,
    /// Llegó el FIN del otro extremo.
    peer_fin: bool,
    /// Última ventana anunciada, para avisar cuando vuelve a abrirse.
    advertised: usize,
    retransmit_at: Option<u64>,
    retries: u32,
    error: Option<Error>,
    /// Socket de escucha del que nació, hasta que se acepta.
    parent: Option<Handle>,
    /// La aplicación lo soltó: se libera solo cuando termina de cerrarse.
    orphan_until: Option<u64>,
}

struct Udp {
    port: u16,
    queue: VecDeque<(Ip, u16, Vec<u8>)>,
}

struct Listener {
    port: u16,
    ready: VecDeque<Handle>,
}

enum DnsState {
    Waiting,
    Done(Result<Vec<Ip>, Error>),
}

struct Dns {
    id: u16,
    port: u16,
    query: Vec<u8>,
    attempts: u32,
    next_try: u64,
    state: DnsState,
}

enum Socket {
    Tcp(Tcp),
    Udp(Udp),
    Listener(Listener),
    Dns(Dns),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DhcpState {
    Off,
    Discovering,
    Requesting,
    Bound,
}

struct Dhcp {
    state: DhcpState,
    xid: u32,
    next_try: u64,
    attempts: u32,
    offered: Ip,
    server: Ip,
    renew_at: u64,
}

pub struct Stack<D: Device> {
    pub device: D,
    mac: Mac,
    config: Option<Config>,
    now: u64,
    arp: Vec<(Ip, Mac, u64)>,
    /// Paquetes IP que esperan la dirección MAC del siguiente salto.
    waiting_arp: Vec<(Ip, Vec<u8>, u64)>,
    arp_requests: Vec<(Ip, u64)>,
    /// Paquetes IP dirigidos a nosotros mismos.
    loopback: VecDeque<Vec<u8>>,
    sockets: Vec<Option<Socket>>,
    dhcp: Dhcp,
    ip_id: u16,
    next_port: u16,
    random: u32,
}

impl<D: Device> Stack<D> {
    pub fn new(device: D) -> Stack<D> {
        let mac = device.mac();
        let seed = u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]) ^ 0x9E37_79B9;
        Stack {
            device,
            mac,
            config: None,
            now: 0,
            arp: Vec::new(),
            waiting_arp: Vec::new(),
            arp_requests: Vec::new(),
            loopback: VecDeque::new(),
            sockets: Vec::new(),
            dhcp: Dhcp { state: DhcpState::Off, xid: 0, next_try: 0, attempts: 0, offered: [0; 4], server: [0; 4], renew_at: 0 },
            ip_id: 1,
            next_port: 49152 + (seed % 8192) as u16,
            random: seed | 1,
        }
    }

    pub fn mac(&self) -> Mac {
        self.mac
    }

    /// Configuración de la interfaz, si ya tiene dirección.
    pub fn config(&self) -> Option<Config> {
        self.config
    }

    /// Fija la dirección a mano (sin DHCP).
    pub fn set_config(&mut self, config: Config) {
        self.config = Some(config);
        self.dhcp.state = DhcpState::Off;
    }

    /// Empieza a pedir dirección por DHCP; `config()` dirá cuándo llega.
    pub fn start_dhcp(&mut self) {
        self.dhcp.xid = self.rand();
        self.dhcp.state = DhcpState::Discovering;
        self.dhcp.attempts = 0;
        self.dhcp.next_try = self.now;
    }

    fn rand(&mut self) -> u32 {
        let mut x = self.random ^ (self.now as u32).wrapping_mul(2_654_435_761);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.random = x | 1;
        x
    }

    fn insert(&mut self, socket: Socket) -> Handle {
        match self.sockets.iter().position(Option::is_none) {
            Some(free) => {
                self.sockets[free] = Some(socket);
                free
            }
            None => {
                self.sockets.push(Some(socket));
                self.sockets.len() - 1
            }
        }
    }

    fn port_in_use(&self, port: u16, tcp: bool) -> bool {
        self.sockets.iter().flatten().any(|s| match s {
            Socket::Tcp(t) => tcp && t.local_port == port,
            Socket::Listener(l) => tcp && l.port == port,
            Socket::Udp(u) => !tcp && u.port == port,
            Socket::Dns(d) => !tcp && d.port == port,
        })
    }

    fn ephemeral_port(&mut self, tcp: bool) -> u16 {
        loop {
            self.next_port = if self.next_port >= 65535 { 49152 } else { self.next_port + 1 };
            if !self.port_in_use(self.next_port, tcp) {
                return self.next_port;
            }
        }
    }

    // ------------------------------------------------------------ enlace e IP

    fn send_frame(&mut self, dst: Mac, ethertype: u16, payload: &[u8]) {
        let mut frame = Vec::with_capacity(14 + payload.len().max(46));
        frame.extend_from_slice(&dst);
        frame.extend_from_slice(&self.mac);
        frame.extend_from_slice(&ethertype.to_be_bytes());
        frame.extend_from_slice(payload);
        // Relleno hasta el mínimo de Ethernet.
        while frame.len() < 60 {
            frame.push(0);
        }
        self.device.transmit(&frame);
    }

    fn send_arp(&mut self, operation: u16, target_mac: Mac, target_ip: Ip) {
        let ip = self.config.map(|c| c.ip).unwrap_or([0; 4]);
        let mut p = Vec::with_capacity(28);
        p.extend_from_slice(&[0, 1, 8, 0, 6, 4]);
        p.extend_from_slice(&operation.to_be_bytes());
        p.extend_from_slice(&self.mac);
        p.extend_from_slice(&ip);
        p.extend_from_slice(&if operation == 1 { [0; 6] } else { target_mac });
        p.extend_from_slice(&target_ip);
        let dst = if operation == 1 { BROADCAST_MAC } else { target_mac };
        self.send_frame(dst, ETH_ARP, &p);
    }

    /// ¿Es una dirección nuestra? Sólo 127.0.0.1 es bucle local: el resto de
    /// 127.0.0.0/8 sale por el adaptador, lo que en un emulador permite
    /// llegar a servicios del PC anfitrión (por ejemplo en 127.0.0.2).
    fn is_local(&self, ip: Ip) -> bool {
        ip == LOOPBACK || self.config.map_or(false, |c| c.ip == ip)
    }

    /// Envía un paquete IP ya montado: decide el siguiente salto y resuelve
    /// su dirección MAC, dejando el paquete en espera si hace falta ARP.
    fn route(&mut self, dst: Ip, packet: Vec<u8>) {
        if self.is_local(dst) {
            self.loopback.push_back(packet);
            return;
        }
        let broadcast = dst == BROADCAST_IP || self.config.map_or(false, |c| (0..4).all(|i| dst[i] == c.ip[i] | !c.mask[i]));
        if broadcast {
            self.send_frame(BROADCAST_MAC, ETH_IP, &packet);
            return;
        }
        let Some(c) = self.config else { return };
        let same_net = (0..4).all(|i| dst[i] & c.mask[i] == c.ip[i] & c.mask[i]);
        let hop = if same_net || c.gateway == [0; 4] { dst } else { c.gateway };
        if let Some(&(_, mac, _)) = self.arp.iter().find(|e| e.0 == hop) {
            self.send_frame(mac, ETH_IP, &packet);
            return;
        }
        if self.waiting_arp.len() < 32 {
            self.waiting_arp.push((hop, packet, self.now + 3000));
        }
        if !self.arp_requests.iter().any(|r| r.0 == hop) {
            self.arp_requests.push((hop, self.now + 1000));
            self.send_arp(1, [0; 6], hop);
        }
    }

    fn send_ip(&mut self, src: Ip, dst: Ip, protocol: u8, payload: &[u8]) {
        let mut p = Vec::with_capacity(20 + payload.len());
        let total = (20 + payload.len()) as u16;
        self.ip_id = self.ip_id.wrapping_add(1);
        p.extend_from_slice(&[0x45, 0]);
        p.extend_from_slice(&total.to_be_bytes());
        p.extend_from_slice(&self.ip_id.to_be_bytes());
        p.extend_from_slice(&[0x40, 0, 64, protocol, 0, 0]);
        p.extend_from_slice(&src);
        p.extend_from_slice(&dst);
        let sum = checksum(&[&p]);
        p[10..12].copy_from_slice(&sum.to_be_bytes());
        p.extend_from_slice(payload);
        self.route(dst, p);
    }

    /// Dirección de origen para hablar con `dst`.
    fn source_for(&self, dst: Ip) -> Ip {
        if dst == LOOPBACK {
            dst
        } else {
            self.config.map(|c| c.ip).unwrap_or([0; 4])
        }
    }

    fn handle_arp(&mut self, p: &[u8]) {
        if p.len() < 28 || be16(p, 0) != 1 || be16(p, 2) != ETH_IP {
            return;
        }
        let operation = be16(p, 6);
        let sender_mac: Mac = [p[8], p[9], p[10], p[11], p[12], p[13]];
        let (sender_ip, target_ip) = (ip_at(p, 14), ip_at(p, 24));
        if sender_ip != [0; 4] {
            let expiry = self.now + 600_000;
            match self.arp.iter_mut().find(|e| e.0 == sender_ip) {
                Some(entry) => *entry = (sender_ip, sender_mac, expiry),
                None => {
                    if self.arp.len() >= 32 {
                        self.arp.remove(0);
                    }
                    self.arp.push((sender_ip, sender_mac, expiry));
                }
            }
            self.arp_requests.retain(|r| r.0 != sender_ip);
            // Sale lo que esperaba esta dirección.
            let mut i = 0;
            while i < self.waiting_arp.len() {
                if self.waiting_arp[i].0 == sender_ip {
                    let (_, packet, _) = self.waiting_arp.remove(i);
                    self.send_frame(sender_mac, ETH_IP, &packet);
                } else {
                    i += 1;
                }
            }
        }
        if operation == 1 && self.config.map_or(false, |c| c.ip == target_ip) {
            self.send_arp(2, sender_mac, sender_ip);
        }
    }

    fn handle_ip(&mut self, p: &[u8]) {
        if p.len() < 20 || p[0] >> 4 != 4 {
            return;
        }
        let header = ((p[0] & 15) as usize) * 4;
        let total = be16(p, 2) as usize;
        if header < 20 || total < header || total > p.len() || checksum(&[&p[..header]]) != 0 {
            return;
        }
        // Sin reensamblado: los fragmentos se descartan.
        if be16(p, 6) & 0x3fff != 0 {
            return;
        }
        let (src, dst) = (ip_at(p, 12), ip_at(p, 16));
        let for_us = match self.config {
            Some(c) => dst == c.ip || dst == BROADCAST_IP || dst[0] == 127 || (0..4).all(|i| dst[i] == c.ip[i] | !c.mask[i]),
            // Sin dirección todavía (DHCP en marcha) se acepta todo.
            None => true,
        };
        if !for_us {
            return;
        }
        let payload = &p[header..total];
        match p[9] {
            PROTO_ICMP => self.handle_icmp(src, dst, payload),
            PROTO_UDP => self.handle_udp(src, dst, payload),
            PROTO_TCP => self.handle_tcp(src, dst, payload),
            _ => {}
        }
    }

    fn handle_icmp(&mut self, src: Ip, dst: Ip, p: &[u8]) {
        // Sólo se responde al eco (ping).
        if p.len() >= 8 && p[0] == 8 && checksum(&[p]) == 0 {
            let mut reply = p.to_vec();
            reply[0] = 0;
            reply[2] = 0;
            reply[3] = 0;
            let sum = checksum(&[&reply]);
            reply[2..4].copy_from_slice(&sum.to_be_bytes());
            self.send_ip(dst, src, PROTO_ICMP, &reply);
        }
    }

    // ------------------------------------------------------------ UDP

    fn send_udp(&mut self, src_port: u16, dst: Ip, dst_port: u16, data: &[u8]) {
        let src = self.source_for(dst);
        let length = (8 + data.len()) as u16;
        let mut p = Vec::with_capacity(8 + data.len());
        p.extend_from_slice(&src_port.to_be_bytes());
        p.extend_from_slice(&dst_port.to_be_bytes());
        p.extend_from_slice(&length.to_be_bytes());
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(data);
        let pseudo = [src[0], src[1], src[2], src[3], dst[0], dst[1], dst[2], dst[3], 0, PROTO_UDP, (length >> 8) as u8, length as u8];
        let mut sum = checksum(&[&pseudo, &p]);
        if sum == 0 {
            sum = 0xffff;
        }
        p[6..8].copy_from_slice(&sum.to_be_bytes());
        self.send_ip(src, dst, PROTO_UDP, &p);
    }

    fn handle_udp(&mut self, src: Ip, dst: Ip, p: &[u8]) {
        if p.len() < 8 {
            return;
        }
        let length = be16(p, 4) as usize;
        if length < 8 || length > p.len() {
            return;
        }
        let p = &p[..length];
        if be16(p, 6) != 0 {
            let pseudo = [src[0], src[1], src[2], src[3], dst[0], dst[1], dst[2], dst[3], 0, PROTO_UDP, (length >> 8) as u8, length as u8];
            if checksum(&[&pseudo, p]) != 0 {
                return;
            }
        }
        let (src_port, dst_port) = (be16(p, 0), be16(p, 2));
        let data = &p[8..];
        if dst_port == 68 && self.dhcp.state != DhcpState::Off {
            self.handle_dhcp(data);
            return;
        }
        for index in 0..self.sockets.len() {
            match &mut self.sockets[index] {
                Some(Socket::Udp(u)) if u.port == dst_port => {
                    if u.queue.len() < UDP_QUEUE {
                        u.queue.push_back((src, src_port, data.to_vec()));
                    }
                    return;
                }
                Some(Socket::Dns(d)) if d.port == dst_port => {
                    if let DnsState::Waiting = d.state {
                        if let Some(result) = parse_dns(d.id, data) {
                            d.state = DnsState::Done(result);
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
    }

    /// Abre un socket UDP en `port` (0 = uno libre cualquiera).
    pub fn udp_bind(&mut self, port: u16) -> Result<Handle, Error> {
        let port = if port == 0 {
            self.ephemeral_port(false)
        } else if self.port_in_use(port, false) {
            return Err(Error::AddrInUse);
        } else {
            port
        };
        Ok(self.insert(Socket::Udp(Udp { port, queue: VecDeque::new() })))
    }

    pub fn udp_send(&mut self, handle: Handle, dst: Ip, dst_port: u16, data: &[u8]) -> Result<(), Error> {
        let Some(Some(Socket::Udp(u))) = self.sockets.get(handle) else { return Err(Error::InvalidHandle) };
        let port = u.port;
        if data.len() > UDP_MAX_PAYLOAD {
            return Err(Error::TooLarge);
        }
        if self.config.is_none() && dst != LOOPBACK {
            return Err(Error::NotConfigured);
        }
        self.send_udp(port, dst, dst_port, data);
        Ok(())
    }

    /// Saca (o sólo mira, con `peek`) el siguiente datagrama recibido.
    pub fn udp_recv(&mut self, handle: Handle, peek: bool) -> Result<Option<(Ip, u16, Vec<u8>)>, Error> {
        let Some(Some(Socket::Udp(u))) = self.sockets.get_mut(handle) else { return Err(Error::InvalidHandle) };
        Ok(if peek { u.queue.front().cloned() } else { u.queue.pop_front() })
    }

    /// Puerto local de un socket (UDP, TCP o de escucha).
    pub fn local_port(&self, handle: Handle) -> Result<u16, Error> {
        match self.sockets.get(handle) {
            Some(Some(Socket::Udp(u))) => Ok(u.port),
            Some(Some(Socket::Tcp(t))) => Ok(t.local_port),
            Some(Some(Socket::Listener(l))) => Ok(l.port),
            _ => Err(Error::InvalidHandle),
        }
    }

    // ------------------------------------------------------------ DHCP

    fn send_dhcp(&mut self, kind: u8) {
        let mut p = alloc_zeroed(240);
        p[0] = 1; // petición
        p[1] = 1; // Ethernet
        p[2] = 6;
        p[4..8].copy_from_slice(&self.dhcp.xid.to_be_bytes());
        p[10] = 0x80; // que respondan por difusión
        let renewing = self.dhcp.state == DhcpState::Bound;
        if renewing {
            if let Some(c) = self.config {
                p[12..16].copy_from_slice(&c.ip);
            }
        }
        p[28..34].copy_from_slice(&self.mac);
        p[236..240].copy_from_slice(&[99, 130, 83, 99]);
        p.extend_from_slice(&[53, 1, kind]);
        if kind == 3 && !renewing {
            p.extend_from_slice(&[50, 4]);
            p.extend_from_slice(&self.dhcp.offered);
            p.extend_from_slice(&[54, 4]);
            p.extend_from_slice(&self.dhcp.server);
        }
        p.extend_from_slice(&[55, 3, 1, 3, 6, 255]);
        let src = if renewing { self.config.map(|c| c.ip).unwrap_or([0; 4]) } else { [0; 4] };
        let length = (8 + p.len()) as u16;
        let mut udp = Vec::with_capacity(8 + p.len());
        udp.extend_from_slice(&68u16.to_be_bytes());
        udp.extend_from_slice(&67u16.to_be_bytes());
        udp.extend_from_slice(&length.to_be_bytes());
        udp.extend_from_slice(&[0, 0]);
        udp.extend_from_slice(&p);
        let pseudo = [src[0], src[1], src[2], src[3], 255, 255, 255, 255, 0, PROTO_UDP, (length >> 8) as u8, length as u8];
        let sum = checksum(&[&pseudo, &udp]);
        udp[6..8].copy_from_slice(&(if sum == 0 { 0xffff } else { sum }).to_be_bytes());
        self.send_ip(src, BROADCAST_IP, PROTO_UDP, &udp);
    }

    fn handle_dhcp(&mut self, p: &[u8]) {
        if p.len() < 240 || p[0] != 2 || be32(p, 4) != self.dhcp.xid || p[236..240] != [99, 130, 83, 99] || p[28..34] != self.mac {
            return;
        }
        let offered = ip_at(p, 16);
        let (mut kind, mut mask, mut gateway, mut dns, mut server, mut lease) = (0u8, [255, 255, 255, 0], [0u8; 4], [[0u8; 4]; 2], [0u8; 4], 86_400u32);
        let mut i = 240;
        while i < p.len() && p[i] != 255 {
            if p[i] == 0 {
                i += 1;
                continue;
            }
            if i + 1 >= p.len() {
                break;
            }
            let (code, len) = (p[i], p[i + 1] as usize);
            let Some(value) = p.get(i + 2..i + 2 + len) else { break };
            match (code, len) {
                (53, 1) => kind = value[0],
                (1, 4) => mask = ip_at(value, 0),
                (3, n) if n >= 4 => gateway = ip_at(value, 0),
                (6, n) if n >= 4 => {
                    dns[0] = ip_at(value, 0);
                    if n >= 8 {
                        dns[1] = ip_at(value, 4);
                    }
                }
                (54, 4) => server = ip_at(value, 0),
                (51, 4) => lease = be32(value, 0),
                _ => {}
            }
            i += 2 + len;
        }
        match (self.dhcp.state, kind) {
            // Oferta: se pide esa dirección.
            (DhcpState::Discovering, 2) => {
                self.dhcp.offered = offered;
                self.dhcp.server = server;
                self.dhcp.state = DhcpState::Requesting;
                self.dhcp.attempts = 0;
                self.dhcp.next_try = self.now + 2000;
                self.send_dhcp(3);
            }
            // Confirmación: la interfaz queda configurada.
            (DhcpState::Requesting, 5) | (DhcpState::Bound, 5) => {
                self.config = Some(Config { ip: offered, mask, gateway, dns });
                self.dhcp.state = DhcpState::Bound;
                self.dhcp.renew_at = self.now + (lease as u64).min(7 * 86_400) * 500;
            }
            // Rechazo: vuelta a empezar.
            (DhcpState::Requesting, 6) | (DhcpState::Bound, 6) => {
                self.config = None;
                self.start_dhcp();
            }
            _ => {}
        }
    }

    fn poll_dhcp(&mut self) {
        match self.dhcp.state {
            DhcpState::Off => {}
            DhcpState::Discovering | DhcpState::Requesting if self.now >= self.dhcp.next_try => {
                self.dhcp.attempts += 1;
                // Sin respuesta a la petición: se vuelve a descubrir.
                if self.dhcp.state == DhcpState::Requesting && self.dhcp.attempts > 3 {
                    self.dhcp.state = DhcpState::Discovering;
                    self.dhcp.attempts = 1;
                }
                self.dhcp.next_try = self.now + (1000u64 << self.dhcp.attempts.min(3));
                let kind = if self.dhcp.state == DhcpState::Discovering { 1 } else { 3 };
                self.send_dhcp(kind);
            }
            DhcpState::Bound if self.now >= self.dhcp.renew_at => {
                // Renovación a mitad del plazo; si nadie contesta se reintenta.
                self.dhcp.renew_at = self.now + 60_000;
                self.send_dhcp(3);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ DNS

    /// Empieza a resolver `name` (un registro A). El resultado se recoge con
    /// [`dns_result`](Stack::dns_result).
    pub fn dns_start(&mut self, name: &str) -> Result<Handle, Error> {
        let Some(c) = self.config else { return Err(Error::NotConfigured) };
        if c.dns[0] == [0; 4] {
            return Err(Error::NotConfigured);
        }
        let id = self.rand() as u16;
        let mut query = Vec::with_capacity(18 + name.len());
        query.extend_from_slice(&id.to_be_bytes());
        // Consulta normal con recursión, una pregunta.
        query.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        for label in name.trim_end_matches('.').split('.') {
            if label.is_empty() || label.len() > 63 {
                return Err(Error::NotFound);
            }
            query.push(label.len() as u8);
            query.extend_from_slice(label.as_bytes());
        }
        // Fin del nombre, tipo A, clase IN.
        query.extend_from_slice(&[0, 0, 1, 0, 1]);
        let port = self.ephemeral_port(false);
        Ok(self.insert(Socket::Dns(Dns { id, port, query, attempts: 0, next_try: self.now, state: DnsState::Waiting })))
    }

    /// `None` mientras no haya respuesta. Tras un `Some`, el identificador
    /// queda liberado.
    pub fn dns_result(&mut self, handle: Handle) -> Option<Result<Vec<Ip>, Error>> {
        match self.sockets.get(handle) {
            Some(Some(Socket::Dns(Dns { state: DnsState::Done(_), .. }))) => {}
            Some(Some(Socket::Dns(_))) => return None,
            _ => return Some(Err(Error::InvalidHandle)),
        }
        match self.sockets[handle].take() {
            Some(Socket::Dns(Dns { state: DnsState::Done(result), .. })) => Some(result),
            _ => None,
        }
    }

    fn poll_dns(&mut self) {
        for index in 0..self.sockets.len() {
            let Some(Socket::Dns(d)) = &mut self.sockets[index] else { continue };
            if !matches!(d.state, DnsState::Waiting) || self.now < d.next_try {
                continue;
            }
            if d.attempts >= 4 {
                d.state = DnsState::Done(Err(Error::TimedOut));
                continue;
            }
            d.attempts += 1;
            d.next_try = self.now + 2000;
            let (port, query, attempt) = (d.port, d.query.clone(), d.attempts);
            let Some(c) = self.config else { continue };
            // Los reintentos alternan entre los dos servidores.
            let server = if attempt % 2 == 0 && c.dns[1] != [0; 4] { c.dns[1] } else { c.dns[0] };
            self.send_udp(port, server, 53, &query);
        }
    }

    // ------------------------------------------------------------ TCP

    fn tcp(&mut self, handle: Handle) -> Result<&mut Tcp, Error> {
        match self.sockets.get_mut(handle) {
            Some(Some(Socket::Tcp(t))) => Ok(t),
            _ => Err(Error::InvalidHandle),
        }
    }

    fn new_tcp(&mut self, state: TcpState, local_port: u16, remote: (Ip, u16)) -> Tcp {
        let iss = self.rand();
        Tcp {
            state,
            local_port,
            remote,
            snd_una: iss,
            snd_nxt: iss,
            snd_wnd: TCP_MSS as u32,
            rcv_nxt: 0,
            send_buf: VecDeque::new(),
            recv_buf: VecDeque::new(),
            mss: 536,
            fin_queued: false,
            fin_sent: false,
            peer_fin: false,
            advertised: TCP_RECV_BUFFER,
            retransmit_at: None,
            retries: 0,
            error: None,
            parent: None,
            orphan_until: None,
        }
    }

    /// Monta y envía un segmento.
    fn send_segment(&mut self, local_port: u16, remote: (Ip, u16), seq: u32, ack: u32, flags: u8, window: usize, data: &[u8]) {
        let src = self.source_for(remote.0);
        let options: &[u8] = if flags & SYN != 0 { &[2, 4, (TCP_MSS >> 8) as u8, TCP_MSS as u8] } else { &[] };
        let header = 20 + options.len();
        let mut p = Vec::with_capacity(header + data.len());
        p.extend_from_slice(&local_port.to_be_bytes());
        p.extend_from_slice(&remote.1.to_be_bytes());
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&ack.to_be_bytes());
        p.extend_from_slice(&[(header as u8 / 4) << 4, flags]);
        p.extend_from_slice(&(window.min(65535) as u16).to_be_bytes());
        p.extend_from_slice(&[0, 0, 0, 0]);
        p.extend_from_slice(options);
        p.extend_from_slice(data);
        let length = p.len() as u16;
        let d = remote.0;
        let pseudo = [src[0], src[1], src[2], src[3], d[0], d[1], d[2], d[3], 0, PROTO_TCP, (length >> 8) as u8, length as u8];
        let sum = checksum(&[&pseudo, &p]);
        p[16..18].copy_from_slice(&sum.to_be_bytes());
        self.send_ip(src, d, PROTO_TCP, &p);
    }

    /// Envía lo que la ventana permita del socket `handle`: SYN, datos
    /// nuevos, FIN, o una retransmisión desde el primer byte sin confirmar.
    fn tcp_output(&mut self, handle: Handle, retransmit: bool) {
        let now = self.now;
        let Ok(t) = self.tcp(handle) else { return };
        let (port, remote) = (t.local_port, t.remote);
        let window = TCP_RECV_BUFFER - t.recv_buf.len();
        t.advertised = window;
        match t.state {
            TcpState::Closed => return,
            TcpState::SynSent | TcpState::SynReceived => {
                if retransmit || t.snd_nxt == t.snd_una {
                    let (seq, ack) = (t.snd_una, t.rcv_nxt);
                    let flags = if t.state == TcpState::SynSent { SYN } else { SYN | ACK };
                    t.snd_nxt = t.snd_una.wrapping_add(1);
                    t.retransmit_at = Some(now + (TCP_RTO_MS << t.retries.min(4)));
                    self.send_segment(port, remote, seq, ack, flags, window, &[]);
                }
                return;
            }
            _ => {}
        }
        if retransmit {
            // Se vuelve a enviar desde el principio de lo no confirmado.
            t.snd_nxt = t.snd_una;
            t.fin_sent = false;
        }
        let mut segments: Vec<(u32, u8, Vec<u8>)> = Vec::new();
        loop {
            let sent = t.snd_nxt.wrapping_sub(t.snd_una) as usize;
            let unsent = t.send_buf.len().saturating_sub(sent);
            // Con la ventana a cero se sondea con un byte.
            let room = (t.snd_wnd as usize).max(1).saturating_sub(sent);
            let size = unsent.min(room).min(t.mss);
            // El FIN va siempre en un segmento propio, sin datos: es válido
            // juntarlos, pero hay traductores de red (el de PCSX2) que lo rechazan.
            let fin_now = t.fin_queued && !t.fin_sent && unsent == 0 && room > 0;
            if size == 0 && !fin_now {
                break;
            }
            let data: Vec<u8> = t.send_buf.iter().skip(sent).take(size).copied().collect();
            let mut flags = ACK;
            if size > 0 && unsent == size {
                flags |= PSH;
            }
            if fin_now {
                flags |= FIN;
                t.fin_sent = true;
            }
            segments.push((t.snd_nxt, flags, data));
            t.snd_nxt = t.snd_nxt.wrapping_add(size as u32 + fin_now as u32);
            if retransmit {
                // En una retransmisión, un solo segmento.
                break;
            }
        }
        if !segments.is_empty() && (retransmit || t.retransmit_at.is_none()) {
            t.retransmit_at = Some(now + (TCP_RTO_MS << t.retries.min(4)));
        }
        let ack = t.rcv_nxt;
        for (seq, flags, data) in segments {
            self.send_segment(port, remote, seq, ack, flags, window, &data);
        }
    }

    fn send_ack(&mut self, handle: Handle) {
        let Ok(t) = self.tcp(handle) else { return };
        let window = TCP_RECV_BUFFER - t.recv_buf.len();
        t.advertised = window;
        let (port, remote, seq, ack) = (t.local_port, t.remote, t.snd_nxt, t.rcv_nxt);
        self.send_segment(port, remote, seq, ack, ACK, window, &[]);
    }

    fn handle_tcp(&mut self, src: Ip, dst: Ip, p: &[u8]) {
        if p.len() < 20 {
            return;
        }
        let length = p.len() as u16;
        let pseudo = [src[0], src[1], src[2], src[3], dst[0], dst[1], dst[2], dst[3], 0, PROTO_TCP, (length >> 8) as u8, length as u8];
        if checksum(&[&pseudo, p]) != 0 {
            return;
        }
        let (src_port, dst_port) = (be16(p, 0), be16(p, 2));
        let (seq, ack, flags) = (be32(p, 4), be32(p, 8), p[13]);
        let window = be16(p, 14) as u32;
        let header = ((p[12] >> 4) as usize) * 4;
        if header < 20 || header > p.len() {
            return;
        }
        let data = &p[header..];
        // Opción MSS del SYN.
        let mut peer_mss = 536usize;
        let mut i = 20;
        while i + 1 < header {
            match p[i] {
                0 => break,
                1 => i += 1,
                2 if p[i + 1] == 4 && i + 3 < header => {
                    peer_mss = be16(p, i + 2) as usize;
                    i += 4;
                }
                _ => i += (p[i + 1] as usize).max(2),
            }
        }

        let connection = self.sockets.iter().position(|s| matches!(s, Some(Socket::Tcp(t)) if t.local_port == dst_port && t.remote == (src, src_port) && t.state != TcpState::Closed));
        let Some(handle) = connection else {
            // Conexión ya cerrada con orden que aún recordamos: el otro extremo
            // repite algo porque se perdió nuestra confirmación. Se repite.
            let closed = self.sockets.iter().position(|s| matches!(s, Some(Socket::Tcp(t)) if t.local_port == dst_port && t.remote == (src, src_port) && t.error.is_none() && t.peer_fin));
            if let Some(handle) = closed {
                if flags & RST == 0 {
                    self.send_ack(handle);
                }
                return;
            }
            let listener = self.sockets.iter().position(|s| matches!(s, Some(Socket::Listener(l)) if l.port == dst_port));
            match listener {
                // Conexión entrante.
                Some(parent) if flags & (SYN | ACK | RST) == SYN => {
                    let mut t = self.new_tcp(TcpState::SynReceived, dst_port, (src, src_port));
                    t.rcv_nxt = seq.wrapping_add(1);
                    t.mss = peer_mss.clamp(64, TCP_MSS);
                    t.snd_wnd = window;
                    t.parent = Some(parent);
                    let handle = self.insert(Socket::Tcp(t));
                    self.tcp_output(handle, false);
                }
                // Nadie escucha ahí: se rechaza.
                _ if flags & RST == 0 => {
                    let (rseq, rack, rflags) = if flags & ACK != 0 {
                        (ack, 0, RST)
                    } else {
                        (0, seq.wrapping_add(data.len() as u32 + (flags & SYN != 0) as u32 + (flags & FIN != 0) as u32), RST | ACK)
                    };
                    self.send_segment(dst_port, (src, src_port), rseq, rack, rflags, 0, &[]);
                }
                _ => {}
            }
            return;
        };

        let now = self.now;
        let t = match &mut self.sockets[handle] {
            Some(Socket::Tcp(t)) => t,
            _ => return,
        };

        if t.state == TcpState::SynSent {
            if flags & ACK != 0 && ack != t.snd_nxt {
                return;
            }
            if flags & RST != 0 {
                if flags & ACK != 0 {
                    t.state = TcpState::Closed;
                    t.error = Some(Error::ConnectionRefused);
                }
                return;
            }
            if flags & (SYN | ACK) == (SYN | ACK) {
                t.rcv_nxt = seq.wrapping_add(1);
                t.snd_una = ack;
                t.snd_wnd = window;
                t.mss = peer_mss.clamp(64, TCP_MSS);
                t.state = TcpState::Established;
                t.retransmit_at = None;
                t.retries = 0;
                self.send_ack(handle);
                self.tcp_output(handle, false);
            }
            return;
        }

        if flags & RST != 0 {
            // Sólo vale si cae dentro de lo que esperamos.
            if seq_le(t.rcv_nxt, seq) && seq_lt(seq, t.rcv_nxt.wrapping_add(TCP_RECV_BUFFER as u32 + 1)) {
                t.error = Some(if t.state == TcpState::SynReceived { Error::ConnectionRefused } else { Error::ConnectionReset });
                t.state = TcpState::Closed;
                t.retransmit_at = None;
            }
            return;
        }
        if flags & SYN != 0 {
            // SYN repetido: se repite nuestra respuesta.
            if t.state == TcpState::SynReceived {
                self.tcp_output(handle, true);
            } else {
                self.send_ack(handle);
            }
            return;
        }
        if flags & ACK == 0 {
            return;
        }

        // Confirmaciones.
        let mut became_established = None;
        if seq_lt(t.snd_una, ack) && seq_le(ack, t.snd_nxt) {
            let mut acked = ack.wrapping_sub(t.snd_una) as usize;
            if t.state == TcpState::SynReceived {
                // Confirma nuestro SYN.
                acked -= 1;
                t.state = TcpState::Established;
                became_established = t.parent.take();
            }
            let from_buffer = acked.min(t.send_buf.len());
            t.send_buf.drain(..from_buffer);
            // Lo que sobra es la confirmación de nuestro FIN.
            if acked > from_buffer && t.fin_sent {
                match t.state {
                    TcpState::FinWait => {}
                    TcpState::Closing => t.state = TcpState::Closed,
                    _ => {}
                }
                t.fin_queued = false;
            }
            t.snd_una = ack;
            t.retries = 0;
            t.retransmit_at = if t.snd_una == t.snd_nxt { None } else { Some(now + TCP_RTO_MS) };
        }
        if seq_le(t.snd_una, ack) {
            t.snd_wnd = window;
        }
        let fin_acked = t.fin_sent && !t.fin_queued;

        // Datos y FIN, sólo si llegan en orden.
        let mut need_ack = false;
        let can_receive = matches!(t.state, TcpState::Established | TcpState::FinWait);
        if !data.is_empty() || flags & FIN != 0 {
            need_ack = true;
            if seq == t.rcv_nxt && can_receive {
                let room = TCP_RECV_BUFFER - t.recv_buf.len();
                let take = data.len().min(room);
                t.recv_buf.extend(&data[..take]);
                t.rcv_nxt = t.rcv_nxt.wrapping_add(take as u32);
                if flags & FIN != 0 && take == data.len() {
                    t.rcv_nxt = t.rcv_nxt.wrapping_add(1);
                    t.peer_fin = true;
                    t.state = match t.state {
                        TcpState::Established => TcpState::CloseWait,
                        // Ya habíamos cerrado nosotros.
                        _ if fin_acked => TcpState::Closed,
                        _ => TcpState::Closing,
                    };
                }
            }
        }
        if t.state == TcpState::Closed {
            t.retransmit_at = None;
        }
        if let Some(parent) = became_established {
            match &mut self.sockets[parent] {
                Some(Socket::Listener(l)) => l.ready.push_back(handle),
                // El socket de escucha ya no existe: se corta.
                _ => {
                    let _ = self.tcp_abort(handle);
                    return;
                }
            }
        }
        if need_ack {
            self.send_ack(handle);
        }
        self.tcp_output(handle, false);
    }

    /// Empieza a conectar con `dst`. El estado se consulta con
    /// [`tcp_state`](Stack::tcp_state).
    pub fn tcp_connect(&mut self, dst: Ip, dst_port: u16) -> Result<Handle, Error> {
        if self.config.is_none() && dst != LOOPBACK {
            return Err(Error::NotConfigured);
        }
        let port = self.ephemeral_port(true);
        let t = self.new_tcp(TcpState::SynSent, port, (dst, dst_port));
        let handle = self.insert(Socket::Tcp(t));
        self.tcp_output(handle, false);
        Ok(handle)
    }

    pub fn tcp_listen(&mut self, port: u16) -> Result<Handle, Error> {
        let port = if port == 0 {
            self.ephemeral_port(true)
        } else if self.port_in_use(port, true) {
            return Err(Error::AddrInUse);
        } else {
            port
        };
        Ok(self.insert(Socket::Listener(Listener { port, ready: VecDeque::new() })))
    }

    /// Siguiente conexión entrante ya establecida, si la hay.
    pub fn tcp_accept(&mut self, listener: Handle) -> Result<Option<Handle>, Error> {
        match self.sockets.get_mut(listener) {
            Some(Some(Socket::Listener(l))) => Ok(l.ready.pop_front()),
            _ => Err(Error::InvalidHandle),
        }
    }

    /// Estado de la conexión y, si se cortó, por qué.
    pub fn tcp_state(&mut self, handle: Handle) -> Result<(TcpState, Option<Error>), Error> {
        let t = self.tcp(handle)?;
        Ok((t.state, t.error))
    }

    pub fn tcp_remote(&mut self, handle: Handle) -> Result<(Ip, u16), Error> {
        Ok(self.tcp(handle)?.remote)
    }

    /// Encola datos para enviar. Devuelve cuántos bytes cupieron (0 = el
    /// búfer está lleno, hay que esperar).
    pub fn tcp_send(&mut self, handle: Handle, data: &[u8]) -> Result<usize, Error> {
        let t = self.tcp(handle)?;
        if let Some(e) = t.error {
            return Err(e);
        }
        if !matches!(t.state, TcpState::Established | TcpState::CloseWait) || t.fin_queued {
            return Err(Error::NotConnected);
        }
        let take = data.len().min(TCP_SEND_BUFFER - t.send_buf.len());
        t.send_buf.extend(&data[..take]);
        self.tcp_output(handle, false);
        Ok(take)
    }

    /// Lee datos recibidos. `Ok(None)` = aún no hay nada; `Ok(Some(0))` = el
    /// otro extremo cerró y no queda nada por leer.
    pub fn tcp_recv(&mut self, handle: Handle, buffer: &mut [u8], peek: bool) -> Result<Option<usize>, Error> {
        let t = self.tcp(handle)?;
        if t.recv_buf.is_empty() {
            if t.peer_fin {
                return Ok(Some(0));
            }
            if let Some(e) = t.error {
                return Err(e);
            }
            return match t.state {
                TcpState::Closed => Ok(Some(0)),
                TcpState::SynSent | TcpState::SynReceived => Err(Error::NotConnected),
                _ => Ok(None),
            };
        }
        let count = buffer.len().min(t.recv_buf.len());
        for (slot, byte) in buffer.iter_mut().zip(t.recv_buf.iter()) {
            *slot = *byte;
        }
        if !peek {
            t.recv_buf.drain(..count);
            // Si la ventana anunciada era pequeña y ha vuelto a abrirse, se avisa.
            let window = TCP_RECV_BUFFER - t.recv_buf.len();
            if t.advertised < TCP_MSS && window >= TCP_MSS && t.state != TcpState::Closed {
                self.send_ack(handle);
            }
        }
        Ok(Some(count))
    }

    /// Cierra nuestro lado de la conexión (se envía FIN tras los datos
    /// pendientes). Se puede seguir leyendo.
    pub fn tcp_shutdown_write(&mut self, handle: Handle) -> Result<(), Error> {
        let t = self.tcp(handle)?;
        match t.state {
            TcpState::Established => t.state = TcpState::FinWait,
            TcpState::CloseWait => t.state = TcpState::Closing,
            TcpState::SynSent | TcpState::SynReceived => return Err(Error::NotConnected),
            _ => return Ok(()),
        }
        t.fin_queued = true;
        self.tcp_output(handle, false);
        Ok(())
    }

    /// Corta la conexión con un RST y libera el socket.
    pub fn tcp_abort(&mut self, handle: Handle) -> Result<(), Error> {
        let t = self.tcp(handle)?;
        let (state, port, remote, seq, ack) = (t.state, t.local_port, t.remote, t.snd_nxt, t.rcv_nxt);
        self.sockets[handle] = None;
        if !matches!(state, TcpState::Closed | TcpState::SynSent) {
            self.send_segment(port, remote, seq, ack, RST | ACK, 0, &[]);
        }
        Ok(())
    }

    /// La aplicación ya no usa el socket. Una conexión TCP abierta se cierra
    /// con orden por su cuenta (hasta 5 s) antes de liberarse.
    pub fn release(&mut self, handle: Handle) {
        let now = self.now;
        match self.sockets.get_mut(handle) {
            Some(Some(Socket::Tcp(t))) => {
                if t.state == TcpState::Closed && t.error.is_none() && t.peer_fin {
                    // Cerrada con orden: se recuerda un rato por si el otro
                    // extremo repite su cierre.
                    t.orphan_until = Some(now + 2000);
                } else if matches!(t.state, TcpState::Closed | TcpState::SynSent | TcpState::SynReceived) || !t.recv_buf.is_empty() && !t.peer_fin {
                    // Sin conexión, o con datos sin leer: no hay cierre limpio.
                    let _ = self.tcp_abort(handle);
                } else {
                    t.orphan_until = Some(now + 5000);
                    let _ = self.tcp_shutdown_write(handle);
                }
            }
            Some(Some(Socket::Listener(l))) => {
                let pending: Vec<Handle> = l.ready.drain(..).collect();
                self.sockets[handle] = None;
                for h in pending {
                    let _ = self.tcp_abort(h);
                }
            }
            Some(slot) => *slot = None,
            None => {}
        }
    }

    fn poll_tcp(&mut self) {
        let now = self.now;
        for handle in 0..self.sockets.len() {
            let Some(Socket::Tcp(t)) = &mut self.sockets[handle] else { continue };
            if let Some(deadline) = t.orphan_until {
                // Una conexión cerrada con orden se recuerda hasta el plazo;
                // una que no llegó a cerrarse se corta al cumplirlo.
                let forget = if t.state == TcpState::Closed { now >= deadline || t.error.is_some() } else { false };
                if forget {
                    self.sockets[handle] = None;
                    continue;
                }
                if now >= deadline {
                    let _ = self.tcp_abort(handle);
                    continue;
                }
            }
            let Some(at) = t.retransmit_at else { continue };
            if now < at {
                continue;
            }
            // Con la ventana del otro extremo cerrada no hay nada que dar por
            // perdido: se le sondea sin límite hasta que vuelva a abrirla.
            let persisting = t.snd_wnd == 0 && !matches!(t.state, TcpState::SynSent | TcpState::SynReceived);
            t.retries = if persisting { (t.retries + 1).min(3) } else { t.retries + 1 };
            if t.retries > TCP_MAX_RETRIES {
                t.error = Some(Error::TimedOut);
                t.state = TcpState::Closed;
                t.retransmit_at = None;
                continue;
            }
            self.tcp_output(handle, true);
        }
    }

    // ------------------------------------------------------------ bucle

    /// Procesa lo recibido, atiende los temporizadores y envía lo pendiente.
    /// `now` son milisegundos de un reloj que no retrocede.
    pub fn poll(&mut self, now: u64) {
        self.now = now;
        // Tramas del adaptador.
        for _ in 0..64 {
            let Some(frame) = self.device.receive() else { break };
            if frame.len() < 14 {
                continue;
            }
            match be16(&frame, 12) {
                ETH_ARP => self.handle_arp(&frame[14..]),
                ETH_IP => self.handle_ip(&frame[14..]),
                _ => {}
            }
        }
        // Paquetes a nosotros mismos (pueden generar más).
        for _ in 0..256 {
            let Some(packet) = self.loopback.pop_front() else { break };
            self.handle_ip(&packet);
        }
        // ARP: reintentos y caducidades.
        self.arp.retain(|e| e.2 > now);
        self.waiting_arp.retain(|w| w.2 > now);
        let mut retry = Vec::new();
        for request in &mut self.arp_requests {
            if now >= request.1 {
                request.1 = now + 1000;
                retry.push(request.0);
            }
        }
        self.arp_requests.retain(|r| self.waiting_arp.iter().any(|w| w.0 == r.0));
        for ip in retry {
            if self.arp_requests.iter().any(|r| r.0 == ip) {
                self.send_arp(1, [0; 6], ip);
            }
        }
        self.poll_dhcp();
        self.poll_dns();
        self.poll_tcp();
    }
}

fn alloc_zeroed(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len + 64);
    v.resize(len, 0);
    v
}

/// Interpreta una respuesta de DNS para la consulta `id`. `None` si el
/// mensaje no es esa respuesta.
fn parse_dns(id: u16, p: &[u8]) -> Option<Result<Vec<Ip>, Error>> {
    if p.len() < 12 || be16(p, 0) != id || p[2] & 0x80 == 0 {
        return None;
    }
    match p[3] & 15 {
        0 => {}
        3 => return Some(Err(Error::NotFound)),
        _ => return Some(Err(Error::Protocol)),
    }
    let (questions, answers) = (be16(p, 4), be16(p, 6));
    // Salta un nombre (con o sin compresión) y devuelve dónde acaba.
    let skip_name = |mut i: usize| -> Option<usize> {
        loop {
            let len = *p.get(i)? as usize;
            if len & 0xc0 == 0xc0 {
                return Some(i + 2);
            }
            if len == 0 {
                return Some(i + 1);
            }
            i += 1 + len;
        }
    };
    let mut i = 12;
    for _ in 0..questions {
        match skip_name(i) {
            Some(end) => i = end + 4,
            None => return Some(Err(Error::Protocol)),
        }
    }
    let mut addresses = Vec::new();
    for _ in 0..answers {
        let Some(end) = skip_name(i) else { return Some(Err(Error::Protocol)) };
        if end + 10 > p.len() {
            return Some(Err(Error::Protocol));
        }
        let (kind, class, length) = (be16(p, end), be16(p, end + 2), be16(p, end + 8) as usize);
        let data = end + 10;
        if data + length > p.len() {
            return Some(Err(Error::Protocol));
        }
        if kind == 1 && class == 1 && length == 4 {
            addresses.push(ip_at(p, data));
        }
        i = data + length;
    }
    Some(if addresses.is_empty() { Err(Error::NotFound) } else { Ok(addresses) })
}

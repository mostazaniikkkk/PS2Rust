//! Batería de pruebas de `std::net`.
//!
//! Este mismo archivo se compila para la PlayStation 2 y para el PC, y las
//! dos ejecuciones hablan con el mismo servidor de pruebas
//! (`toolchain/selftest-host --net-server`): la PS2 con su propia pila TCP/IP
//! a través de la red del emulador, el PC con la del sistema operativo. Las
//! dos salidas deben coincidir.

use std::fmt::Write as _;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

/// Dónde está cada servicio del servidor de pruebas (`net-test.cfg`).
pub struct Config {
    pub host: Ipv4Addr,
    values: Vec<(String, String)>,
}

impl Config {
    pub fn parse(text: &str) -> Option<Config> {
        let values: Vec<(String, String)> = text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
        let host = values.iter().find(|(k, _)| k == "host")?.1.parse().ok()?;
        Some(Config { host, values })
    }

    pub fn port(&self, name: &str) -> u16 {
        self.values.iter().find(|(k, _)| k == name).and_then(|(_, v)| v.parse().ok()).unwrap_or(0)
    }

    pub fn address(&self, name: &str) -> SocketAddr {
        SocketAddr::from((self.host, self.port(name)))
    }
}

pub struct T {
    pub out: String,
    pub total: u32,
    pub failed: u32,
}

impl T {
    pub fn section(&mut self, name: &str) {
        let _ = writeln!(self.out, "== {name}");
    }

    pub fn check(&mut self, name: &str, ok: bool) {
        self.total += 1;
        if !ok {
            self.failed += 1;
        }
        let _ = writeln!(self.out, "[{}] {}", if ok { "ok" } else { "FALLO" }, name);
    }

    /// Comprueba y, si falla, deja escrito lo que se obtuvo.
    pub fn expect<V: std::fmt::Debug>(&mut self, name: &str, ok: bool, got: V) {
        self.check(name, ok);
        if !ok {
            let _ = writeln!(self.out, "     se obtuvo {got:?}");
        }
    }
}

/// Un plazo vencido: `TimedOut` (PS2, Windows) o `WouldBlock` (Unix).
fn timed_out<V>(result: &std::io::Result<V>) -> bool {
    matches!(result, Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock))
}

fn would_block<V>(result: &std::io::Result<V>) -> bool {
    matches!(result, Err(e) if e.kind() == ErrorKind::WouldBlock)
}

fn pattern(len: usize, salt: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + salt * 7) % 251) as u8).collect()
}

pub fn run(config: &Config) -> (String, u32) {
    let mut t = T { out: String::new(), total: 0, failed: 0 };
    tcp_client(&mut t, config);
    tcp_long_lived(&mut t, config);
    tcp_closing(&mut t, config);
    tcp_failures(&mut t, config);
    udp(&mut t, config);
    loopback(&mut t);
    names(&mut t);
    let (total, failed) = (t.total, t.failed);
    let _ = writeln!(t.out, "== resultado: {total} comprobaciones, {failed} fallos");
    (t.out, failed)
}

fn tcp_client(t: &mut T, config: &Config) {
    t.section("TCP: cliente");
    let server = config.address("echo");
    let stream = TcpStream::connect(server);
    t.expect("connect", stream.is_ok(), &stream);
    let Ok(mut stream) = stream else { return };
    t.expect("peer_addr es el servidor", stream.peer_addr().ok() == Some(server), stream.peer_addr());
    t.check("local_addr tiene puerto", stream.local_addr().map_or(false, |a| a.port() != 0));
    let mut four = [0u8; 4];
    let echoed = stream.write_all(b"hola").and_then(|_| stream.read_exact(&mut four));
    t.expect("enviar y recibir", echoed.is_ok() && &four == b"hola", (&echoed, four));
    t.check("set_nodelay", stream.set_nodelay(true).is_ok() && stream.nodelay().ok() == Some(true));
    // peek mira sin consumir.
    let mut three = [0u8; 3];
    let _ = stream.write_all(b"abc");
    let peeked = stream.peek(&mut three);
    t.expect("peek", peeked.ok() == Some(3) && &three == b"abc", three);
    three = [0; 3];
    t.check("read tras peek devuelve lo mismo", stream.read_exact(&mut three).is_ok() && &three == b"abc");
    // 200 KiB de ida y vuelta en trozos de 4 KiB.
    let mut intact = true;
    for round in 0..50 {
        let data = pattern(4096, round);
        let mut back = vec![0u8; 4096];
        intact &= stream.write_all(&data).is_ok() && stream.read_exact(&mut back).is_ok() && back == data;
        if !intact {
            break;
        }
    }
    t.check("200 KiB de ida y vuelta sin un byte cambiado", intact);
    // Un trozo mayor que los búferes de la pila, leyendo mientras se escribe.
    let big = pattern(64 * 1024, 99);
    let reader = stream.try_clone().map(|mut r| {
        thread::spawn(move || {
            let mut back = vec![0u8; 64 * 1024];
            r.read_exact(&mut back).map(|_| back)
        })
    });
    let wrote = stream.write_all(&big);
    let back = reader.ok().and_then(|h| h.join().ok()).and_then(|r| r.ok());
    t.expect("64 KiB con un hilo que lee y otro que escribe", wrote.is_ok() && back.as_deref() == Some(&big[..]), &wrote);
    // try_clone comparte la conexión.
    let clone = stream.try_clone();
    let shared = clone.and_then(|mut c| c.write_all(b"clon")).and_then(|_| stream.read_exact(&mut four));
    t.expect("try_clone escribe por la misma conexión", shared.is_ok() && &four == b"clon", &shared);
}

fn tcp_long_lived(t: &mut T, config: &Config) {
    t.section("TCP: plazos y conexiones largas");
    let Ok(mut stream) = TcpStream::connect(config.address("echo")) else {
        t.check("connect", false);
        return;
    };
    t.check("un plazo de 0 no vale", stream.set_read_timeout(Some(Duration::ZERO)).is_err());
    t.check("set_read_timeout", stream.set_read_timeout(Some(Duration::from_millis(150))).is_ok() && stream.read_timeout().ok() == Some(Some(Duration::from_millis(150))));
    let mut buffer = [0u8; 16];
    let start = Instant::now();
    let idle = stream.read(&mut buffer);
    let waited = start.elapsed();
    t.expect("leer sin datos vence el plazo", timed_out(&idle), &idle);
    t.expect("y vence a su hora", waited >= Duration::from_millis(100) && waited < Duration::from_secs(3), waited);
    // Tras vencer, la conexión sigue viva.
    let alive = stream.write_all(b"sigo").and_then(|_| stream.read_exact(&mut buffer[..4]));
    t.expect("la conexión sigue viva tras el plazo", alive.is_ok() && &buffer[..4] == b"sigo", &alive);
    // Latidos: un mensaje cada 50 ms durante medio segundo, con plazo de lectura.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut beats = 0;
    for i in 0..10u8 {
        let message = [b'l', b'a', b't', i];
        if stream.write_all(&message).is_ok() && stream.read_exact(&mut buffer[..4]).is_ok() && buffer[..4] == message {
            beats += 1;
        }
        thread::sleep(Duration::from_millis(50));
    }
    t.expect("10 latidos con respuesta", beats == 10, beats);
    // Sin bloqueo.
    t.check("set_nonblocking", stream.set_nonblocking(true).is_ok());
    let nothing = stream.read(&mut buffer);
    t.expect("leer sin datos y sin bloqueo: WouldBlock", would_block(&nothing), &nothing);
    let _ = stream.write_all(b"x");
    let mut got = None;
    for _ in 0..300 {
        match stream.read(&mut buffer) {
            Ok(n) => {
                got = Some(buffer[..n].to_vec());
                break;
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(10)),
            Err(_) => break,
        }
    }
    t.expect("sondeando llega la respuesta", got.as_deref() == Some(b"x"), &got);
    let _ = stream.set_nonblocking(false);
    t.check("set_read_timeout(None) quita el plazo", stream.set_read_timeout(None).is_ok() && stream.read_timeout().ok() == Some(None));
    // Un servidor que no dice nada: los plazos se repiten sin romper nada.
    if let Ok(mut quiet) = TcpStream::connect(config.address("silent")) {
        let _ = quiet.set_read_timeout(Some(Duration::from_millis(100)));
        let first = quiet.read(&mut buffer);
        let second = quiet.read(&mut buffer);
        t.expect("dos plazos seguidos en una conexión callada", timed_out(&first) && timed_out(&second), (&first, &second));
        t.check("y se puede seguir escribiendo", quiet.write_all(b"hay alguien?").is_ok());
    } else {
        t.check("conectar con el servidor callado", false);
    }
}

fn tcp_closing(t: &mut T, config: &Config) {
    t.section("TCP: cierres");
    let mut text = String::new();
    let hello = TcpStream::connect(config.address("hello")).and_then(|mut s| s.read_to_string(&mut text));
    t.expect("el servidor envía y cierra: read_to_string", hello.is_ok() && text == "hola desde el servidor\n", (&hello, &text));
    // Enviar 100 000 bytes, cerrar la escritura y leer el recuento del servidor.
    let data = pattern(100_000, 5);
    let sum = data.iter().fold(0u32, |s, &b| s.wrapping_add(b as u32));
    let mut reply = String::new();
    let counted = TcpStream::connect(config.address("count")).and_then(|mut s| {
        s.write_all(&data)?;
        s.shutdown(Shutdown::Write)?;
        s.read_to_string(&mut reply)
    });
    t.expect("shutdown(Write) y respuesta con el recuento", counted.is_ok() && reply == format!("100000 {sum}\n"), (&counted, &reply));
    if let Ok(mut stream) = TcpStream::connect(config.address("echo")) {
        let _ = stream.write_all(b"fin");
        let mut buffer = [0u8; 8];
        let _ = stream.read_exact(&mut buffer[..3]);
        t.check("shutdown(Write)", stream.shutdown(Shutdown::Write).is_ok());
        let end = stream.read(&mut buffer);
        t.expect("el servidor cierra: read devuelve 0", end.as_ref().ok() == Some(&0), &end);
        t.check("y sigue devolviendo 0", stream.read(&mut buffer).ok() == Some(0));
        t.check("escribir tras cerrar la escritura falla", stream.write_all(b"tarde").is_err());
    } else {
        t.check("connect", false);
    }
    // Soltar el socket entrega lo pendiente: el servidor cuenta todo.
    let mut reply = String::new();
    let dropped = TcpStream::connect(config.address("count")).and_then(|mut s| {
        s.write_all(&data[..30_000])?;
        let mut reader = s.try_clone()?;
        s.shutdown(Shutdown::Write)?;
        drop(s);
        reader.read_to_string(&mut reply)
    });
    t.expect("30 000 bytes contados tras soltar el socket", dropped.is_ok() && reply.starts_with("30000 "), (&dropped, &reply));
}

fn tcp_failures(t: &mut T, config: &Config) {
    t.section("TCP: fallos");
    // Un puerto sin servidor. El rechazo (ConnectionRefused) se comprueba más
    // abajo, en el bucle local: la red de PCSX2 no devuelve el rechazo del PC
    // y la conexión simplemente no llega a abrirse.
    let refused = TcpStream::connect_timeout(&config.address("closed"), Duration::from_secs(3));
    t.expect("puerto cerrado: la conexión falla", refused.is_err(), &refused);
    // Una dirección que no contesta: connect_timeout corta la espera.
    let start = Instant::now();
    let nowhere = TcpStream::connect_timeout(&SocketAddr::from(([10, 255, 255, 1], 9)), Duration::from_millis(400));
    let waited = start.elapsed();
    t.expect("connect_timeout a una dirección muda falla", nowhere.is_err(), &nowhere);
    t.expect("y no espera mucho más de lo pedido", waited < Duration::from_secs(5), waited);
    t.check("connect_timeout con plazo 0 no vale", TcpStream::connect_timeout(&config.address("echo"), Duration::ZERO).is_err());
    t.check("conectar por IPv6 falla", TcpStream::connect("[::1]:1").is_err());
}

fn udp(t: &mut T, config: &Config) {
    t.section("UDP");
    let server = config.address("udp");
    let socket = UdpSocket::bind("0.0.0.0:0");
    t.expect("bind", socket.is_ok(), &socket);
    let Ok(socket) = socket else { return };
    t.check("local_addr tiene puerto", socket.local_addr().map_or(false, |a| a.port() != 0));
    let _ = socket.set_read_timeout(Some(Duration::from_secs(3)));
    let mut buffer = [0u8; 2048];
    let sent = socket.send_to(b"datagrama", server);
    t.expect("send_to", sent.as_ref().ok() == Some(&9), &sent);
    let received = socket.recv_from(&mut buffer);
    t.expect("recv_from: datos y remitente", matches!(&received, Ok((9, from)) if *from == server) && &buffer[..9] == b"datagrama", &received);
    // Cinco seguidos, que vuelven en orden.
    let mut in_order = true;
    for i in 0..5u8 {
        in_order &= socket.send_to(&[i; 20], server).is_ok();
    }
    for i in 0..5u8 {
        in_order &= matches!(socket.recv_from(&mut buffer), Ok((20, _))) && buffer[..20] == [i; 20];
    }
    t.check("cinco datagramas, en orden", in_order);
    let large = pattern(1400, 3);
    let back = socket.send_to(&large, server).and_then(|_| socket.recv_from(&mut buffer));
    t.expect("un datagrama de 1400 bytes", matches!(back, Ok((1400, _))) && buffer[..1400] == large[..], &back);
    // peek_from no consume.
    let _ = socket.send_to(b"mirar", server);
    let peeked = socket.peek_from(&mut buffer);
    let taken = socket.recv_from(&mut buffer);
    t.expect("peek_from y luego recv_from", matches!(peeked, Ok((5, _))) && matches!(taken, Ok((5, _))) && &buffer[..5] == b"mirar", (&peeked, &taken));
    // Un búfer pequeño recorta el datagrama.
    let _ = socket.send_to(b"0123456789", server);
    let mut small = [0u8; 4];
    let cut = socket.recv_from(&mut small);
    t.expect("un búfer pequeño se queda con el principio", cut.is_ok() && &small == b"0123" || cfg!(windows) && cut.is_err(), &cut);
    // Plazos y modo sin bloqueo.
    let _ = socket.set_read_timeout(Some(Duration::from_millis(150)));
    let idle = socket.recv_from(&mut buffer);
    t.expect("recv_from sin datos vence el plazo", timed_out(&idle), &idle);
    let _ = socket.set_nonblocking(true);
    let nothing = socket.recv_from(&mut buffer);
    t.expect("sin bloqueo: WouldBlock", would_block(&nothing), &nothing);
    let _ = socket.set_nonblocking(false);
    let _ = socket.set_read_timeout(Some(Duration::from_secs(3)));
    // connect fija el destino.
    t.check("connect", socket.connect(server).is_ok() && socket.peer_addr().ok() == Some(server));
    let exchanged = socket.send(b"conectado").and_then(|_| socket.recv(&mut buffer));
    t.expect("send y recv en un socket conectado", exchanged.as_ref().ok() == Some(&9) && &buffer[..9] == b"conectado", &exchanged);
    t.check("set_broadcast", socket.set_broadcast(true).is_ok() && socket.broadcast().ok() == Some(true));
    // Dos sockets distintos tienen puertos distintos.
    let other = UdpSocket::bind("0.0.0.0:0");
    t.check("dos sockets, dos puertos", other.is_ok_and(|o| o.local_addr().ok().map(|a| a.port()) != socket.local_addr().ok().map(|a| a.port())));
    let taken_port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
    t.check("un puerto ocupado no se puede volver a usar", UdpSocket::bind(("0.0.0.0", taken_port)).is_err());
}

fn loopback(t: &mut T) {
    t.section("bucle local: servidor y cliente en la misma máquina");
    let listener = TcpListener::bind("127.0.0.1:0");
    t.expect("TcpListener::bind", listener.is_ok(), &listener);
    let Ok(listener) = listener else { return };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    t.check("local_addr tiene puerto", port != 0);
    let _ = listener.set_nonblocking(true);
    let none = listener.accept();
    t.expect("accept sin clientes y sin bloqueo: WouldBlock", would_block(&none), &none);
    let _ = listener.set_nonblocking(false);
    let client = thread::spawn(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.write_all(b"toc toc")?;
        let mut reply = [0u8; 9];
        stream.read_exact(&mut reply)?;
        Ok::<_, std::io::Error>(reply)
    });
    let accepted = listener.accept();
    t.expect("accept", accepted.is_ok(), &accepted);
    if let Ok((mut stream, peer)) = accepted {
        t.check("el cliente viene de 127.0.0.1", peer.ip() == Ipv4Addr::LOCALHOST);
        let mut knock = [0u8; 7];
        let heard = stream.read_exact(&mut knock);
        t.expect("el servidor recibe", heard.is_ok() && &knock == b"toc toc", &heard);
        let _ = stream.write_all(b"adelante!");
    }
    let reply = client.join().ok().and_then(|r| r.ok());
    t.expect("el cliente recibe la respuesta", reply.as_ref().map(|r| &r[..]) == Some(b"adelante!"), &reply);
    // Tres clientes seguidos en el mismo puerto.
    let mut served = 0;
    for i in 0..3u8 {
        let client = thread::spawn(move || TcpStream::connect(("127.0.0.1", port)).and_then(|mut s| s.write_all(&[i])));
        if let Ok((mut stream, _)) = listener.accept() {
            let mut byte = [0u8; 1];
            if stream.read_exact(&mut byte).is_ok() && byte[0] == i {
                served += 1;
            }
        }
        let _ = client.join();
    }
    t.expect("tres clientes seguidos", served == 3, served);
    drop(listener);
    t.check("sin servidor, la conexión se rechaza", matches!(TcpStream::connect(("127.0.0.1", port)), Err(e) if e.kind() == ErrorKind::ConnectionRefused));
    // UDP entre dos sockets locales.
    let (a, b) = (UdpSocket::bind("127.0.0.1:0"), UdpSocket::bind("127.0.0.1:0"));
    if let (Ok(a), Ok(b)) = (a, b) {
        let _ = b.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buffer = [0u8; 16];
        let target = b.local_addr().unwrap();
        let result = a.send_to(b"local", target).and_then(|_| b.recv_from(&mut buffer));
        t.expect("UDP local", matches!(&result, Ok((5, from)) if Some(*from) == a.local_addr().ok()) && &buffer[..5] == b"local", &result);
    } else {
        t.check("UDP local", false);
    }
}

fn names(t: &mut T) {
    t.section("nombres");
    let localhost: Vec<SocketAddr> = "localhost:80".to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
    t.expect("localhost resuelve a 127.0.0.1", localhost.contains(&SocketAddr::from(([127, 0, 0, 1], 80))), &localhost);
    let literal: Vec<SocketAddr> = "127.0.0.2:1234".to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
    t.check("una dirección escrita no necesita DNS", literal == [SocketAddr::from(([127, 0, 0, 2], 1234))]);
    t.check("sin puerto no es una dirección", "localhost".to_socket_addrs().is_err());
}

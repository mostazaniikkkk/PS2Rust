//! Pruebas en el PC de la pila TCP/IP que lleva la std de la PS2
//! (`toolchain/std-ps2/net/stack.rs`): dos pilas unidas por un cable
//! simulado que puede perder tramas, y un servidor DHCP de mentira.

mod deps {
    pub use std::collections::VecDeque;
    pub use std::vec::Vec;
}

#[path = "../../std-ps2/net/stack.rs"]
#[allow(dead_code)]
mod stack;

use stack::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Un extremo del cable: lo que transmite aparece en la cola del otro.
struct Wire {
    mac: Mac,
    incoming: Rc<RefCell<VecDeque<Vec<u8>>>>,
    outgoing: Rc<RefCell<VecDeque<Vec<u8>>>>,
    /// Pierde, al azar, una de cada `drop_every` tramas transmitidas (0 =
    /// ninguna). El azar es fijo: las pruebas se repiten igual.
    drop_every: usize,
    random: u32,
}

impl Device for Wire {
    fn mac(&self) -> Mac {
        self.mac
    }

    fn transmit(&mut self, frame: &[u8]) {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        if self.drop_every == 0 || self.random as usize % self.drop_every != 0 {
            self.outgoing.borrow_mut().push_back(frame.to_vec());
        }
    }

    fn receive(&mut self) -> Option<Vec<u8>> {
        self.incoming.borrow_mut().pop_front()
    }
}

const A: Ip = [10, 0, 0, 1];
const B: Ip = [10, 0, 0, 2];

fn pair(drop_every: usize) -> (Stack<Wire>, Stack<Wire>) {
    let (ab, ba) = (Rc::new(RefCell::new(VecDeque::new())), Rc::new(RefCell::new(VecDeque::new())));
    let wire = |mac: u8, incoming: &Rc<RefCell<VecDeque<Vec<u8>>>>, outgoing: &Rc<RefCell<VecDeque<Vec<u8>>>>| Wire {
        mac: [2, 0, 0, 0, 0, mac],
        incoming: incoming.clone(),
        outgoing: outgoing.clone(),
        drop_every,
        random: 0x1234_5678 + mac as u32,
    };
    let mut a = Stack::new(wire(1, &ba, &ab));
    let mut b = Stack::new(wire(2, &ab, &ba));
    a.set_config(Config { ip: A, mask: [255, 255, 255, 0], gateway: [0; 4], dns: [[0; 4]; 2] });
    b.set_config(Config { ip: B, mask: [255, 255, 255, 0], gateway: [0; 4], dns: [[0; 4]; 2] });
    (a, b)
}

/// Avanza las dos pilas, 10 ms por paso, hasta que `done` o se agote el tiempo.
fn run(a: &mut Stack<Wire>, b: &mut Stack<Wire>, clock: &mut u64, limit_ms: u64, mut done: impl FnMut(&mut Stack<Wire>, &mut Stack<Wire>) -> bool) -> bool {
    let end = *clock + limit_ms;
    while *clock < end {
        a.poll(*clock);
        b.poll(*clock);
        if done(a, b) {
            return true;
        }
        *clock += 10;
    }
    false
}

#[test]
fn internet_checksum() {
    // Ejemplo de la RFC 1071.
    assert_eq!(checksum(&[&[0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7]]), !0xddf2);
    // Partido en trozos impares da lo mismo.
    assert_eq!(checksum(&[&[0x00, 0x01, 0xf2], &[0x03, 0xf4, 0xf5, 0xf6], &[0xf7]]), !0xddf2);
    assert_eq!(checksum(&[&[0xff]]), !0xff00);
}

#[test]
fn udp_both_ways_after_arp() {
    let (mut a, mut b) = pair(0);
    let mut clock = 0;
    let sa = a.udp_bind(4000).unwrap();
    let sb = b.udp_bind(5000).unwrap();
    assert_eq!(a.udp_bind(4000), Err(Error::AddrInUse));
    a.udp_send(sa, B, 5000, b"hola").unwrap();
    let mut got = None;
    assert!(run(&mut a, &mut b, &mut clock, 1000, |_, b| {
        got = b.udp_recv(sb, false).unwrap();
        got.is_some()
    }));
    assert_eq!(got, Some((A, 4000, b"hola".to_vec())));
    // La respuesta ya no necesita ARP: llega en el mismo paso.
    b.udp_send(sb, A, 4000, &[7; 1472]).unwrap();
    assert!(run(&mut a, &mut b, &mut clock, 100, |a, _| a.udp_recv(sa, true).unwrap().is_some()));
    let (from, port, data) = a.udp_recv(sa, false).unwrap().unwrap();
    assert_eq!((from, port, data.len(), data[1471]), (B, 5000, 1472, 7));
    assert_eq!(a.udp_send(sa, B, 5000, &[0; 1473]), Err(Error::TooLarge));
    // Difusión.
    a.udp_send(sa, [255; 4], 5000, b"todos").unwrap();
    assert!(run(&mut a, &mut b, &mut clock, 100, |_, b| b.udp_recv(sb, true).unwrap().is_some()));
}

/// Transfiere `size` bytes en cada sentido por una conexión TCP y la cierra.
fn tcp_transfer(drop_every: usize, size: usize) {
    let (mut a, mut b) = pair(drop_every);
    let mut clock = 0;
    let listener = b.tcp_listen(80).unwrap();
    let client = a.tcp_connect(B, 80).unwrap();
    let mut server = None;
    assert!(run(&mut a, &mut b, &mut clock, 20_000, |a, b| {
        if server.is_none() {
            server = b.tcp_accept(listener).unwrap();
        }
        server.is_some() && a.tcp_state(client).unwrap().0 == TcpState::Established
    }));
    let server = server.unwrap();
    assert_eq!(b.tcp_remote(server).unwrap().0, A);

    let pattern = |i: usize, salt: usize| ((i * 31 + salt) % 251) as u8;
    let (mut sent_a, mut sent_b) = (0, 0);
    let (mut got_a, mut got_b) = (Vec::new(), Vec::new());
    let mut buffer = [0u8; 3000];
    assert!(run(&mut a, &mut b, &mut clock, 600_000, |a, b| {
        // Cada lado envía su flujo y lee el del otro.
        if sent_a < size {
            let chunk: Vec<u8> = (sent_a..(sent_a + 2000).min(size)).map(|i| pattern(i, 1)).collect();
            sent_a += a.tcp_send(client, &chunk).unwrap();
        }
        if sent_b < size {
            let chunk: Vec<u8> = (sent_b..(sent_b + 2000).min(size)).map(|i| pattern(i, 2)).collect();
            sent_b += b.tcp_send(server, &chunk).unwrap();
        }
        while let Some(n) = a.tcp_recv(client, &mut buffer, false).unwrap() {
            got_a.extend_from_slice(&buffer[..n]);
        }
        while let Some(n) = b.tcp_recv(server, &mut buffer, false).unwrap() {
            got_b.extend_from_slice(&buffer[..n]);
        }
        got_a.len() == size && got_b.len() == size
    }));
    assert!(got_a.iter().enumerate().all(|(i, &v)| v == pattern(i, 2)), "datos de B a A corruptos");
    assert!(got_b.iter().enumerate().all(|(i, &v)| v == pattern(i, 1)), "datos de A a B corruptos");

    // Cierre ordenado: A cierra, B ve el fin, cierra, y los dos acaban cerrados.
    a.tcp_shutdown_write(client).unwrap();
    assert!(run(&mut a, &mut b, &mut clock, 60_000, |_, b| b.tcp_recv(server, &mut buffer, false).unwrap() == Some(0)));
    assert_eq!(b.tcp_state(server).unwrap().0, TcpState::CloseWait);
    // B aún puede enviar por su lado.
    assert_eq!(b.tcp_send(server, b"adios").unwrap(), 5);
    assert!(run(&mut a, &mut b, &mut clock, 60_000, |a, _| a.tcp_recv(client, &mut buffer, true).unwrap() == Some(5)));
    assert_eq!(a.tcp_recv(client, &mut buffer, false).unwrap(), Some(5));
    assert_eq!(&buffer[..5], b"adios");
    b.tcp_shutdown_write(server).unwrap();
    assert!(run(&mut a, &mut b, &mut clock, 60_000, |a, b| {
        a.tcp_state(client).unwrap().0 == TcpState::Closed && b.tcp_state(server).unwrap().0 == TcpState::Closed
    }));
    assert_eq!(a.tcp_recv(client, &mut buffer, false).unwrap(), Some(0));
    assert_eq!(a.tcp_send(client, b"x"), Err(Error::NotConnected));
    assert_eq!((a.tcp_state(client).unwrap().1, b.tcp_state(server).unwrap().1), (None, None));
}

#[test]
fn tcp_transfer_and_orderly_close() {
    tcp_transfer(0, 200_000);
}

#[test]
fn tcp_survives_losing_one_frame_in_seven_at_random() {
    tcp_transfer(7, 60_000);
}

#[test]
fn tcp_survives_losing_one_frame_in_three_at_random() {
    tcp_transfer(3, 8_000);
}

#[test]
fn tcp_waits_for_a_reader_that_takes_its_time() {
    // El receptor no lee durante dos minutos: su ventana se cierra, el emisor
    // la sondea sin rendirse y, cuando por fin lee, llega todo.
    for drop_every in [0, 5] {
        let (mut a, mut b) = pair(drop_every);
        let mut clock = 0;
        let listener = b.tcp_listen(80).unwrap();
        let client = a.tcp_connect(B, 80).unwrap();
        let mut server = None;
        assert!(run(&mut a, &mut b, &mut clock, 20_000, |_, b| {
            server = server.or(b.tcp_accept(listener).unwrap());
            server.is_some()
        }));
        let server = server.unwrap();
        let data: Vec<u8> = (0..100_000usize).map(|i| (i % 241) as u8).collect();
        let mut sent = 0;
        run(&mut a, &mut b, &mut clock, 120_000, |a, _| {
            sent += a.tcp_send(client, &data[sent..(sent + 4000).min(data.len())]).unwrap();
            false
        });
        assert!(sent < data.len(), "el emisor debe haberse quedado esperando");
        assert_eq!(a.tcp_state(client).unwrap(), (TcpState::Established, None));
        let mut got = Vec::new();
        let mut buffer = [0u8; 5000];
        assert!(run(&mut a, &mut b, &mut clock, 600_000, |a, b| {
            if sent < data.len() {
                sent += a.tcp_send(client, &data[sent..(sent + 4000).min(data.len())]).unwrap();
            }
            while let Some(n) = b.tcp_recv(server, &mut buffer, false).unwrap() {
                got.extend_from_slice(&buffer[..n]);
            }
            got.len() == data.len()
        }));
        assert!(got == data);
    }
}

#[test]
fn tcp_refused_reset_and_timeout() {
    let (mut a, mut b) = pair(0);
    let mut clock = 0;
    // Nadie escucha: rechazada.
    let client = a.tcp_connect(B, 81).unwrap();
    assert!(run(&mut a, &mut b, &mut clock, 5000, |a, _| a.tcp_state(client).unwrap().0 == TcpState::Closed));
    assert_eq!(a.tcp_state(client).unwrap().1, Some(Error::ConnectionRefused));
    a.release(client);
    // El otro extremo corta: reset.
    let listener = b.tcp_listen(80).unwrap();
    let client = a.tcp_connect(B, 80).unwrap();
    let mut server = None;
    assert!(run(&mut a, &mut b, &mut clock, 5000, |_, b| {
        server = b.tcp_accept(listener).unwrap();
        server.is_some()
    }));
    b.tcp_abort(server.unwrap()).unwrap();
    assert!(run(&mut a, &mut b, &mut clock, 5000, |a, _| a.tcp_state(client).unwrap().1 == Some(Error::ConnectionReset)));
    assert_eq!(a.tcp_recv(client, &mut [0; 4], false), Err(Error::ConnectionReset));
    a.release(client);
    // Una dirección que no existe: sin respuesta, la conexión caduca.
    let client = a.tcp_connect([10, 0, 0, 99], 80).unwrap();
    assert!(run(&mut a, &mut b, &mut clock, 200_000, |a, _| a.tcp_state(client).unwrap().0 == TcpState::Closed));
    assert_eq!(a.tcp_state(client).unwrap().1, Some(Error::TimedOut));
}

#[test]
fn tcp_release_closes_in_the_background() {
    let (mut a, mut b) = pair(0);
    let mut clock = 0;
    let listener = b.tcp_listen(80).unwrap();
    let client = a.tcp_connect(B, 80).unwrap();
    let mut server = None;
    assert!(run(&mut a, &mut b, &mut clock, 5000, |_, b| {
        server = server.or(b.tcp_accept(listener).unwrap());
        server.is_some()
    }));
    let server = server.unwrap();
    a.tcp_send(client, b"ultimo mensaje").unwrap();
    // Soltar el socket (lo que hace `drop`) entrega los datos y cierra.
    a.release(client);
    let mut buffer = [0u8; 64];
    assert!(run(&mut a, &mut b, &mut clock, 5000, |_, b| b.tcp_recv(server, &mut buffer, true).unwrap() == Some(14)));
    assert_eq!(b.tcp_recv(server, &mut buffer, false).unwrap(), Some(14));
    assert!(run(&mut a, &mut b, &mut clock, 5000, |_, b| b.tcp_recv(server, &mut buffer, false).unwrap() == Some(0)));
    b.release(server);
    assert!(run(&mut a, &mut b, &mut clock, 10_000, |a, b| a.tcp_state(client).is_err() && b.tcp_state(server).is_err()));
}

#[test]
fn tcp_and_udp_to_ourselves() {
    let (mut a, mut b) = pair(0);
    let mut clock = 0;
    for address in [[127, 0, 0, 1], A] {
        let listener = a.tcp_listen(7000).unwrap();
        let client = a.tcp_connect(address, 7000).unwrap();
        let mut server = None;
        assert!(run(&mut a, &mut b, &mut clock, 2000, |a, _| {
            server = server.or(a.tcp_accept(listener).unwrap());
            server.is_some() && a.tcp_state(client).unwrap().0 == TcpState::Established
        }));
        let server = server.unwrap();
        a.tcp_send(client, b"eco").unwrap();
        let mut buffer = [0u8; 8];
        assert!(run(&mut a, &mut b, &mut clock, 2000, |a, _| a.tcp_recv(server, &mut buffer, false).unwrap() == Some(3)));
        assert_eq!(&buffer[..3], b"eco");
        a.release(client);
        a.release(server);
        a.release(listener);
        assert!(run(&mut a, &mut b, &mut clock, 10_000, |a, _| a.tcp_state(client).is_err() && a.tcp_state(server).is_err()));
    }
    let socket = a.udp_bind(0).unwrap();
    let port = a.local_port(socket).unwrap();
    a.udp_send(socket, [127, 0, 0, 1], port, b"yo").unwrap();
    assert!(run(&mut a, &mut b, &mut clock, 100, |a, _| a.udp_recv(socket, true).unwrap().is_some()));
    assert_eq!(a.udp_recv(socket, false).unwrap(), Some(([127, 0, 0, 1], port, b"yo".to_vec())));
}

/// Monta una trama Ethernet + IP + UDP dirigida a todos.
fn udp_frame(src: Ip, src_port: u16, dst_port: u16, payload: &[u8]) -> Vec<u8> {
    let mut ip = vec![0x45, 0];
    ip.extend_from_slice(&((28 + payload.len()) as u16).to_be_bytes());
    ip.extend_from_slice(&[0, 0, 0, 0, 64, 17, 0, 0]);
    ip.extend_from_slice(&src);
    ip.extend_from_slice(&[255; 4]);
    let sum = checksum(&[&ip]);
    ip[10..12].copy_from_slice(&sum.to_be_bytes());
    let mut frame = vec![0xff; 6];
    frame.extend_from_slice(&[2, 0, 0, 0, 0, 0xfe, 8, 0]);
    frame.extend_from_slice(&ip);
    frame.extend_from_slice(&src_port.to_be_bytes());
    frame.extend_from_slice(&dst_port.to_be_bytes());
    frame.extend_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
    // Sin suma de comprobación de UDP (0 = no calculada).
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(payload);
    frame
}

#[test]
fn dhcp_discover_offer_request_ack_and_dns() {
    let (to_client, from_client) = (Rc::new(RefCell::new(VecDeque::new())), Rc::new(RefCell::new(VecDeque::new())));
    let wire = Wire { mac: [2, 0, 0, 0, 0, 9], incoming: to_client.clone(), outgoing: from_client.clone(), drop_every: 0, random: 1 };
    let mut client = Stack::new(wire);
    assert_eq!(client.udp_bind(0).and_then(|s| client.udp_send(s, B, 1, b"x")), Err(Error::NotConfigured));
    client.start_dhcp();
    let server_ip = [192, 168, 7, 1];
    let mut kinds = Vec::new();
    let mut dns_port = 0;
    let mut clock = 0;
    let mut lookup = None;
    let mut resolved = None;
    while clock < 20_000 && resolved.is_none() {
        client.poll(clock);
        // El "servidor": contesta a DHCP y a DNS.
        while let Some(frame) = from_client.borrow_mut().pop_front() {
            // ARP: el servidor dice dónde está.
            if frame[12..14] == [8, 6] && frame[21] == 1 && frame[38..42] == server_ip {
                let mut reply = frame[6..12].to_vec();
                reply.extend_from_slice(&[2, 0, 0, 0, 0, 0xfe, 8, 6, 0, 1, 8, 0, 6, 4, 0, 2, 2, 0, 0, 0, 0, 0xfe]);
                reply.extend_from_slice(&server_ip);
                reply.extend_from_slice(&frame[22..32]);
                to_client.borrow_mut().push_back(reply);
                continue;
            }
            if frame.len() < 42 || frame[12..14] != [8, 0] || frame[23] != 17 {
                continue;
            }
            let (src_port, dst_port) = (u16::from_be_bytes([frame[34], frame[35]]), u16::from_be_bytes([frame[36], frame[37]]));
            let payload = &frame[42..];
            if dst_port == 67 {
                let kind = payload[242];
                kinds.push(kind);
                // La primera petición se pierde, para forzar un reintento.
                if kinds.len() == 1 {
                    continue;
                }
                let mut reply = vec![0u8; 240];
                reply[0] = 2;
                reply[4..8].copy_from_slice(&payload[4..8]);
                reply[16..20].copy_from_slice(&[192, 168, 7, 50]);
                reply[28..34].copy_from_slice(&payload[28..34]);
                reply[236..240].copy_from_slice(&[99, 130, 83, 99]);
                reply.extend_from_slice(&[53, 1, if kind == 1 { 2 } else { 5 }]);
                reply.extend_from_slice(&[54, 4, 192, 168, 7, 1, 1, 4, 255, 255, 0, 0, 3, 4, 192, 168, 7, 1]);
                reply.extend_from_slice(&[6, 8, 192, 168, 7, 1, 8, 8, 8, 8, 51, 4, 0, 0, 14, 16, 255]);
                to_client.borrow_mut().push_back(udp_frame(server_ip, 67, 68, &reply));
            } else if dst_port == 53 {
                dns_port = src_port;
                // Respuesta: un CNAME y dos direcciones, con nombres comprimidos.
                let mut reply = payload[..2].to_vec();
                reply.extend_from_slice(&[0x81, 0x80, 0, 1, 0, 3, 0, 0, 0, 0]);
                reply.extend_from_slice(&payload[12..]);
                reply.extend_from_slice(&[0xc0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 4, 1, b'x', 0xc0, 12]);
                reply.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 93, 184, 216, 34]);
                reply.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 1, 2, 3, 4]);
                let mut frame = udp_frame(server_ip, 53, src_port, &reply);
                // Dirigida a la dirección del cliente.
                frame[30..34].copy_from_slice(&[192, 168, 7, 50]);
                frame[24..26].copy_from_slice(&[0, 0]);
                let sum = checksum(&[&frame[14..34]]);
                frame[24..26].copy_from_slice(&sum.to_be_bytes());
                to_client.borrow_mut().push_back(frame);
            }
        }
        if let (Some(_), None) = (client.config(), lookup) {
            lookup = Some(client.dns_start("example.com").unwrap());
        }
        if let Some(handle) = lookup {
            resolved = client.dns_result(handle);
        }
        clock += 10;
    }
    assert_eq!(kinds, [1, 1, 3], "descubrir (perdido), descubrir, pedir");
    assert_eq!(
        client.config(),
        Some(Config { ip: [192, 168, 7, 50], mask: [255, 255, 0, 0], gateway: [192, 168, 7, 1], dns: [[192, 168, 7, 1], [8, 8, 8, 8]] })
    );
    assert!(dns_port >= 49152);
    assert_eq!(resolved, Some(Ok(vec![[93, 184, 216, 34], [1, 2, 3, 4]])));
    assert_eq!(client.dns_start("nombre..malo"), Err(Error::NotFound));
}

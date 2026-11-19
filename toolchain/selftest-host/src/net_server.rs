//! Servidor de pruebas de red. Lo usan la batería `net-selftest` de la PS2
//! (a través de la red del emulador) y la misma batería ejecutada en el PC.
//!
//! Escucha sólo en 127.0.0.2, una dirección de bucle local: nada es
//! accesible desde fuera del PC. Los puertos los elige el sistema y quedan
//! apuntados en `net-test.cfg`.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::thread;
use std::time::Duration;

pub const ADDRESS: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 2);
pub const HELLO: &str = "hola desde el servidor\n";

/// Atiende cada conexión de `listener` en su propio hilo.
fn serve(listener: TcpListener, handler: fn(TcpStream)) {
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || handler(stream));
        }
    });
}

/// Devuelve lo que recibe hasta que el cliente cierra su lado.
fn echo(mut stream: TcpStream) {
    let mut buffer = [0u8; 4096];
    while let Ok(n) = stream.read(&mut buffer) {
        if n == 0 || stream.write_all(&buffer[..n]).is_err() {
            break;
        }
    }
}

/// Lee hasta el fin y responde cuántos bytes llegaron y cuánto suman.
fn count(mut stream: TcpStream) {
    let (mut total, mut sum) = (0u64, 0u32);
    let mut buffer = [0u8; 4096];
    while let Ok(n) = stream.read(&mut buffer) {
        if n == 0 {
            break;
        }
        total += n as u64;
        sum = buffer[..n].iter().fold(sum, |s, &b| s.wrapping_add(b as u32));
    }
    let _ = stream.write_all(format!("{total} {sum}\n").as_bytes());
}

fn hello(mut stream: TcpStream) {
    let _ = stream.write_all(HELLO.as_bytes());
}

/// Acepta y no dice nada, hasta que el cliente se va.
fn silent(mut stream: TcpStream) {
    let mut buffer = [0u8; 256];
    while matches!(stream.read(&mut buffer), Ok(n) if n > 0) {}
}

/// Arranca el servidor, apunta sus puertos en `directory/net-test.cfg` y no
/// vuelve: termina cuando alguien conecta al puerto `quit` (o a los 20
/// minutos, por si nadie lo para).
pub fn run(directory: &Path) -> ! {
    let listen = || TcpListener::bind((ADDRESS, 0)).expect("no pude escuchar en 127.0.0.2");
    let port = |l: &TcpListener| l.local_addr().unwrap().port();
    let (echo_l, count_l, hello_l, silent_l, quit_l) = (listen(), listen(), listen(), listen(), listen());
    // Un puerto donde no escucha nadie.
    let closed = port(&listen());
    let udp = UdpSocket::bind((ADDRESS, 0)).expect("no pude abrir UDP en 127.0.0.2");
    let mut config = format!(
        "host={ADDRESS}\necho={}\ncount={}\nhello={}\nsilent={}\nclosed={closed}\nudp={}\nquit={}\n",
        port(&echo_l),
        port(&count_l),
        port(&hello_l),
        port(&silent_l),
        udp.local_addr().unwrap().port(),
        port(&quit_l)
    );
    config.push_str(&crate::web_server::start());
    serve(echo_l, echo);
    serve(count_l, count);
    serve(hello_l, hello);
    serve(silent_l, silent);
    thread::spawn(move || {
        let mut buffer = [0u8; 2048];
        while let Ok((n, from)) = udp.recv_from(&mut buffer) {
            let _ = udp.send_to(&buffer[..n], from);
        }
    });
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(20 * 60));
        std::process::exit(0);
    });
    std::fs::write(directory.join("net-test.cfg"), &config).expect("no pude escribir net-test.cfg");
    println!("servidor de pruebas en {ADDRESS}:\n{config}");
    let _ = quit_l.accept();
    std::process::exit(0);
}

/// Para el servidor que dejó apuntado su puerto `quit` en `directory`.
pub fn stop(directory: &Path) {
    let Ok(config) = std::fs::read_to_string(directory.join("net-test.cfg")) else { return };
    if let Some(port) = config.lines().find_map(|l| l.strip_prefix("quit=")).and_then(|p| p.parse::<u16>().ok()) {
        let _ = TcpStream::connect_timeout(&SocketAddr::from((ADDRESS, port)), Duration::from_secs(2));
    }
}

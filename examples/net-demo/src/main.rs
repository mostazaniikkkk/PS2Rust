//! La PlayStation 2 en Internet con `std::net`.
//!
//! Pide dirección por DHCP, resuelve `example.com` por DNS, descarga su
//! página por HTTP (con el crate `ps2-web`) y monta un servidor y un cliente
//! TCP dentro de la propia consola. Todo con la pila TCP/IP de la std de
//! Rust para PS2 y el adaptador de red en un módulo del IOP (`ps2net.irx`).
//!
//!     cargo ps2 run -p net-demo
//!
//! Necesita el adaptador de red (en PCSX2: Ethernet activado) y salida a
//! Internet para la descarga.

use ps2::gs::{Color, Gs, VideoMode};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::thread;
use std::time::{Duration, Instant};

fn main() {
    let mut lines: Vec<(bool, String)> = Vec::new();
    let mut report = |ok: bool, text: String| {
        println!("[{}] {text}", if ok { "ok" } else { "FALLO" });
        lines.push((ok, text));
    };

    // 1. Adaptador y DHCP.
    let start = Instant::now();
    let info = match std::os::ps2::network_info() {
        Ok(info) => info,
        Err(e) => {
            println!("sin red: {e}");
            std::process::exit(100);
        }
    };
    report(true, format!("DHCP en {} ms: {}", start.elapsed().as_millis(), info.ip));
    report(true, format!("puerta {}  DNS {}", info.gateway, info.dns[0]));

    // 2. DNS.
    let start = Instant::now();
    let resolved: Vec<_> = "example.com:80".to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
    report(!resolved.is_empty(), format!("DNS example.com: {} direcciones en {} ms", resolved.len(), start.elapsed().as_millis()));

    // 3. HTTP contra Internet.
    let start = Instant::now();
    match ps2_web::http::get("http://example.com/") {
        Ok(page) => {
            let text = page.text();
            let title = text.split("<title>").nth(1).and_then(|t| t.split("</title>").next()).unwrap_or("?").to_string();
            report(page.status == 200, format!("HTTP {} en {} ms: {} bytes", page.status, start.elapsed().as_millis(), page.body.len()));
            report(title == "Example Domain", format!("titulo: {title}"));
        }
        Err(e) => report(false, format!("HTTP: {e}")),
    }

    // 4. Servidor y cliente TCP en la propia consola.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept()?;
        let mut total = 0usize;
        let mut buffer = [0u8; 4096];
        loop {
            let n = stream.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n;
        }
        stream.write_all(format!("recibidos {total}").as_bytes())?;
        Ok::<_, std::io::Error>(total)
    });
    let start = Instant::now();
    let local = TcpStream::connect(("127.0.0.1", port)).and_then(|mut stream| {
        stream.write_all(&vec![7u8; 256 * 1024])?;
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply)?;
        Ok(reply)
    });
    let _ = server.join();
    report(local.as_deref().ok() == Some("recibidos 262144"), format!("TCP local: 256 KiB en {} ms", start.elapsed().as_millis()));

    let good = lines.iter().filter(|l| l.0).count();
    let total = lines.len();
    println!("demo de red: {good}/{total} correctas");

    let mut gs = Gs::init(VideoMode::Ntsc);
    for frame in 0..300u32 {
        gs.clear(Color::rgb(12, 20, 36));
        gs.rect(40, 50, 560, 4, Color::rgb(90, 170, 250));
        gs.text(40, 66, 5, Color::WHITE, "std::net");
        gs.text(40, 116, 2, Color::rgb(150, 200, 250), "TCP/IP en Rust: EE (pila) + IOP (adaptador)");
        for (i, (ok, line)) in lines.iter().enumerate() {
            let y = 160 + i as i32 * 26;
            let color = if *ok { Color::rgb(120, 220, 140) } else { Color::rgb(240, 90, 90) };
            gs.text(40, y, 2, color, if *ok { "ok" } else { "MAL" });
            gs.text(90, y, 2, Color::rgb(210, 220, 235), line);
        }
        let bar = (frame as i32 * 560 / 299).max(1);
        gs.rect(40, 380, bar, 8, Color::rgb(90, 170, 250));
        gs.present();
        if frame == 0 {
            println!("pantalla lista");
        }
        thread::sleep(Duration::from_millis(1));
    }
    std::process::exit((total - good) as i32);
}

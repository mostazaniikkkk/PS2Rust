//! Servicios HTTP y WebSocket del servidor de pruebas (en 127.0.0.2).

use crate::net_server::ADDRESS;
use ps2_web::ws::{Message, WebSocket};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

/// Cuerpo de `/grande`: 300 000 bytes que se pueden recalcular.
pub fn big_body() -> Vec<u8> {
    (0..300_000usize).map(|i| ((i * 13 + i / 251) % 256) as u8).collect()
}

/// Atiende una petición HTTP y cierra.
fn http(mut stream: TcpStream) {
    let mut data = Vec::new();
    let mut buffer = [0u8; 2048];
    let end = loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        match stream.read(&mut buffer) {
            Ok(n) if n > 0 => data.extend_from_slice(&buffer[..n]),
            _ => return,
        }
    };
    let head = String::from_utf8_lossy(&data[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request = lines.next().unwrap_or("").split(' ');
    let (method, path) = (request.next().unwrap_or(""), request.next().unwrap_or(""));
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let header = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    let length: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = data[end + 4..].to_vec();
    while body.len() < length {
        match stream.read(&mut buffer) {
            Ok(n) if n > 0 => body.extend_from_slice(&buffer[..n]),
            _ => return,
        }
    }
    let plain = |status: &str, extra: &str, body: &[u8]| {
        let mut out = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n", body.len()).into_bytes();
        out.extend_from_slice(body);
        out
    };
    let response = match (method, path) {
        (_, "/hola") => plain("200 OK", "", b"hola mundo"),
        (_, "/trozos") => b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\ne\r\nprimer trozo, \r\nf\r\nsegundo trozo, \r\n5\r\ny fin\r\n0\r\n\r\n".to_vec(),
        (_, "/redir") => plain("302 Found", "Location: /hola\r\n", b"por alli"),
        ("POST", "/eco") => plain("200 OK", "X-Metodo: POST\r\n", &body),
        (_, "/cabecera") => plain("200 OK", "", header("x-prueba").unwrap_or("").as_bytes()),
        (_, "/grande") => plain("200 OK", "", &big_body()),
        (_, "/sin-longitud") => b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nhasta el cierre".to_vec(),
        _ => plain("404 Not Found", "", b"no existe"),
    };
    let _ = stream.write_all(&response);
}

/// Eco por WebSocket. Tres textos tienen truco: "fragmenta" (respuesta en
/// tres tramas con un ping por medio), "cierra" (el servidor cierra) y
/// "calla" (sin respuesta).
fn websocket(stream: TcpStream) {
    let Ok(mut socket) = WebSocket::accept(stream) else { return };
    loop {
        let result = match socket.read() {
            Ok(Message::Text(t)) if t == "fragmenta" => socket
                .send_frame(false, 1, b"uno ")
                .and_then(|_| socket.send_frame(true, 9, b"en medio"))
                .and_then(|_| socket.send_frame(false, 0, b"dos "))
                .and_then(|_| socket.send_frame(true, 0, b"tres")),
            Ok(Message::Text(t)) if t == "cierra" => socket.close(1000, "adios"),
            Ok(Message::Text(t)) if t == "calla" => Ok(()),
            Ok(message @ (Message::Text(_) | Message::Binary(_))) => socket.send(&message),
            Ok(_) => Ok(()),
            Err(_) => break,
        };
        if result.is_err() {
            break;
        }
    }
}

/// Arranca los servicios y devuelve sus líneas para `net-test.cfg`.
pub fn start() -> String {
    let mut config = String::new();
    let services: [(&str, fn(TcpStream)); 2] = [("http", http), ("ws", websocket)];
    for (name, handler) in services {
        let listener = TcpListener::bind((ADDRESS, 0)).expect("no pude escuchar en 127.0.0.2");
        config.push_str(&format!("{name}={}\n", listener.local_addr().unwrap().port()));
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                thread::spawn(move || handler(stream));
            }
        });
    }
    config
}

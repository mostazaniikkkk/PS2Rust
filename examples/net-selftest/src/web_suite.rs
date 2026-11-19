//! Batería de HTTP y WebSocket (crate `ps2-web`), contra los servicios web
//! del servidor de pruebas. Como `net_suite`, se ejecuta igual en la PS2 y
//! en el PC y las dos salidas deben coincidir.

use crate::net_suite::{Config, T};
use ps2_web::ws::{Message, WebSocket};
use ps2_web::{http, Error};
use std::fmt::Write as _;
use std::thread;
use std::time::Duration;

pub fn run(config: &Config) -> (String, u32) {
    let mut t = T { out: String::new(), total: 0, failed: 0 };
    http_client(&mut t, config);
    websocket(&mut t, config);
    let (total, failed) = (t.total, t.failed);
    let _ = writeln!(t.out, "== resultado web: {total} comprobaciones, {failed} fallos");
    (t.out, failed)
}

fn http_client(t: &mut T, config: &Config) {
    t.section("HTTP");
    let base = format!("http://{}:{}", config.host, config.port("http"));
    let page = http::get(&format!("{base}/hola"));
    t.expect("GET con Content-Length", matches!(&page, Ok(r) if r.status == 200 && r.text() == "hola mundo"), &page);
    t.check("cabeceras de la respuesta", page.as_ref().is_ok_and(|r| r.header("content-type") == Some("text/plain; charset=utf-8") && r.reason == "OK"));
    let chunked = http::get(&format!("{base}/trozos"));
    t.expect("cuerpo troceado (chunked)", matches!(&chunked, Ok(r) if r.text() == "primer trozo, segundo trozo, y fin"), &chunked);
    let redirected = http::get(&format!("{base}/redir"));
    t.expect("sigue la redirección", matches!(&redirected, Ok(r) if r.status == 200 && r.text() == "hola mundo"), &redirected);
    let not_followed = http::Request::new("GET", &format!("{base}/redir")).and_then(|r| r.follow_redirects(false).send());
    t.expect("o no, si se pide", matches!(&not_followed, Ok(r) if r.status == 302 && r.header("Location") == Some("/hola")), &not_followed);
    let missing = http::get(&format!("{base}/no-esta"));
    t.expect("404 con su cuerpo", matches!(&missing, Ok(r) if r.status == 404 && r.text() == "no existe"), &missing);
    let body: Vec<u8> = (0..5000usize).map(|i| (i % 200) as u8 + 32).collect();
    let echoed = http::post(&format!("{base}/eco"), "application/octet-stream", body.clone());
    t.expect("POST de 5000 bytes devuelto", matches!(&echoed, Ok(r) if r.body == body && r.header("X-Metodo") == Some("POST")), echoed.as_ref().map(|r| r.body.len()));
    let custom = http::Request::new("GET", &format!("{base}/cabecera")).and_then(|r| r.header("X-Prueba", "desde la PS2").send());
    t.expect("cabecera propia en la petición", matches!(&custom, Ok(r) if r.text() == "desde la PS2"), &custom);
    let big = http::get(&format!("{base}/grande"));
    let expected: Vec<u8> = (0..300_000usize).map(|i| ((i * 13 + i / 251) % 256) as u8).collect();
    t.expect("300 000 bytes sin un byte cambiado", matches!(&big, Ok(r) if r.body == expected), big.as_ref().map(|r| r.body.len()));
    let unbounded = http::get(&format!("{base}/sin-longitud"));
    t.expect("cuerpo sin longitud, hasta el cierre", matches!(&unbounded, Ok(r) if r.text() == "hasta el cierre"), &unbounded);
    t.check("https:// da Error::Tls", matches!(http::get("https://example.com/"), Err(Error::Tls)));
    t.check("una dirección mal escrita da Error::Url", matches!(http::get("example.com"), Err(Error::Url(_))));
}

fn websocket(t: &mut T, config: &Config) {
    t.section("WebSocket");
    let url = format!("ws://{}:{}/eco", config.host, config.port("ws"));
    let socket = WebSocket::connect(&url);
    t.expect("saludo", socket.is_ok(), socket.as_ref().err());
    let Ok(mut socket) = socket else { return };
    let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
    let text = "hola desde la consola: ñandú, 日本";
    let echoed = socket.send_text(text).and_then(|_| socket.read());
    t.expect("texto UTF-8 de ida y vuelta", matches!(&echoed, Ok(Message::Text(s)) if s == text), &echoed);
    let data: Vec<u8> = (0..70_000usize).map(|i| (i % 249) as u8).collect();
    let binary = socket.send_binary(&data).and_then(|_| socket.read());
    t.expect("70 000 bytes binarios", matches!(&binary, Ok(Message::Binary(b)) if *b == data), binary.as_ref().map(|_| ()));
    let pong = socket.ping(b"latido").and_then(|_| socket.read());
    t.expect("ping y pong", matches!(&pong, Ok(Message::Pong(p)) if p == b"latido"), &pong);
    // El servidor responde en tres tramas con un ping por medio.
    let _ = socket.send_text("fragmenta");
    let (first, second) = (socket.read(), socket.read());
    t.expect("ping del servidor entre fragmentos", matches!(&first, Ok(Message::Ping(p)) if p == b"en medio"), &first);
    t.expect("mensaje fragmentado, recompuesto", matches!(&second, Ok(Message::Text(s)) if s == "uno dos tres"), &second);
    // Conexión larga: plazo de lectura sin nada que leer, y sigue viva.
    let _ = socket.set_read_timeout(Some(Duration::from_millis(150)));
    let _ = socket.send_text("calla");
    let quiet = socket.read();
    t.expect("sin mensajes: Error::Timeout", matches!(quiet, Err(Error::Timeout)), &quiet);
    let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
    let mut beats = 0;
    for i in 0..5u8 {
        if matches!(socket.ping(&[i]).and_then(|_| socket.read()), Ok(Message::Pong(p)) if p == [i]) {
            beats += 1;
        }
        thread::sleep(Duration::from_millis(50));
    }
    t.expect("cinco latidos tras el plazo", beats == 5, beats);
    // Cierre iniciado por el servidor.
    let _ = socket.send_text("cierra");
    let closed = socket.read();
    t.expect("cierre del servidor con código y motivo", matches!(&closed, Ok(Message::Close(Some((1000, reason)))) if reason == "adios"), &closed);
    t.check("después, Error::Closed", matches!(socket.read(), Err(Error::Closed)) && matches!(socket.send_text("tarde"), Err(Error::Closed)));
    // Cierre iniciado por el cliente.
    let goodbye = WebSocket::connect(&url).and_then(|mut s| {
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.close(1001, "me voy")?;
        s.read()
    });
    t.expect("cierre del cliente, confirmado por el servidor", matches!(&goodbye, Ok(Message::Close(Some((1001, _))))), &goodbye);
    t.check("wss:// da Error::Tls", matches!(WebSocket::connect("wss://example.com/"), Err(Error::Tls)));
    let not_ws = WebSocket::connect(&format!("ws://{}:{}/hola", config.host, config.port("http")));
    t.expect("un servidor HTTP normal no es un WebSocket", matches!(not_ws, Err(Error::Protocol(_))), not_ws.as_ref().err());
}

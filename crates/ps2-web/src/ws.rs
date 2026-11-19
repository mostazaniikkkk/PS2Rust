//! WebSocket (RFC 6455) sobre TCP sin cifrar: cliente y lado servidor.
//!
//! Pensado para conexiones largas con latidos: [`WebSocket::read`] con plazo
//! devuelve [`Error::Timeout`] sin perder lo que haya llegado a medias, así
//! que se puede alternar entre esperar mensajes y enviar un ping.

use crate::util::{base64, random_bytes, sha1};
use crate::{http, Error, Url};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// Tamaño máximo de un mensaje recibido.
const MAX_MESSAGE: usize = 4 * 1024 * 1024;

const OP_CONTINUATION: u8 = 0;
const OP_TEXT: u8 = 1;
const OP_BINARY: u8 = 2;
const OP_CLOSE: u8 = 8;
const OP_PING: u8 = 9;
const OP_PONG: u8 = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// Cierre, con su código y motivo si los trae.
    Close(Option<(u16, String)>),
}

pub struct WebSocket {
    stream: TcpStream,
    /// El cliente enmascara lo que envía; el servidor no.
    client: bool,
    /// Bytes recibidos que aún no forman una trama completa.
    buffer: Vec<u8>,
    /// Mensaje fragmentado a medio llegar: código y datos acumulados.
    partial: Option<(u8, Vec<u8>)>,
    close_sent: bool,
    closed: bool,
}

fn accept_key(key: &str) -> String {
    base64(&sha1(format!("{key}{GUID}").as_bytes()))
}

impl WebSocket {
    /// Conecta con un servidor `ws://host[:puerto]/ruta` (plazo de 15 s para
    /// conectar y saludar; después, lecturas sin plazo).
    pub fn connect(url: &str) -> Result<WebSocket, Error> {
        WebSocket::connect_with(url, &[], Some(Duration::from_secs(15)))
    }

    /// Como [`connect`](WebSocket::connect), con cabeceras adicionales
    /// (`Origin`, `Sec-WebSocket-Protocol`…) y el plazo del saludo.
    pub fn connect_with(url: &str, headers: &[(&str, &str)], timeout: Option<Duration>) -> Result<WebSocket, Error> {
        let url = Url::parse(url)?;
        if url.scheme != "ws" {
            return Err(Error::Url(format!("{}:// no es WebSocket", url.scheme)));
        }
        let mut stream = http::connect(&url, timeout)?;
        let key = base64(&random_bytes::<16>());
        let mut request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n",
            url.path,
            url.host_header()
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes())?;
        let (status, response_headers, rest) = http::read_head(&mut stream)?;
        if status.split(' ').nth(1) != Some("101") {
            return Err(Error::Protocol(format!("el servidor no aceptó el WebSocket: {status:?}")));
        }
        let accept = response_headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("Sec-WebSocket-Accept")).map(|(_, v)| v.as_str());
        if accept != Some(accept_key(&key).as_str()) {
            return Err(Error::Protocol("Sec-WebSocket-Accept no coincide".to_string()));
        }
        stream.set_read_timeout(None)?;
        Ok(WebSocket { stream, client: true, buffer: rest, partial: None, close_sent: false, closed: false })
    }

    /// Lado servidor: atiende el saludo de un cliente que acaba de conectar.
    pub fn accept(mut stream: TcpStream) -> Result<WebSocket, Error> {
        let (_, headers, rest) = http::read_head(&mut stream)?;
        let key = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("Sec-WebSocket-Key")).map(|(_, v)| v.clone());
        let Some(key) = key else {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
            return Err(Error::Protocol("la petición no es un saludo de WebSocket".to_string()));
        };
        let response = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_key(&key));
        stream.write_all(response.as_bytes())?;
        Ok(WebSocket { stream, client: false, buffer: rest, partial: None, close_sent: false, closed: false })
    }

    /// Plazo de [`read`](WebSocket::read). `None` = esperar sin límite.
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        Ok(self.stream.set_read_timeout(timeout)?)
    }

    /// La conexión TCP de debajo.
    pub fn stream(&self) -> &TcpStream {
        &self.stream
    }

    /// Envía una trama suelta. Para mensajes normales usa
    /// [`send`](WebSocket::send); esto sirve para fragmentar a mano.
    pub fn send_frame(&mut self, fin: bool, opcode: u8, payload: &[u8]) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push((fin as u8) << 7 | (opcode & 15));
        let mask_bit = (self.client as u8) << 7;
        match payload.len() {
            n if n < 126 => frame.push(mask_bit | n as u8),
            n if n < 65536 => {
                frame.push(mask_bit | 126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                frame.push(mask_bit | 127);
                frame.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        if self.client {
            let mask = random_bytes::<4>();
            frame.extend_from_slice(&mask);
            frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        } else {
            frame.extend_from_slice(payload);
        }
        self.stream.write_all(&frame)?;
        Ok(())
    }

    pub fn send(&mut self, message: &Message) -> Result<(), Error> {
        match message {
            Message::Text(text) => self.send_frame(true, OP_TEXT, text.as_bytes()),
            Message::Binary(data) => self.send_frame(true, OP_BINARY, data),
            Message::Ping(data) => self.send_frame(true, OP_PING, data),
            Message::Pong(data) => self.send_frame(true, OP_PONG, data),
            Message::Close(reason) => {
                let mut payload = Vec::new();
                if let Some((code, text)) = reason {
                    payload.extend_from_slice(&code.to_be_bytes());
                    payload.extend_from_slice(text.as_bytes());
                }
                self.close_sent = true;
                self.send_frame(true, OP_CLOSE, &payload)
            }
        }
    }

    pub fn send_text(&mut self, text: &str) -> Result<(), Error> {
        self.send_frame(true, OP_TEXT, text.as_bytes())
    }

    pub fn send_binary(&mut self, data: &[u8]) -> Result<(), Error> {
        self.send_frame(true, OP_BINARY, data)
    }

    /// Envía un ping (el latido de WebSocket). La respuesta llega por
    /// [`read`](WebSocket::read) como [`Message::Pong`].
    pub fn ping(&mut self, data: &[u8]) -> Result<(), Error> {
        self.send_frame(true, OP_PING, data)
    }

    /// Inicia el cierre. El otro extremo contesta con su propio cierre, que
    /// llega por [`read`](WebSocket::read).
    pub fn close(&mut self, code: u16, reason: &str) -> Result<(), Error> {
        self.send(&Message::Close(Some((code, reason.to_string()))))
    }

    /// Saca del búfer la siguiente trama completa: (fin, código, datos).
    fn take_frame(&mut self) -> Result<Option<(bool, u8, Vec<u8>)>, Error> {
        let b = &self.buffer;
        if b.len() < 2 {
            return Ok(None);
        }
        if b[0] & 0x70 != 0 {
            return Err(Error::Protocol("trama con bits reservados".to_string()));
        }
        let (fin, opcode, masked) = (b[0] & 0x80 != 0, b[0] & 15, b[1] & 0x80 != 0);
        let (length, mut offset) = match b[1] & 127 {
            126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as u64, 4),
            127 if b.len() >= 10 => (u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]), 10),
            126 | 127 => return Ok(None),
            n => (n as u64, 2),
        };
        if length > MAX_MESSAGE as u64 {
            return Err(Error::Protocol("trama demasiado grande".to_string()));
        }
        if opcode >= 8 && (length > 125 || !fin) {
            return Err(Error::Protocol("trama de control no válida".to_string()));
        }
        let length = length as usize;
        let mask = if masked {
            if b.len() < offset + 4 {
                return Ok(None);
            }
            offset += 4;
            Some([b[offset - 4], b[offset - 3], b[offset - 2], b[offset - 1]])
        } else {
            None
        };
        if b.len() < offset + length {
            return Ok(None);
        }
        let mut payload = b[offset..offset + length].to_vec();
        if let Some(mask) = mask {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i % 4];
            }
        }
        self.buffer.drain(..offset + length);
        Ok(Some((fin, opcode, payload)))
    }

    /// Espera el siguiente mensaje. Los pings se contestan solos (y también
    /// se devuelven). Con plazo, [`Error::Timeout`] no pierde nada: se puede
    /// volver a llamar.
    pub fn read(&mut self) -> Result<Message, Error> {
        loop {
            if self.closed {
                return Err(Error::Closed);
            }
            let Some((fin, opcode, payload)) = self.take_frame()? else {
                let mut chunk = [0u8; 4096];
                let n = self.stream.read(&mut chunk)?;
                if n == 0 {
                    self.closed = true;
                    return Err(Error::Closed);
                }
                self.buffer.extend_from_slice(&chunk[..n]);
                continue;
            };
            match opcode {
                OP_PING => {
                    self.send_frame(true, OP_PONG, &payload)?;
                    return Ok(Message::Ping(payload));
                }
                OP_PONG => return Ok(Message::Pong(payload)),
                OP_CLOSE => {
                    let reason = (payload.len() >= 2).then(|| (u16::from_be_bytes([payload[0], payload[1]]), String::from_utf8_lossy(&payload[2..]).into_owned()));
                    // Si el cierre lo inició el otro extremo, se le devuelve.
                    if !self.close_sent {
                        let _ = self.send_frame(true, OP_CLOSE, &payload[..payload.len().min(2)]);
                    }
                    self.closed = true;
                    return Ok(Message::Close(reason));
                }
                OP_TEXT | OP_BINARY | OP_CONTINUATION => {
                    let (kind, data) = match (opcode, self.partial.take()) {
                        (OP_CONTINUATION, Some((kind, mut data))) => {
                            if data.len() + payload.len() > MAX_MESSAGE {
                                return Err(Error::Protocol("mensaje demasiado grande".to_string()));
                            }
                            data.extend_from_slice(&payload);
                            (kind, data)
                        }
                        (OP_CONTINUATION, None) => return Err(Error::Protocol("continuación sin mensaje".to_string())),
                        (_, Some(_)) => return Err(Error::Protocol("mensaje nuevo en mitad de otro".to_string())),
                        (kind, None) => (kind, payload),
                    };
                    if !fin {
                        self.partial = Some((kind, data));
                        continue;
                    }
                    return if kind == OP_TEXT {
                        String::from_utf8(data).map(Message::Text).map_err(|_| Error::Protocol("texto que no es UTF-8".to_string()))
                    } else {
                        Ok(Message::Binary(data))
                    };
                }
                other => return Err(Error::Protocol(format!("código de trama desconocido: {other}"))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// Servidor de eco en un hilo; "fragmenta" y "cierra" tienen truco.
    fn server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                thread::spawn(move || {
                    let Ok(mut socket) = WebSocket::accept(stream) else { return };
                    loop {
                        match socket.read() {
                            Ok(Message::Text(t)) if t == "fragmenta" => {
                                socket.send_frame(false, OP_TEXT, b"uno ").unwrap();
                                socket.send_frame(true, OP_PING, b"en medio").unwrap();
                                socket.send_frame(false, OP_CONTINUATION, b"dos ").unwrap();
                                socket.send_frame(true, OP_CONTINUATION, b"tres").unwrap();
                            }
                            Ok(Message::Text(t)) if t == "cierra" => socket.close(1000, "adios").unwrap(),
                            Ok(message @ (Message::Text(_) | Message::Binary(_))) => socket.send(&message).unwrap(),
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                });
            }
        });
        port
    }

    #[test]
    fn echo_fragments_ping_timeout_and_close() {
        let port = server();
        let mut socket = WebSocket::connect(&format!("ws://127.0.0.1:{port}/eco")).unwrap();
        socket.send_text("hola ñandú").unwrap();
        assert_eq!(socket.read().unwrap(), Message::Text("hola ñandú".to_string()));
        // Longitudes de 7, 16 y 64 bits.
        for size in [0usize, 125, 126, 65535, 65536, 200_000] {
            let data: Vec<u8> = (0..size).map(|i| (i % 253) as u8).collect();
            socket.send_binary(&data).unwrap();
            assert_eq!(socket.read().unwrap(), Message::Binary(data), "tamaño {size}");
        }
        socket.ping(b"latido").unwrap();
        assert_eq!(socket.read().unwrap(), Message::Pong(b"latido".to_vec()));
        // Un mensaje en tres tramas con un ping por medio.
        socket.send_text("fragmenta").unwrap();
        assert_eq!(socket.read().unwrap(), Message::Ping(b"en medio".to_vec()));
        assert_eq!(socket.read().unwrap(), Message::Text("uno dos tres".to_string()));
        // Plazo: nada que leer, y después todo sigue funcionando.
        socket.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        assert!(matches!(socket.read(), Err(Error::Timeout)));
        socket.send_text("sigo").unwrap();
        assert_eq!(socket.read().unwrap(), Message::Text("sigo".to_string()));
        // Cierre iniciado por el servidor.
        socket.send_text("cierra").unwrap();
        assert_eq!(socket.read().unwrap(), Message::Close(Some((1000, "adios".to_string()))));
        assert!(matches!(socket.read(), Err(Error::Closed)));
        assert!(matches!(socket.send_text("tarde"), Err(Error::Closed)));
    }

    #[test]
    fn close_started_by_the_client_and_bad_urls() {
        let port = server();
        let mut socket = WebSocket::connect(&format!("ws://127.0.0.1:{port}/")).unwrap();
        socket.close(1001, "me voy").unwrap();
        assert_eq!(socket.read().unwrap(), Message::Close(Some((1001, String::new()))));
        assert!(matches!(WebSocket::connect("wss://example.com/"), Err(Error::Tls)));
        assert!(matches!(WebSocket::connect("http://example.com/"), Err(Error::Url(_))));
    }

    #[test]
    fn a_plain_http_server_is_not_a_websocket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = http::read_head(&mut stream);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nno");
            }
        });
        assert!(matches!(WebSocket::connect(&format!("ws://127.0.0.1:{port}/")), Err(Error::Protocol(_))));
    }
}

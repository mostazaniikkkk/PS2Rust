//! Cliente HTTP/1.1: una petición por conexión, con redirecciones y cuerpos
//! troceados (`Transfer-Encoding: chunked`). Sin TLS ni compresión.

use crate::{Error, Url};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Tamaño máximo aceptado para las cabeceras y para el cuerpo.
const MAX_HEADERS: usize = 64 * 1024;
const MAX_BODY: usize = 8 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub reason: String,
    /// Cabeceras en el orden en que llegaron.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// Valor de una cabecera (sin distinguir mayúsculas).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// El cuerpo como texto UTF-8 (lo que no lo sea se sustituye).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Una petición por montar. `Request::new("GET", url)?.header(..).send()`.
pub struct Request {
    method: String,
    url: Url,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    timeout: Option<Duration>,
    follow_redirects: bool,
}

impl Request {
    pub fn new(method: &str, url: &str) -> Result<Request, Error> {
        let url = Url::parse(url)?;
        if url.scheme != "http" {
            return Err(Error::Url(format!("{}:// no es HTTP", url.scheme)));
        }
        Ok(Request { method: method.to_ascii_uppercase(), url, headers: Vec::new(), body: Vec::new(), timeout: Some(Duration::from_secs(15)), follow_redirects: true })
    }

    pub fn header(mut self, name: &str, value: &str) -> Request {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Request {
        self.body = body.into();
        self
    }

    /// Plazo para conectar y para cada lectura (por defecto 15 s).
    pub fn timeout(mut self, timeout: Option<Duration>) -> Request {
        self.timeout = timeout;
        self
    }

    pub fn follow_redirects(mut self, follow: bool) -> Request {
        self.follow_redirects = follow;
        self
    }

    pub fn send(mut self) -> Result<Response, Error> {
        for _ in 0..=MAX_REDIRECTS {
            let response = self.send_once()?;
            let location = match response.status {
                301 | 302 | 303 | 307 | 308 if self.follow_redirects => response.header("Location").map(str::to_string),
                _ => None,
            };
            let Some(location) = location else { return Ok(response) };
            // La nueva dirección puede ser completa o sólo una ruta.
            self.url = if location.contains("://") {
                Url::parse(&location)?
            } else if location.starts_with('/') {
                Url { path: location, ..self.url.clone() }
            } else {
                return Err(Error::Protocol(format!("redirección a {location:?}")));
            };
            if matches!(response.status, 301 | 302 | 303) && self.method != "HEAD" {
                self.method = "GET".to_string();
                self.body.clear();
            }
        }
        Err(Error::Protocol("demasiadas redirecciones".to_string()))
    }

    fn send_once(&self) -> Result<Response, Error> {
        let mut stream = connect(&self.url, self.timeout)?;
        let mut head = format!("{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept-Encoding: identity\r\n", self.method, self.url.path, self.url.host_header());
        if !self.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("User-Agent")) {
            head.push_str("User-Agent: ps2-web/0.1\r\n");
        }
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if !self.body.is_empty() || matches!(self.method.as_str(), "POST" | "PUT" | "PATCH") {
            head.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        }
        head.push_str("\r\n");
        // Cabeceras y cuerpo en una sola escritura.
        let mut message = head.into_bytes();
        message.extend_from_slice(&self.body);
        stream.write_all(&message)?;
        read_response(&mut stream, self.method == "HEAD")
    }
}

/// `GET` de una dirección `http://`.
pub fn get(url: &str) -> Result<Response, Error> {
    Request::new("GET", url)?.send()
}

/// `POST` con el cuerpo y el tipo de contenido indicados.
pub fn post(url: &str, content_type: &str, body: impl Into<Vec<u8>>) -> Result<Response, Error> {
    Request::new("POST", url)?.header("Content-Type", content_type).body(body).send()
}

/// Abre la conexión TCP con el servidor de `url`.
pub(crate) fn connect(url: &Url, timeout: Option<Duration>) -> Result<TcpStream, Error> {
    use std::net::ToSocketAddrs;
    let addresses: Vec<_> = (url.host.as_str(), url.port).to_socket_addrs()?.collect();
    let mut last = None;
    for address in &addresses {
        let attempt = match timeout {
            Some(t) => TcpStream::connect_timeout(address, t),
            None => TcpStream::connect(address),
        };
        match attempt {
            Ok(stream) => {
                stream.set_read_timeout(timeout)?;
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.map(Error::from).unwrap_or_else(|| Error::Url(format!("{} no tiene dirección", url.host))))
}

/// Lee de `stream` hasta el final de las cabeceras. Devuelve la línea de
/// estado, las cabeceras y lo que se haya leído de más (inicio del cuerpo).
pub(crate) fn read_head(stream: &mut impl Read) -> Result<(String, Vec<(String, String)>, Vec<u8>), Error> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 2048];
    let end = loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        if data.len() > MAX_HEADERS {
            return Err(Error::Protocol("cabeceras demasiado largas".to_string()));
        }
        let n = stream.read(&mut buffer)?;
        if n == 0 {
            return Err(Error::Protocol("la conexión se cerró antes de las cabeceras".to_string()));
        }
        data.extend_from_slice(&buffer[..n]);
    };
    let head = String::from_utf8_lossy(&data[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("").to_string();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    Ok((status_line, headers, data[end + 4..].to_vec()))
}

fn read_response(stream: &mut impl Read, head_only: bool) -> Result<Response, Error> {
    let (status_line, headers, rest) = read_head(stream)?;
    let mut parts = status_line.splitn(3, ' ');
    let (version, code, reason) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let status: u16 = match (version.starts_with("HTTP/1."), code.parse()) {
        (true, Ok(status)) => status,
        _ => return Err(Error::Protocol(format!("respuesta no válida: {status_line:?}"))),
    };
    let mut response = Response { status, reason: reason.to_string(), headers, body: Vec::new() };
    if head_only || status / 100 == 1 || status == 204 || status == 304 {
        return Ok(response);
    }
    let mut reader = Buffered { pending: rest, stream };
    let chunked = response.header("Transfer-Encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    if chunked {
        loop {
            let line = reader.line()?;
            let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).map_err(|_| Error::Protocol(format!("tamaño de trozo no válido: {line:?}")))?;
            if size == 0 {
                break;
            }
            if response.body.len() + size > MAX_BODY {
                return Err(Error::Protocol("cuerpo demasiado grande".to_string()));
            }
            reader.exact(size, &mut response.body)?;
            // Fin de línea tras cada trozo.
            reader.line()?;
        }
    } else if let Some(length) = response.header("Content-Length") {
        let length: usize = length.parse().map_err(|_| Error::Protocol(format!("Content-Length no válido: {length:?}")))?;
        if length > MAX_BODY {
            return Err(Error::Protocol("cuerpo demasiado grande".to_string()));
        }
        reader.exact(length, &mut response.body)?;
    } else {
        // Sin longitud: el cuerpo llega hasta que el servidor cierra.
        response.body = reader.pending;
        let mut buffer = [0u8; 4096];
        loop {
            let n = reader.stream.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            if response.body.len() + n > MAX_BODY {
                return Err(Error::Protocol("cuerpo demasiado grande".to_string()));
            }
            response.body.extend_from_slice(&buffer[..n]);
        }
    }
    Ok(response)
}

/// Lector con lo que sobró de leer las cabeceras por delante.
struct Buffered<'a, R: Read> {
    pending: Vec<u8>,
    stream: &'a mut R,
}

impl<R: Read> Buffered<'_, R> {
    fn fill(&mut self) -> Result<(), Error> {
        let mut buffer = [0u8; 4096];
        let n = self.stream.read(&mut buffer)?;
        if n == 0 {
            return Err(Error::Protocol("la conexión se cerró a mitad del cuerpo".to_string()));
        }
        self.pending.extend_from_slice(&buffer[..n]);
        Ok(())
    }

    /// Una línea terminada en CRLF, sin el terminador.
    fn line(&mut self) -> Result<String, Error> {
        loop {
            if let Some(end) = self.pending.windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8_lossy(&self.pending[..end]).into_owned();
                self.pending.drain(..end + 2);
                return Ok(line);
            }
            self.fill()?;
        }
    }

    /// Exactamente `count` bytes, añadidos a `out`.
    fn exact(&mut self, count: usize, out: &mut Vec<u8>) -> Result<(), Error> {
        while self.pending.len() < count {
            self.fill()?;
        }
        out.extend(self.pending.drain(..count));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un "servidor" que entrega la respuesta en trozos de `step` bytes.
    struct Slow<'a>(&'a [u8], usize);

    impl Read for Slow<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.0.len().min(self.1).min(buf.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    #[test]
    fn content_length_chunked_and_until_close() {
        for step in [1, 3, 1000] {
            let r = read_response(&mut Slow(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Uno: dos\r\n\r\nholaXsobra", step), false).unwrap();
            assert_eq!((r.status, r.reason.as_str(), r.body.as_slice(), r.header("x-uno")), (200, "OK", &b"holaX"[..], Some("dos")));
            let r = read_response(&mut Slow(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nhola\r\n6;ext=1\r\n mundo\r\n0\r\n\r\n", step), false).unwrap();
            assert_eq!(r.text(), "hola mundo");
            let r = read_response(&mut Slow(b"HTTP/1.0 404 Not Found\r\n\r\nno esta", step), false).unwrap();
            assert_eq!((r.status, r.reason.as_str(), r.text().as_str()), (404, "Not Found", "no esta"));
        }
        let r = read_response(&mut Slow(b"HTTP/1.1 204 No Content\r\n\r\n", 7), false).unwrap();
        assert!(r.body.is_empty());
        let r = read_response(&mut Slow(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n", 7), true).unwrap();
        assert!(r.body.is_empty());
    }

    #[test]
    fn broken_responses_are_errors() {
        assert!(matches!(read_response(&mut Slow(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\ncorto", 5), false), Err(Error::Protocol(_))));
        assert!(matches!(read_response(&mut Slow(b"hola\r\n\r\n", 5), false), Err(Error::Protocol(_))));
        assert!(matches!(read_response(&mut Slow(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n", 5), false), Err(Error::Protocol(_))));
        assert!(matches!(read_response(&mut Slow(b"HTTP/1.1 200", 5), false), Err(Error::Protocol(_))));
        assert!(matches!(Request::new("GET", "ws://host/"), Err(Error::Url(_))));
        assert!(matches!(get("https://example.com/"), Err(Error::Tls)));
    }
}

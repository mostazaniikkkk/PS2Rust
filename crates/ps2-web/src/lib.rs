//! HTTP/1.1 y WebSocket sobre `std::net`, sin dependencias.
//!
//! ```no_run
//! use ps2_web::{http, ws};
//!
//! let page = http::get("http://example.com/")?;
//! println!("{} {}", page.status, page.text().len());
//!
//! let mut socket = ws::WebSocket::connect("ws://192.168.1.10:9000/chat")?;
//! socket.send_text("hola")?;
//! println!("{:?}", socket.read()?);
//! # Ok::<(), ps2_web::Error>(())
//! ```
//!
//! Sólo usa la biblioteca estándar, así que funciona igual en el PC y en la
//! PlayStation 2. **No hay TLS**: las direcciones `https://` y `wss://` dan
//! [`Error::Tls`].

pub mod http;
mod util;
pub mod ws;

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    /// Fallo de red.
    Io(io::Error),
    /// Venció el plazo de lectura. La conexión sigue siendo válida.
    Timeout,
    /// La dirección no se entiende.
    Url(String),
    /// `https://` o `wss://`: este crate no cifra.
    Tls,
    /// El otro extremo no habla el protocolo como se esperaba.
    Protocol(String),
    /// La conexión WebSocket ya está cerrada.
    Closed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "error de red: {e}"),
            Error::Timeout => write!(f, "plazo de lectura vencido"),
            Error::Url(u) => write!(f, "dirección no válida: {u}"),
            Error::Tls => write!(f, "https:// y wss:// necesitan TLS, que no está disponible"),
            Error::Protocol(p) => write!(f, "error de protocolo: {p}"),
            Error::Closed => write!(f, "la conexión está cerrada"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        match e.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Error::Timeout,
            _ => Error::Io(e),
        }
    }
}

/// Una dirección `esquema://host[:puerto][/ruta]` ya separada.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// Ruta con su consulta; siempre empieza por `/`.
    pub path: String,
}

impl Url {
    pub fn parse(text: &str) -> Result<Url, Error> {
        let bad = || Error::Url(text.to_string());
        let (scheme, rest) = text.split_once("://").ok_or_else(bad)?;
        let scheme = scheme.to_ascii_lowercase();
        let default_port = match scheme.as_str() {
            "http" | "ws" => 80,
            "https" | "wss" => return Err(Error::Tls),
            _ => return Err(bad()),
        };
        let rest = rest.split('#').next().unwrap_or("");
        let (authority, path) = match rest.find(['/', '?']) {
            Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_string()),
            Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
            None => (rest, "/".to_string()),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().map_err(|_| bad())?),
            None => (authority, default_port),
        };
        if host.is_empty() || host.contains(['@', ' ', '[']) {
            return Err(bad());
        }
        Ok(Url { scheme, host: host.to_string(), port, path })
    }

    /// Valor de la cabecera `Host`.
    pub fn host_header(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u = Url::parse("http://example.com").unwrap();
        assert_eq!((u.scheme.as_str(), u.host.as_str(), u.port, u.path.as_str()), ("http", "example.com", 80, "/"));
        let u = Url::parse("WS://10.0.0.2:9000/chat?sala=1#ancla").unwrap();
        assert_eq!((u.scheme.as_str(), u.host.as_str(), u.port, u.path.as_str()), ("ws", "10.0.0.2", 9000, "/chat?sala=1"));
        assert_eq!(u.host_header(), "10.0.0.2:9000");
        assert_eq!(Url::parse("http://host?x=1").unwrap().path, "/?x=1");
        assert!(matches!(Url::parse("https://example.com"), Err(Error::Tls)));
        assert!(matches!(Url::parse("wss://example.com"), Err(Error::Tls)));
        assert!(matches!(Url::parse("ftp://example.com"), Err(Error::Url(_))));
        assert!(matches!(Url::parse("example.com"), Err(Error::Url(_))));
        assert!(matches!(Url::parse("http://host:puerto/"), Err(Error::Url(_))));
    }
}

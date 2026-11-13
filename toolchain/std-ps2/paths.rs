//! Directorio actual y ruta del ejecutable.
//!
//! El directorio inicial es el del ELF: `host:` si se cargó desde el PC
//! (PCSX2 o ps2link), `cdrom0:\` desde un disco, `mc0:/CARPETA/` desde la
//! tarjeta de memoria.

use crate::ffi::OsString;
use crate::io;
use crate::os::ps2::ffi::{OsStrExt, OsStringExt};
use crate::path::{Path, PathBuf};
use crate::sync::Mutex;

static CWD: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Directorio de una ruta de la PS2 (`dispositivo:carpeta/`).
fn initial_dir() -> PathBuf {
    let Some(exe) = crate::env::args_os().next() else {
        return PathBuf::from("host:");
    };
    let bytes = exe.as_bytes();
    let Some(colon) = bytes.iter().position(|&b| b == b':') else {
        return PathBuf::from("host:");
    };
    if bytes.starts_with(b"host") {
        // La raíz de host: ya es la carpeta del ELF.
        return PathBuf::from(OsString::from_vec(bytes[..=colon].to_vec()));
    }
    let end = bytes.iter().rposition(|&b| b == b'/' || b == b'\\').map(|i| i + 1).unwrap_or(colon + 1);
    PathBuf::from(OsString::from_vec(bytes[..end.max(colon + 1)].to_vec()))
}

pub fn getcwd() -> io::Result<PathBuf> {
    let mut cwd = CWD.lock().unwrap();
    Ok(cwd.get_or_insert_with(initial_dir).clone())
}

pub fn chdir(p: &Path) -> io::Result<()> {
    let target = crate::sys::fs::ps2_resolve(p);
    let attr = crate::sys::fs::metadata(&target)?;
    if !attr.file_type().is_dir() {
        return Err(io::const_error!(io::ErrorKind::NotADirectory, "not a directory"));
    }
    let mut s = target.into_os_string().into_vec();
    if !matches!(s.last(), Some(b':' | b'/' | b'\\')) {
        s.push(b'/');
    }
    *CWD.lock().unwrap() = Some(PathBuf::from(OsString::from_vec(s)));
    Ok(())
}

pub fn current_exe() -> io::Result<PathBuf> {
    crate::env::args_os()
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| io::const_error!(io::ErrorKind::NotFound, "no argv[0]"))
}

pub fn temp_dir() -> PathBuf {
    getcwd().unwrap_or_else(|_| PathBuf::from("host:"))
}

pub fn home_dir() -> Option<PathBuf> {
    None
}

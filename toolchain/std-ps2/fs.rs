//! `std::fs` sobre el servidor FILEIO del IOP.
//!
//! Las rutas llevan el dispositivo delante: `host:datos.txt`, `mc0:/SAVE/x`,
//! `cdrom0:\DATOS.BIN;1`. Una ruta sin dispositivo es relativa al directorio
//! actual (al arrancar, el del ELF). Los módulos de tarjeta de memoria se
//! cargan solos la primera vez que se usa `mc0:` o `mc1:`.

use crate::ffi::OsString;
use crate::fmt;
use crate::fs::TryLockError;
use crate::hash::{Hash, Hasher};
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut, SeekFrom};
use crate::os::ps2::ffi::{OsStrExt, OsStringExt};
use crate::path::{Path, PathBuf};
pub use crate::sys::fs::common::Dir;
use crate::sys::os::{fileio, loadfile};
use crate::sys::time::SystemTime;
use crate::sys::unsupported;

const ENOENT: i32 = 2;
const EEXIST: i32 = 17;
const ENOTDIR: i32 = 20;
const EISDIR: i32 = 21;
const ENOTEMPTY: i32 = 39;

fn os_err(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code.unsigned_abs() as i32)
}

/// Ruta completa (con dispositivo) que entiende el IOP.
pub fn ps2_resolve(p: &Path) -> PathBuf {
    let bytes = p.as_os_str().as_bytes();
    let has_device = match bytes.iter().position(|&b| b == b':') {
        Some(i) => !bytes[..i].contains(&b'/') && !bytes[..i].contains(&b'\\'),
        None => false,
    };
    if has_device {
        return p.to_path_buf();
    }
    let cwd = crate::sys::paths::getcwd().unwrap_or_else(|_| PathBuf::from("host:"));
    let mut out = cwd.into_os_string().into_vec();
    let rel = bytes.strip_prefix(b"./").unwrap_or(bytes);
    if !matches!(out.last(), Some(b':' | b'/' | b'\\')) && !rel.is_empty() {
        out.push(b'/');
    }
    out.extend_from_slice(rel);
    PathBuf::from(OsString::from_vec(out))
}

fn native(p: &Path) -> io::Result<Vec<u8>> {
    let full = ps2_resolve(p);
    let bytes = full.as_os_str().as_bytes().to_vec();
    if bytes.len() > fileio::PATH_MAX {
        return Err(io::const_error!(io::ErrorKind::InvalidFilename, "path too long for the IOP"));
    }
    if bytes.starts_with(b"mc") && !loadfile::ensure_memory_card() {
        return Err(io::const_error!(io::ErrorKind::NotFound, "memory card modules not available"));
    }
    Ok(bytes)
}

/// Directorio padre de una ruta nativa (`host:a/b` -> `host:a`); `None` si
/// el padre es la raíz del dispositivo.
fn parent_native(path: &[u8]) -> Option<&[u8]> {
    let colon = path.iter().position(|&b| b == b':')?;
    let rest = &path[colon + 1..];
    let trimmed = rest.strip_suffix(b"/").or_else(|| rest.strip_suffix(b"\\")).unwrap_or(rest);
    let sep = trimmed.iter().rposition(|&b| b == b'/' || b == b'\\')?;
    if sep == 0 {
        return None;
    }
    Some(&path[..colon + 1 + sep])
}

/// Algunos dispositivos (y el HLE de PCSX2) responden EIO cuando falta el
/// directorio padre: se traduce a NotFound, que es lo que espera la std
/// (por ejemplo `create_dir_all`).
fn create_err(path: &[u8], code: i32) -> io::Error {
    if let Some(parent) = parent_native(path) {
        match fileio::getstat(parent) {
            Err(_) => return os_err(ENOENT),
            Ok(st) if !st.is_dir() => return os_err(ENOTDIR),
            _ => {}
        }
    }
    os_err(code)
}

fn stat_native(path: &[u8]) -> io::Result<fileio::Stat> {
    fileio::getstat(path).map_err(|_| os_err(ENOENT))
}

// ------------------------------------------------------------- metadatos

#[derive(Clone)]
pub struct FileAttr {
    stat: fileio::Stat,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FilePermissions {
    readonly: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FileType {
    dir: bool,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct FileTimes {}

fn ps2_time(t: &[u8; 8]) -> io::Result<SystemTime> {
    // {sin uso, segundo, minuto, hora, día, mes, año (u16)}, hora de Japón.
    let mut year = t[6] as u32 | (t[7] as u32) << 8;
    if year < 1000 {
        year += 1900; // el HLE de PCSX2 guarda años desde 1900
    }
    crate::sys::time::from_ps2_datetime(year, t[5] as u32, t[4] as u32, t[3] as u32, t[2] as u32, t[1] as u32)
        .ok_or_else(|| io::const_error!(io::ErrorKind::InvalidData, "invalid file time"))
}

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.stat.size
    }

    pub fn perm(&self) -> FilePermissions {
        // Bits bajos del modo: 1 = lectura, 2 = escritura, 4 = ejecución.
        FilePermissions { readonly: self.stat.mode & 0x2 == 0 && self.stat.mode & 0xF000 == 0 }
    }

    pub fn file_type(&self) -> FileType {
        FileType { dir: self.stat.is_dir() }
    }

    pub fn modified(&self) -> io::Result<SystemTime> {
        ps2_time(&self.stat.mtime)
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        ps2_time(&self.stat.atime)
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        ps2_time(&self.stat.ctime)
    }
}

impl FilePermissions {
    pub fn readonly(&self) -> bool {
        self.readonly
    }

    pub fn set_readonly(&mut self, readonly: bool) {
        self.readonly = readonly;
    }
}

impl FileTimes {
    pub fn set_accessed(&mut self, _t: SystemTime) {}
    pub fn set_modified(&mut self, _t: SystemTime) {}
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.dir
    }

    pub fn is_file(&self) -> bool {
        !self.dir
    }

    pub fn is_symlink(&self) -> bool {
        false
    }
}

// ------------------------------------------------------------ directorios

pub struct ReadDir {
    entries: crate::vec::IntoIter<DirEntry>,
    root: PathBuf,
}

pub struct DirEntry {
    root: PathBuf,
    name: OsString,
    stat: fileio::Stat,
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.root, f)
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        self.entries.next().map(Ok)
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.root.join(&self.name)
    }

    pub fn file_name(&self) -> OsString {
        self.name.clone()
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        Ok(FileAttr { stat: self.stat })
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(FileType { dir: self.stat.is_dir() })
    }
}

pub fn readdir(p: &Path) -> io::Result<ReadDir> {
    let path = native(p)?;
    let mut dir_path = path.clone();
    // `dopen` necesita la raíz explícita del dispositivo (`mc0:/`).
    if dir_path.last() == Some(&b':') && dir_path.starts_with(b"mc") {
        dir_path.push(b'/');
    }
    let st = stat_native(&path).ok();
    if let Some(st) = st {
        if !st.is_dir() {
            return Err(os_err(ENOTDIR));
        }
    }
    let fd = fileio::dopen(&dir_path).map_err(|_| os_err(ENOENT))?;
    let mut entries = Vec::new();
    let mut name = [0u8; 256];
    loop {
        match fileio::dread(fd, &mut name) {
            Ok(Some((stat, len))) => {
                let n = &name[..len];
                if n == b"." || n == b".." {
                    continue;
                }
                entries.push(DirEntry { root: p.to_path_buf(), name: OsString::from_vec(n.to_vec()), stat });
            }
            Ok(None) => break,
            Err(e) => {
                fileio::dclose(fd);
                return Err(os_err(e));
            }
        }
    }
    fileio::dclose(fd);
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ReadDir { entries: entries.into_iter(), root: p.to_path_buf() })
}

#[derive(Debug)]
pub struct DirBuilder {}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder {}
    }

    pub fn mkdir(&self, p: &Path) -> io::Result<()> {
        let path = native(p)?;
        if stat_native(&path).is_ok() {
            return Err(os_err(EEXIST));
        }
        fileio::mkdir(&path).map_err(|e| create_err(&path, e))
    }
}

// ----------------------------------------------------------------- archivos

#[derive(Clone, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions { read: false, write: false, append: false, truncate: false, create: false, create_new: false }
    }

    pub fn read(&mut self, read: bool) {
        self.read = read;
    }

    pub fn write(&mut self, write: bool) {
        self.write = write;
    }

    pub fn append(&mut self, append: bool) {
        self.append = append;
    }

    pub fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }

    pub fn create(&mut self, create: bool) {
        self.create = create;
    }

    pub fn create_new(&mut self, create_new: bool) {
        self.create_new = create_new;
    }

    fn flags(&self) -> io::Result<i32> {
        let writable = self.write || self.append;
        let mut flags = match (self.read, writable) {
            (true, false) => fileio::O_RDONLY,
            (false, true) => fileio::O_WRONLY,
            (true, true) => fileio::O_RDWR,
            (false, false) => return Err(io::const_error!(io::ErrorKind::InvalidInput, "invalid access mode")),
        };
        if (self.truncate || self.create || self.create_new) && !writable {
            return Err(io::const_error!(io::ErrorKind::InvalidInput, "creating or truncating requires write access"));
        }
        if self.truncate && self.append {
            return Err(io::const_error!(io::ErrorKind::InvalidInput, "truncate and append are incompatible"));
        }
        if self.create || self.create_new {
            flags |= fileio::O_CREAT;
        }
        if self.truncate {
            flags |= fileio::O_TRUNC;
        }
        if self.append {
            flags |= fileio::O_APPEND;
        }
        Ok(flags)
    }
}

pub struct File {
    fd: i32,
    path: Vec<u8>,
    append: bool,
}

impl File {
    pub fn open(p: &Path, opts: &OpenOptions) -> io::Result<File> {
        let flags = opts.flags()?;
        let path = native(p)?;
        let existing = stat_native(&path).ok();
        match existing {
            Some(st) if st.is_dir() && flags & 3 != fileio::O_RDONLY => return Err(os_err(EISDIR)),
            Some(_) if opts.create_new => return Err(os_err(EEXIST)),
            None if flags & fileio::O_CREAT == 0 => return Err(os_err(ENOENT)),
            _ => {}
        }
        let fd = fileio::open(&path, flags).map_err(|e| create_err(&path, e))?;
        let file = File { fd, path, append: opts.append };
        if opts.append {
            fileio::lseek(fd, 0, fileio::SEEK_END).map_err(os_err)?;
        }
        Ok(file)
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        Ok(FileAttr { stat: stat_native(&self.path)? })
    }

    pub fn fsync(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn datasync(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn lock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn lock_shared(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn try_lock(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(crate::sys::unsupported_err()))
    }

    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(crate::sys::unsupported_err()))
    }

    pub fn unlock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn truncate(&self, _size: u64) -> io::Result<()> {
        unsupported()
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        fileio::read(self.fd, buf).map_err(os_err)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        crate::io::default_read_vectored(|b| self.read(b), bufs)
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        crate::io::default_read_buf(|b| self.read(b), cursor)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        if self.append {
            fileio::lseek(self.fd, 0, fileio::SEEK_END).map_err(os_err)?;
        }
        fileio::write(self.fd, buf).map_err(os_err)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        crate::io::default_write_vectored(|b| self.write(b), bufs)
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(n) => (i32::try_from(n).map_err(|_| os_err(22))?, fileio::SEEK_SET),
            SeekFrom::Current(n) => (i32::try_from(n).map_err(|_| os_err(22))?, fileio::SEEK_CUR),
            SeekFrom::End(n) => (i32::try_from(n).map_err(|_| os_err(22))?, fileio::SEEK_END),
        };
        fileio::lseek(self.fd, offset, whence).map(u64::from).map_err(os_err)
    }

    pub fn size(&self) -> Option<io::Result<u64>> {
        Some(self.file_attr().map(|a| a.size()))
    }

    pub fn tell(&self) -> io::Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    pub fn duplicate(&self) -> io::Result<File> {
        unsupported()
    }

    pub fn set_permissions(&self, _perm: FilePermissions) -> io::Result<()> {
        Ok(())
    }

    pub fn set_times(&self, _times: FileTimes) -> io::Result<()> {
        unsupported()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        fileio::close(self.fd);
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File").field("fd", &self.fd).field("path", &String::from_utf8_lossy(&self.path)).finish()
    }
}

// ------------------------------------------------------- operaciones sueltas

pub fn unlink(p: &Path) -> io::Result<()> {
    let path = native(p)?;
    let st = stat_native(&path)?;
    if st.is_dir() {
        return Err(os_err(EISDIR));
    }
    let _ = fileio::remove(&path);
    // El FILEIO de la BIOS, tras borrar, sigue por error hasta `mkdir` y deja
    // un directorio vacío con el mismo nombre: se deshace.
    if let Ok(after) = fileio::getstat(&path) {
        if after.is_dir() {
            let _ = fileio::rmdir(&path);
        } else {
            return Err(io::const_error!(io::ErrorKind::Other, "the device could not remove the file"));
        }
    }
    Ok(())
}

pub fn rename(old: &Path, new: &Path) -> io::Result<()> {
    // FILEIO no sabe renombrar: copia y borra (no es atómico).
    let from = native(old)?;
    let st = stat_native(&from)?;
    if st.is_dir() {
        return unsupported();
    }
    if let Ok(dest) = stat_native(&native(new)?) {
        if dest.is_dir() {
            return Err(os_err(EISDIR));
        }
    }
    copy(old, new)?;
    unlink(old)
}

pub fn set_perm(_p: &Path, _perm: FilePermissions) -> io::Result<()> {
    unsupported()
}

pub fn set_times(_p: &Path, _times: FileTimes) -> io::Result<()> {
    unsupported()
}

pub fn set_times_nofollow(_p: &Path, _times: FileTimes) -> io::Result<()> {
    unsupported()
}

pub fn rmdir(p: &Path) -> io::Result<()> {
    let path = native(p)?;
    let st = stat_native(&path)?;
    if !st.is_dir() {
        return Err(os_err(ENOTDIR));
    }
    if readdir(p)?.next().is_some() {
        return Err(os_err(ENOTEMPTY));
    }
    fileio::rmdir(&path).map_err(os_err)
}

pub fn remove_dir_all(p: &Path) -> io::Result<()> {
    let st = stat_native(&native(p)?)?;
    if !st.is_dir() {
        return Err(os_err(ENOTDIR));
    }
    for entry in readdir(p)? {
        let entry = entry?;
        if entry.stat.is_dir() {
            remove_dir_all(&entry.path())?;
        } else {
            unlink(&entry.path())?;
        }
    }
    rmdir(p)
}

pub fn exists(p: &Path) -> io::Result<bool> {
    Ok(stat_native(&native(p)?).is_ok())
}

pub fn readlink(_p: &Path) -> io::Result<PathBuf> {
    unsupported()
}

pub fn symlink(_original: &Path, _link: &Path) -> io::Result<()> {
    unsupported()
}

pub fn link(_src: &Path, _dst: &Path) -> io::Result<()> {
    unsupported()
}

pub fn stat(p: &Path) -> io::Result<FileAttr> {
    Ok(FileAttr { stat: stat_native(&native(p)?)? })
}

pub fn lstat(p: &Path) -> io::Result<FileAttr> {
    stat(p)
}

pub fn canonicalize(p: &Path) -> io::Result<PathBuf> {
    let full = ps2_resolve(p);
    stat_native(&native(&full)?)?;
    Ok(full)
}

pub fn copy(from: &Path, to: &Path) -> io::Result<u64> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    let src = File::open(from, &opts)?;
    if src.file_attr()?.file_type().is_dir() {
        return Err(io::const_error!(io::ErrorKind::InvalidInput, "the source path is not a file"));
    }
    let mut opts = OpenOptions::new();
    opts.write(true);
    opts.create(true);
    opts.truncate(true);
    let dst = File::open(to, &opts)?;
    let mut buf = vec![0u8; fileio::CHUNK];
    let mut total = 0u64;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let mut written = 0;
        while written < n {
            let w = dst.write(&buf[written..n])?;
            if w == 0 {
                return Err(io::Error::WRITE_ALL_EOF);
            }
            written += w;
        }
        total += n as u64;
    }
    Ok(total)
}

impl Hash for FileAttr {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.stat.size.hash(h);
        self.stat.mode.hash(h);
    }
}

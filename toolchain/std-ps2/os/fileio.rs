//! Cliente del servidor FILEIO de la BIOS (RPC 0x80000001 en el IOP).
//!
//! Da acceso a los dispositivos que conoce el IOP: `host:` (el PC, con
//! PCSX2 o ps2link), `cdrom0:` (el disco), `mc0:`/`mc1:` (tarjetas de memoria,
//! si sus módulos están cargados) y `rom0:`.
//!
//! Todos los datos pasan por búferes propios alineados y sin caché, así que
//! el código de arriba puede usar memoria normal sin preocuparse del DMA.

use super::kernel::{self, uncached};
use super::sif::{self, Client};
use core::ptr::{addr_of, addr_of_mut, copy_nonoverlapping, read_volatile, write_bytes, write_volatile};

const SID_FILEIO: u32 = 0x8000_0001;

const F_OPEN: u32 = 0;
const F_CLOSE: u32 = 1;
const F_READ: u32 = 2;
const F_WRITE: u32 = 3;
const F_LSEEK: u32 = 4;
const F_REMOVE: u32 = 6;
const F_MKDIR: u32 = 7;
const F_RMDIR: u32 = 8;
const F_DOPEN: u32 = 9;
const F_DCLOSE: u32 = 10;
const F_DREAD: u32 = 11;
const F_GETSTAT: u32 = 12;

pub const O_RDONLY: i32 = 0x0001;
pub const O_WRONLY: i32 = 0x0002;
pub const O_RDWR: i32 = 0x0003;
pub const O_APPEND: i32 = 0x0100;
pub const O_CREAT: i32 = 0x0200;
pub const O_TRUNC: i32 = 0x0400;
pub const O_EXCL: i32 = 0x0800;

pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

/// Longitud máxima de una ruta, sin el terminador.
pub const PATH_MAX: usize = 255;

/// Tamaño de cada trozo de lectura o escritura.
pub const CHUNK: usize = 16 * 1024;

#[repr(C, align(64))]
struct Arg([u8; 512]);
#[repr(C, align(64))]
struct Small([u8; 64]);
#[repr(C, align(64))]
struct Bounce([u8; CHUNK]);
#[repr(C, align(64))]
struct Dirent([u8; 320]);

static mut CLIENT: Client = Client::new();
static mut ARG: Arg = Arg([0; 512]);
static mut RESULT: Small = Small([0; 64]);
static mut READ_DATA: Arg = Arg([0; 512]);
static mut STAT: Small = Small([0; 64]);
static mut DIRENT: Dirent = Dirent([0; 320]);
static mut BOUNCE: Bounce = Bounce([0; CHUNK]);
static mut LOCK: i32 = -1;

/// Metadatos de un archivo (estructura `io_stat_t` del IOP).
#[derive(Clone, Copy, Debug, Default)]
pub struct Stat {
    pub mode: u32,
    pub attr: u32,
    pub size: u64,
    pub ctime: [u8; 8],
    pub atime: [u8; 8],
    pub mtime: [u8; 8],
}

impl Stat {
    pub fn is_dir(&self) -> bool {
        // ioman antiguo: 0x20 = directorio; iomanX: tipo 0x1000 en los bits altos.
        (self.mode & 0xF000 == 0x1000) || (self.mode & 0xF000 == 0 && self.mode & 0x20 != 0)
    }

    pub fn is_file(&self) -> bool {
        (self.mode & 0xF000 == 0x2000) || (self.mode & 0xF000 == 0 && self.mode & 0x10 != 0)
    }
}

fn arg() -> *mut u8 {
    uncached(unsafe { addr_of!(ARG.0) } as *const u8)
}

fn result_ptr() -> *mut u8 {
    uncached(unsafe { addr_of!(RESULT.0) } as *const u8)
}

fn put_u32(p: *mut u8, offset: usize, v: u32) {
    unsafe { write_volatile(p.add(offset) as *mut u32, v) }
}

fn get_u32(p: *const u8, offset: usize) -> u32 {
    unsafe { read_volatile(p.add(offset) as *const u32) }
}

fn put_path(p: *mut u8, offset: usize, path: &[u8]) -> Result<(), i32> {
    if path.len() > PATH_MAX || path.contains(&0) {
        return Err(-22); // EINVAL
    }
    unsafe {
        write_bytes(p.add(offset), 0, PATH_MAX + 1);
        copy_nonoverlapping(path.as_ptr(), p.add(offset), path.len());
    }
    Ok(())
}

/// Ejecuta `f` con el cliente ligado y en exclusiva.
fn with_fio<R>(f: impl FnOnce(&mut Client) -> R) -> R {
    sif::init();
    unsafe {
        if read_volatile(addr_of!(LOCK)) < 0 {
            let s = kernel::create_sema(1, 1);
            write_volatile(addr_of_mut!(LOCK), s);
        }
        kernel::wait_sema(LOCK);
        let client = &mut *addr_of_mut!(CLIENT);
        if !client.is_bound() {
            sif::bind(client, SID_FILEIO, u32::MAX);
        }
        let r = f(client);
        kernel::signal_sema(LOCK);
        r
    }
}

/// Olvida lo que se sabía del IOP (tras reiniciarlo).
pub fn forget() {
    unsafe { *addr_of_mut!(CLIENT) = Client::new() };
}

/// Llamada con argumentos en `ARG` y un entero de respuesta.
fn simple_call(client: &mut Client, fno: u32, send_size: usize) -> i32 {
    let res = result_ptr();
    put_u32(res, 0, 0);
    sif::call(client, fno, arg(), send_size as u32, res, 16);
    get_u32(res, 0) as i32
}

pub fn open(path: &[u8], flags: i32) -> Result<i32, i32> {
    with_fio(|c| {
        let a = arg();
        put_u32(a, 0, flags as u32);
        put_path(a, 4, path)?;
        let fd = simple_call(c, F_OPEN, 4 + PATH_MAX + 1);
        if fd < 0 { Err(fd) } else { Ok(fd) }
    })
}

pub fn close(fd: i32) -> i32 {
    with_fio(|c| {
        put_u32(arg(), 0, fd as u32);
        simple_call(c, F_CLOSE, 4)
    })
}

/// Lee hasta `buf.len()` bytes (en trozos de [`CHUNK`]).
pub fn read(fd: i32, buf: &mut [u8]) -> Result<usize, i32> {
    let mut total = 0;
    while total < buf.len() {
        let want = (buf.len() - total).min(CHUNK);
        let got = read_chunk(fd, &mut buf[total..total + want])?;
        total += got;
        if got < want {
            break;
        }
    }
    Ok(total)
}

fn read_chunk(fd: i32, out: &mut [u8]) -> Result<usize, i32> {
    with_fio(|c| unsafe {
        let bounce = uncached(addr_of!(BOUNCE.0) as *const u8);
        let rd = uncached(addr_of!(READ_DATA.0) as *const u8);
        write_bytes(rd, 0, 64);
        let a = arg();
        put_u32(a, 0, fd as u32);
        put_u32(a, 4, bounce as u32);
        put_u32(a, 8, out.len() as u32);
        put_u32(a, 12, rd as u32);
        let n = simple_call(c, F_READ, 16);
        if n < 0 {
            return Err(n);
        }
        // El IOP deja la cabeza y la cola no alineadas a 16 bytes en
        // READ_DATA: {size1, size2, dest1, dest2, buf1[16], buf2[16]}.
        let (size1, size2) = (get_u32(rd, 0) as usize, get_u32(rd, 4) as usize);
        let (dest1, dest2) = (get_u32(rd, 8) as usize, get_u32(rd, 12) as usize);
        if size1 > 0 && dest1 != 0 {
            copy_nonoverlapping(rd.add(16), uncached(dest1 as *const u8), size1.min(16));
        }
        if size2 > 0 && dest2 != 0 {
            copy_nonoverlapping(rd.add(32), uncached(dest2 as *const u8), size2.min(16));
        }
        let n = (n as usize).min(out.len());
        copy_nonoverlapping(bounce as *const u8, out.as_mut_ptr(), n);
        Ok(n)
    })
}

pub fn write(fd: i32, data: &[u8]) -> Result<usize, i32> {
    let mut total = 0;
    while total < data.len() {
        let len = (data.len() - total).min(CHUNK);
        let n = write_chunk(fd, &data[total..total + len])?;
        total += n;
        if n < len {
            break;
        }
    }
    Ok(total)
}

fn write_chunk(fd: i32, data: &[u8]) -> Result<usize, i32> {
    with_fio(|c| unsafe {
        let bounce = uncached(addr_of!(BOUNCE.0) as *const u8);
        copy_nonoverlapping(data.as_ptr(), bounce, data.len());
        let a = arg();
        write_bytes(a, 0, 32);
        put_u32(a, 0, fd as u32);
        put_u32(a, 4, bounce as u32);
        put_u32(a, 8, data.len() as u32);
        put_u32(a, 12, 0); // mis: el búfer está alineado
        let n = simple_call(c, F_WRITE, 32);
        if n < 0 { Err(n) } else { Ok(n as usize) }
    })
}

pub fn lseek(fd: i32, offset: i32, whence: i32) -> Result<u32, i32> {
    with_fio(|c| {
        let a = arg();
        put_u32(a, 0, fd as u32);
        put_u32(a, 4, offset as u32);
        put_u32(a, 8, whence as u32);
        let r = simple_call(c, F_LSEEK, 12);
        if r < 0 { Err(r) } else { Ok(r as u32) }
    })
}

fn path_call(fno: u32, path: &[u8]) -> Result<i32, i32> {
    with_fio(|c| {
        put_path(arg(), 0, path)?;
        let r = simple_call(c, fno, PATH_MAX + 1);
        if r < 0 { Err(r) } else { Ok(r) }
    })
}

pub fn remove(path: &[u8]) -> Result<(), i32> {
    path_call(F_REMOVE, path).map(drop)
}

pub fn mkdir(path: &[u8]) -> Result<(), i32> {
    path_call(F_MKDIR, path).map(drop)
}

pub fn rmdir(path: &[u8]) -> Result<(), i32> {
    path_call(F_RMDIR, path).map(drop)
}

pub fn dopen(path: &[u8]) -> Result<i32, i32> {
    path_call(F_DOPEN, path)
}

pub fn dclose(fd: i32) -> i32 {
    with_fio(|c| {
        put_u32(arg(), 0, fd as u32);
        simple_call(c, F_DCLOSE, 4)
    })
}

fn parse_stat(p: *const u8) -> Stat {
    let mut s = Stat {
        mode: get_u32(p, 0),
        attr: get_u32(p, 4),
        size: get_u32(p, 8) as u64 | (get_u32(p, 36) as u64) << 32,
        ..Stat::default()
    };
    unsafe {
        for i in 0..8 {
            s.ctime[i] = read_volatile(p.add(12 + i));
            s.atime[i] = read_volatile(p.add(20 + i));
            s.mtime[i] = read_volatile(p.add(28 + i));
        }
    }
    s
}

/// Siguiente entrada del directorio: `Ok(None)` al terminar. El nombre se
/// copia en `name` y se devuelve su longitud.
pub fn dread(fd: i32, name: &mut [u8; 256]) -> Result<Option<(Stat, usize)>, i32> {
    with_fio(|c| unsafe {
        let d = uncached(addr_of!(DIRENT.0) as *const u8);
        write_bytes(d, 0, 320);
        let a = arg();
        put_u32(a, 0, fd as u32);
        put_u32(a, 4, d as u32);
        let r = simple_call(c, F_DREAD, 8);
        if r < 0 {
            return Err(r);
        }
        if r == 0 {
            return Ok(None);
        }
        let stat = parse_stat(d);
        let mut len = 0;
        while len < 255 {
            let b = read_volatile(d.add(40 + len));
            if b == 0 {
                break;
            }
            name[len] = b;
            len += 1;
        }
        Ok(Some((stat, len)))
    })
}

pub fn getstat(path: &[u8]) -> Result<Stat, i32> {
    with_fio(|c| unsafe {
        let s = uncached(addr_of!(STAT.0) as *const u8);
        write_bytes(s, 0, 64);
        let a = arg();
        put_u32(a, 0, s as u32);
        put_path(a, 4, path)?;
        let r = simple_call(c, F_GETSTAT, 4 + PATH_MAX + 1);
        if r < 0 { Err(r) } else { Ok(parse_stat(s)) }
    })
}

/// ¿Está ligado ya el cliente? (Para diagnósticos.)
pub fn is_ready() -> bool {
    unsafe { (*addr_of!(CLIENT)).is_bound() }
}

//! Argumentos del programa: los rellena el kernel en `SetupThread`
//! (`argv[0]` es la ruta del ELF, por ejemplo `host:juego.elf`).

pub use super::common::Args;
use crate::ffi::{CStr, OsString};
use crate::os::ps2::ffi::OsStringExt;
use crate::sync::atomic::{AtomicIsize, AtomicPtr, Ordering};

static ARGC: AtomicIsize = AtomicIsize::new(0);
static ARGV: AtomicPtr<*const u8> = AtomicPtr::new(crate::ptr::null_mut());

pub unsafe fn init(argc: isize, argv: *const *const u8) {
    ARGC.store(argc, Ordering::Relaxed);
    ARGV.store(argv as *mut _, Ordering::Relaxed);
}

pub fn args() -> Args {
    let argc = ARGC.load(Ordering::Relaxed);
    let argv = ARGV.load(Ordering::Relaxed);
    let mut out = Vec::new();
    if !argv.is_null() {
        for i in 0..argc.max(0) as usize {
            let p = unsafe { *argv.add(i) };
            if p.is_null() {
                break;
            }
            let bytes = unsafe { CStr::from_ptr(p.cast()) }.to_bytes().to_vec();
            out.push(OsString::from_vec(bytes));
        }
    }
    Args::new(out)
}

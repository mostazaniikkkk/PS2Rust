//! Claves de almacenamiento por hilo (`thread_local!`).
//!
//! El kernel no tiene TLS: cada hilo tiene una tabla de punteros, indexada
//! por su id del kernel, que se crea al primer uso.

use crate::alloc::{GlobalAlloc, Layout, System};
use crate::ptr;
use crate::sync::atomic::{AtomicUsize, Ordering};
use crate::sys::os::kernel;

pub type Key = usize;
pub type Dtor = unsafe extern "C" fn(*mut u8);

const MAX_KEYS: usize = 128;
const MAX_THREADS: usize = 256;

static NEXT_KEY: AtomicUsize = AtomicUsize::new(1);
static mut DTORS: [Option<Dtor>; MAX_KEYS] = [None; MAX_KEYS];
static mut TABLES: [*mut *mut u8; MAX_THREADS] = [ptr::null_mut(); MAX_THREADS];

fn layout() -> Layout {
    Layout::array::<*mut u8>(MAX_KEYS).unwrap()
}

fn table() -> *mut *mut u8 {
    let tid = kernel::thread_id() as usize % MAX_THREADS;
    unsafe {
        let t = TABLES[tid];
        if !t.is_null() {
            return t;
        }
        let t = System.alloc_zeroed(layout()) as *mut *mut u8;
        if t.is_null() {
            crate::sys::abort_internal();
        }
        TABLES[tid] = t;
        t
    }
}

pub fn create(dtor: Option<Dtor>) -> Key {
    let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
    if key >= MAX_KEYS {
        rtabort!("too many thread-local keys on the PlayStation 2");
    }
    unsafe { DTORS[key] = dtor };
    key
}

#[inline]
pub unsafe fn set(key: Key, value: *mut u8) {
    unsafe { *table().add(key) = value };
}

#[inline]
pub unsafe fn get(key: Key) -> *mut u8 {
    unsafe { *table().add(key) }
}

pub unsafe fn destroy(_key: Key) {}

/// Ejecuta los destructores del hilo actual y libera su tabla.
pub unsafe fn destroy_tls() {
    let tid = kernel::thread_id() as usize % MAX_THREADS;
    let t = unsafe { TABLES[tid] };
    if t.is_null() {
        return;
    }
    // Un destructor puede volver a llenar otra clave: varias pasadas.
    for _ in 0..5 {
        let mut any = false;
        for key in 1..MAX_KEYS {
            let value = unsafe { *t.add(key) };
            if value.is_null() {
                continue;
            }
            if let Some(dtor) = unsafe { DTORS[key] } {
                unsafe {
                    *t.add(key) = ptr::null_mut();
                    dtor(value);
                }
                any = true;
            }
        }
        if !any {
            break;
        }
    }
    unsafe {
        TABLES[tid] = ptr::null_mut();
        System.dealloc(t as *mut u8, layout());
    }
}

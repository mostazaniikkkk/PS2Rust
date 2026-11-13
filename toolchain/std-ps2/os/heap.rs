//! Asignador de memoria: lista de bloques libres ordenada por dirección,
//! primer ajuste, con fusión de vecinos al liberar.
//!
//! Ocupa desde el final del programa hasta la base de la pila del hilo
//! principal. Las operaciones van en sección crítica, así que se puede usar
//! desde cualquier hilo.

use super::kernel;
use core::ptr;

const UNIT: usize = 16;

#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

#[repr(C)]
struct Header {
    start: usize,
    size: usize,
}

static mut HEAD: *mut FreeBlock = ptr::null_mut();
static mut TOTAL: usize = 0;
static mut FREE: usize = 0;

/// Registra la región `[start, end)` como heap. Llamar una sola vez.
pub unsafe fn init(start: usize, end: usize) {
    let start = (start + UNIT - 1) & !(UNIT - 1);
    let end = end & !(UNIT - 1);
    if end <= start + 2 * UNIT {
        return;
    }
    unsafe {
        let block = start as *mut FreeBlock;
        (*block).size = end - start;
        (*block).next = ptr::null_mut();
        HEAD = block;
        TOTAL = end - start;
        FREE = end - start;
    }
}

pub fn total_bytes() -> usize {
    unsafe { TOTAL }
}

pub fn free_bytes() -> usize {
    unsafe { FREE }
}

pub unsafe fn alloc(size: usize, align: usize) -> *mut u8 {
    kernel::critical(|| unsafe { alloc_inner(size, align) })
}

unsafe fn alloc_inner(size: usize, align: usize) -> *mut u8 {
    let align = align.max(UNIT);
    let slack = align - UNIT;
    let Some(need) = size.checked_add(UNIT + slack + UNIT - 1).map(|n| n & !(UNIT - 1)) else {
        return ptr::null_mut();
    };
    unsafe {
        let mut prev: *mut FreeBlock = ptr::null_mut();
        let mut cur = HEAD;
        while !cur.is_null() {
            let size = (*cur).size;
            if size >= need {
                let next = (*cur).next;
                let (taken, replacement) = if size - need >= 2 * UNIT {
                    let rest = (cur as usize + need) as *mut FreeBlock;
                    (*rest).size = size - need;
                    (*rest).next = next;
                    (need, rest)
                } else {
                    (size, next)
                };
                if prev.is_null() {
                    HEAD = replacement;
                } else {
                    (*prev).next = replacement;
                }
                FREE -= taken;
                let start = cur as usize;
                let data = (start + UNIT + align - 1) & !(align - 1);
                let header = (data - UNIT) as *mut Header;
                (*header).start = start;
                (*header).size = taken;
                return data as *mut u8;
            }
            prev = cur;
            cur = (*cur).next;
        }
    }
    ptr::null_mut()
}

pub unsafe fn dealloc(p: *mut u8) {
    kernel::critical(|| unsafe { dealloc_inner(p) })
}

unsafe fn dealloc_inner(p: *mut u8) {
    unsafe {
        let header = (p as usize - UNIT) as *mut Header;
        let start = (*header).start;
        let size = (*header).size;
        FREE += size;
        let mut prev: *mut FreeBlock = ptr::null_mut();
        let mut cur = HEAD;
        while !cur.is_null() && (cur as usize) < start {
            prev = cur;
            cur = (*cur).next;
        }
        let block = start as *mut FreeBlock;
        (*block).size = size;
        (*block).next = cur;
        if !cur.is_null() && start + size == cur as usize {
            (*block).size += (*cur).size;
            (*block).next = (*cur).next;
        }
        if prev.is_null() {
            HEAD = block;
        } else if prev as usize + (*prev).size == start {
            (*prev).size += (*block).size;
            (*prev).next = (*block).next;
        } else {
            (*prev).next = block;
        }
    }
}

/// Tamaño útil del bloque asignado en `p` (para `realloc` en el sitio).
pub unsafe fn usable_size(p: *mut u8) -> usize {
    unsafe {
        let header = (p as usize - UNIT) as *const Header;
        (*header).start + (*header).size - p as usize
    }
}

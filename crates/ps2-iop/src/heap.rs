//! Asignador global del módulo.
//!
//! Al arrancar se pide al kernel (`sysmem`) un bloque del tamaño indicado en
//! `module!` y dentro se reparte con una lista de bloques libres ordenada por
//! dirección (primer ajuste, fusión de vecinos). Las operaciones van con las
//! interrupciones suspendidas, así que sirve desde cualquier hilo.

use crate::kernel;
use core::alloc::{GlobalAlloc, Layout};
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

pub(crate) fn init(bytes: usize) -> bool {
    let bytes = bytes & !(UNIT - 1);
    let arena = unsafe { kernel::AllocSysMemory(0, bytes as i32, ptr::null_mut()) };
    if arena.is_null() {
        return false;
    }
    let start = (arena as usize + UNIT - 1) & !(UNIT - 1);
    let end = (arena as usize + bytes) & !(UNIT - 1);
    unsafe {
        let block = start as *mut FreeBlock;
        (*block).size = end - start;
        (*block).next = ptr::null_mut();
        HEAD = block;
        TOTAL = end - start;
        FREE = end - start;
    }
    true
}

/// Bytes del heap del módulo.
pub fn total_bytes() -> usize {
    unsafe { TOTAL }
}

/// Bytes libres en el heap del módulo.
pub fn free_bytes() -> usize {
    kernel::critical(|| unsafe { FREE })
}

/// Bytes libres en todo el IOP, según el kernel.
pub fn system_free_bytes() -> usize {
    unsafe { kernel::QueryTotalFreeMemSize() as usize }
}

struct Heap;

#[global_allocator]
static HEAP: Heap = Heap;

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align().max(UNIT);
        let slack = align - UNIT;
        let Some(need) = layout.size().checked_add(UNIT + slack + UNIT - 1).map(|n| n & !(UNIT - 1)) else {
            return ptr::null_mut();
        };
        kernel::critical(|| unsafe {
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
            ptr::null_mut()
        })
    }

    unsafe fn dealloc(&self, p: *mut u8, _layout: Layout) {
        kernel::critical(|| unsafe {
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
        })
    }
}

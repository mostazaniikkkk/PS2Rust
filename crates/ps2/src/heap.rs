//! Asignador global de memoria: lista de bloques libres ordenada por
//! dirección, primer ajuste, con fusión de vecinos al liberar.
//!
//! El heap ocupa desde el final del programa (`_end`) hasta la base de la pila
//! del hilo principal, que el kernel coloca al final de los 32 MiB de RAM.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr;

/// Granularidad y alineación mínima de todos los bloques.
const UNIT: usize = 16;

/// Nodo de la lista de libres; vive dentro del propio bloque libre.
#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

/// Cabecera justo antes de cada bloque entregado.
#[repr(C)]
struct Header {
    /// Inicio real del bloque (puede ser anterior por alineación).
    start: usize,
    /// Tamaño total del bloque, cabecera incluida.
    size: usize,
}

struct State {
    head: *mut FreeBlock,
    total: usize,
    free: usize,
}

pub struct Heap(UnsafeCell<State>);

// El runtime es de un solo hilo y el asignador no se usa desde interrupciones.
unsafe impl Sync for Heap {}

#[global_allocator]
static HEAP: Heap = Heap(UnsafeCell::new(State { head: ptr::null_mut(), total: 0, free: 0 }));

extern "C" {
    static _end: u8;
    static __stack_size: u8;
    static __ps2_heap_end: usize;
}

/// Tope de la RAM de usuario de una PlayStation 2 de 32 MiB.
const RAM_END: usize = 0x0200_0000;

pub(crate) unsafe fn init() {
    let start = (ptr::addr_of!(_end) as usize + UNIT - 1) & !(UNIT - 1);
    let stack_size = ptr::addr_of!(__stack_size) as usize;
    // El kernel nos dijo dónde acaba el heap; si la respuesta no tiene
    // sentido, se calcula a partir de la pila que pedimos.
    let mut end = ptr::read_volatile(ptr::addr_of!(__ps2_heap_end)) & !(UNIT - 1);
    if end <= start || end > RAM_END {
        end = (RAM_END - 0x1000 - stack_size) & !(UNIT - 1);
    }
    let st = &mut *HEAP.0.get();
    if end > start + 2 * UNIT {
        let block = start as *mut FreeBlock;
        (*block).size = end - start;
        (*block).next = ptr::null_mut();
        st.head = block;
        st.total = end - start;
        st.free = end - start;
    }
}

/// Bytes totales gestionados por el heap.
pub fn total_bytes() -> usize {
    unsafe { (*HEAP.0.get()).total }
}

/// Bytes libres en este momento (puede estar fragmentado).
pub fn free_bytes() -> usize {
    unsafe { (*HEAP.0.get()).free }
}

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let st = &mut *self.0.get();
        let align = layout.align().max(UNIT);
        // Cabecera + datos + holgura para alinear, en múltiplos de UNIT.
        let slack = align - UNIT;
        let Some(need) = layout
            .size()
            .checked_add(UNIT + slack + UNIT - 1)
            .map(|n| n & !(UNIT - 1))
        else {
            return ptr::null_mut();
        };

        let mut prev: *mut FreeBlock = ptr::null_mut();
        let mut cur = st.head;
        while !cur.is_null() {
            let size = (*cur).size;
            if size >= need {
                let next = (*cur).next;
                // Parte el bloque si lo que sobra puede alojar otro.
                let (taken, replacement) = if size - need >= 2 * UNIT {
                    let rest = (cur as usize + need) as *mut FreeBlock;
                    (*rest).size = size - need;
                    (*rest).next = next;
                    (need, rest)
                } else {
                    (size, next)
                };
                if prev.is_null() {
                    st.head = replacement;
                } else {
                    (*prev).next = replacement;
                }
                st.free -= taken;

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
    }

    unsafe fn dealloc(&self, ptr_: *mut u8, _layout: Layout) {
        let st = &mut *self.0.get();
        let header = (ptr_ as usize - UNIT) as *mut Header;
        let start = (*header).start;
        let size = (*header).size;
        st.free += size;

        // Busca el hueco: prev < bloque < cur.
        let mut prev: *mut FreeBlock = ptr::null_mut();
        let mut cur = st.head;
        while !cur.is_null() && (cur as usize) < start {
            prev = cur;
            cur = (*cur).next;
        }

        let mut block = start as *mut FreeBlock;
        (*block).size = size;
        (*block).next = cur;
        // Fusiona con el siguiente.
        if !cur.is_null() && start + size == cur as usize {
            (*block).size += (*cur).size;
            (*block).next = (*cur).next;
        }
        // Fusiona con el anterior.
        if prev.is_null() {
            st.head = block;
        } else if prev as usize + (*prev).size == start {
            (*prev).size += (*block).size;
            (*prev).next = (*block).next;
            block = prev;
        } else {
            (*prev).next = block;
        }
        let _ = block;
    }
}

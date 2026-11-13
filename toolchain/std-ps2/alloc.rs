//! Asignador del sistema: el heap de la consola (toda la RAM libre).

use crate::alloc::{GlobalAlloc, Layout, System};
use crate::ptr;
use crate::sys::os::heap;

#[stable(feature = "alloc_system_type", since = "1.28.0")]
unsafe impl GlobalAlloc for System {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { heap::alloc(layout.size(), layout.align()) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { heap::alloc(layout.size(), layout.align()) };
        if !p.is_null() {
            unsafe { ptr::write_bytes(p, 0, layout.size()) };
        }
        p
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe { heap::dealloc(ptr) }
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Si el bloque ya tiene sitio, se queda donde está.
        if new_size <= unsafe { heap::usable_size(ptr) } {
            return ptr;
        }
        unsafe { super::realloc_fallback(self, ptr, layout, new_size) }
    }
}

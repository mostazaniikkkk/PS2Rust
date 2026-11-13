//! Servidor RPC: lo que permite a un programa del EE llamar a este módulo.
//!
//! El EE liga un cliente al identificador del servidor y le envía llamadas
//! (número de función + datos). Aquí cada llamada llega a un manejador de
//! Rust con los datos recibidos en un búfer, donde también deja la respuesta.

use crate::kernel;
use alloc::alloc::{alloc_zeroed, Layout};
use alloc::boxed::Box;

/// Manejador: (número de función, búfer con los datos recibidos y sitio para
/// la respuesta, bytes recibidos).
type Handler = Box<dyn FnMut(u32, &mut [u8], usize)>;

static mut HANDLER: Option<Handler> = None;
static mut BUFFER_SIZE: usize = 0;

extern "C" fn dispatch(fno: i32, buf: *mut u8, size: i32) -> *mut u8 {
    unsafe {
        let capacity = BUFFER_SIZE;
        let data = core::slice::from_raw_parts_mut(buf, capacity);
        if let Some(h) = (*core::ptr::addr_of_mut!(HANDLER)).as_mut() {
            h(fno as u32, data, (size.max(0) as usize).min(capacity));
        }
    }
    buf
}

/// Registra el servidor `sid` y atiende llamadas para siempre en el hilo
/// actual. `buffer_size` es el máximo de bytes por llamada (envío o
/// respuesta). Sólo puede haber un servidor por módulo.
pub fn serve(sid: u32, buffer_size: usize, handler: impl FnMut(u32, &mut [u8], usize) + 'static) -> ! {
    let buffer_size = (buffer_size + 63) & !63;
    unsafe {
        assert!((*core::ptr::addr_of!(HANDLER)).is_none(), "ya hay un servidor RPC en este módulo");
        HANDLER = Some(Box::new(handler));
        BUFFER_SIZE = buffer_size;
        // Estructuras del kernel (SifRpcServerData_t y SifRpcDataQueue_t) y
        // el búfer de recepción; viven mientras viva el módulo.
        let server = alloc_zeroed(Layout::from_size_align(128, 16).unwrap());
        let queue = alloc_zeroed(Layout::from_size_align(64, 16).unwrap());
        let buffer = alloc_zeroed(Layout::from_size_align(buffer_size, 64).unwrap());
        assert!(!server.is_null() && !queue.is_null() && !buffer.is_null(), "sin memoria para el servidor RPC");
        kernel::sceSifInitRpc(0);
        kernel::sceSifSetRpcQueue(queue, kernel::GetThreadId());
        kernel::sceSifRegisterRpc(server, sid, dispatch, buffer, 0, 0, queue);
        kernel::sceSifRpcLoop(queue);
    }
    loop {
        unsafe { kernel::SleepThread() };
    }
}

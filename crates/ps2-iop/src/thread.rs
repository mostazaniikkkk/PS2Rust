//! Hilos del kernel del IOP (`thbase`). A diferencia del EE, aquí el
//! planificador es expulsivo por prioridades: un hilo de mayor prioridad
//! (número menor) interrumpe a los demás en cuanto puede correr.

use crate::kernel;
use alloc::boxed::Box;

/// Identificador de un hilo del kernel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ThreadId(pub i32);

/// Configuración de un hilo nuevo.
pub struct Builder {
    stack_size: usize,
    priority: u32,
}

impl Builder {
    pub fn new() -> Builder {
        Builder { stack_size: 8 * 1024, priority: 40 }
    }

    /// Tamaño de la pila en bytes (por defecto 8 KiB).
    pub fn stack_size(mut self, bytes: usize) -> Builder {
        self.stack_size = bytes;
        self
    }

    /// Prioridad: de 1 (máxima) a 126; por defecto 40.
    pub fn priority(mut self, priority: u32) -> Builder {
        self.priority = priority;
        self
    }

    /// Crea y arranca el hilo. `None` si el kernel no pudo crearlo.
    pub fn spawn<F: FnOnce() + Send + 'static>(self, f: F) -> Option<ThreadId> {
        // Doble caja: `StartThread` sólo pasa un puntero fino.
        let closure: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
        let arg = Box::into_raw(closure) as *mut u8;
        let param = kernel::ThreadParam {
            attr: kernel::TH_C,
            option: 0,
            entry: trampoline,
            stack_size: self.stack_size as u32,
            priority: self.priority,
        };
        let id = unsafe { kernel::CreateThread(&param) };
        if id <= 0 {
            drop(unsafe { Box::from_raw(arg as *mut Box<dyn FnOnce() + Send>) });
            return None;
        }
        if unsafe { kernel::StartThread(id, arg) } < 0 {
            unsafe {
                kernel::DeleteThread(id);
                drop(Box::from_raw(arg as *mut Box<dyn FnOnce() + Send>));
            }
            return None;
        }
        Some(ThreadId(id))
    }
}

extern "C" fn trampoline(arg: *mut u8) {
    let closure = unsafe { Box::from_raw(arg as *mut Box<dyn FnOnce() + Send>) };
    closure();
    // Termina y libera el hilo.
    unsafe { kernel::ExitDeleteThread() };
}

/// Crea un hilo con la configuración por defecto.
pub fn spawn<F: FnOnce() + Send + 'static>(f: F) -> Option<ThreadId> {
    Builder::new().spawn(f)
}

/// Identificador del hilo actual.
pub fn current() -> ThreadId {
    ThreadId(unsafe { kernel::GetThreadId() })
}

/// Duerme el hilo actual `usec` microsegundos.
pub fn sleep_usec(usec: u32) {
    unsafe { kernel::DelayThread(usec as i32) };
}

/// Duerme el hilo actual `ms` milisegundos.
pub fn sleep_ms(ms: u32) {
    sleep_usec(ms.saturating_mul(1000));
}

/// Cede la CPU a otros hilos de la misma prioridad.
pub fn yield_now() {
    unsafe { kernel::RotateThreadReadyQueue(0) };
}

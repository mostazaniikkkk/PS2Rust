//! Hilos de la std sobre los hilos del kernel del EE.
//!
//! Todos los hilos de Rust tienen la misma prioridad, así que el planificador
//! es cooperativo: un hilo corre hasta que se bloquea (E/S, `Mutex`, canal,
//! `join`), duerme o llama a `yield_now`.

use crate::ffi::CStr;
use crate::io;
use crate::num::NonZero;
use crate::ptr;
use crate::sys::os::{kernel, sched};
use crate::thread::ThreadInit;
use crate::time::Duration;

pub const DEFAULT_MIN_STACK_SIZE: usize = 256 * 1024;
const MIN_STACK_SIZE: usize = 16 * 1024;

const RUNNING: u32 = 0;
const FINISHED: u32 = 1;
const DETACHED: u32 = 2;

/// Estado compartido entre el hilo y su `JoinHandle`.
struct Shared {
    init: Option<Box<ThreadInit>>,
    id: i32,
    done_sema: i32,
    stack: *mut u8,
    stack_size: usize,
    state: u32,
    /// Lista de hilos sueltos ya terminados, pendientes de liberar.
    next_zombie: *mut Shared,
}

static mut ZOMBIES: *mut Shared = ptr::null_mut();

pub struct Thread {
    shared: *mut Shared,
}

unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

fn stack_layout(size: usize) -> crate::alloc::Layout {
    crate::alloc::Layout::from_size_align(size, 16).unwrap()
}

/// Libera un hilo terminado (debe estar ya inactivo en el kernel).
unsafe fn release(shared: *mut Shared) {
    unsafe {
        while kernel::thread_status((*shared).id) >= 0 && kernel::thread_status((*shared).id) & kernel::THS_DORMANT == 0 {
            kernel::rotate_ready_queue(kernel::THREAD_PRIORITY);
        }
        kernel::delete_thread((*shared).id);
        kernel::delete_sema((*shared).done_sema);
        crate::alloc::dealloc((*shared).stack, stack_layout((*shared).stack_size));
        drop(Box::from_raw(shared));
    }
}

/// Libera los hilos sueltos que ya terminaron.
fn reap_zombies() {
    let list = kernel::critical(|| unsafe { crate::mem::replace(&mut ZOMBIES, ptr::null_mut()) });
    let mut cur = list;
    while !cur.is_null() {
        let next = unsafe { (*cur).next_zombie };
        unsafe { release(cur) };
        cur = next;
    }
}

impl Thread {
    // unsafe: ver thread::Builder::spawn_unchecked.
    pub unsafe fn new(stack: usize, init: Box<ThreadInit>) -> io::Result<Thread> {
        reap_zombies();
        let stack_size = (stack.max(MIN_STACK_SIZE) + 15) & !15;
        let stack_ptr = unsafe { crate::alloc::alloc(stack_layout(stack_size)) };
        if stack_ptr.is_null() {
            return Err(io::const_error!(io::ErrorKind::OutOfMemory, "no memory for the thread stack"));
        }
        let done_sema = kernel::create_sema(0, 1);
        let shared = Box::into_raw(Box::new(Shared {
            init: Some(init),
            id: -1,
            done_sema,
            stack: stack_ptr,
            stack_size,
            state: RUNNING,
            next_zombie: ptr::null_mut(),
        }));
        let param = kernel::ThreadParam {
            status: 0,
            func: thread_start as *const () as usize,
            stack: stack_ptr as usize,
            stack_size: stack_size as i32,
            gp: 0,
            initial_priority: kernel::THREAD_PRIORITY as i32,
            current_priority: 0,
            attr: 0,
            option: 0,
        };
        let id = kernel::create_thread(&param);
        if id < 0 {
            unsafe {
                kernel::delete_sema(done_sema);
                crate::alloc::dealloc(stack_ptr, stack_layout(stack_size));
                drop(Box::from_raw(shared));
            }
            return Err(io::const_error!(io::ErrorKind::Other, "the kernel could not create the thread"));
        }
        unsafe { (*shared).id = id };
        kernel::start_thread(id, shared as usize);
        Ok(Thread { shared })
    }

    pub fn join(self) {
        let shared = self.shared;
        crate::mem::forget(self);
        unsafe {
            kernel::wait_sema((*shared).done_sema);
            release(shared);
        }
    }
}

impl Drop for Thread {
    // El `JoinHandle` se soltó sin `join`: el hilo queda suelto.
    fn drop(&mut self) {
        let shared = self.shared;
        let finished = kernel::critical(|| unsafe {
            if (*shared).state == FINISHED {
                true
            } else {
                (*shared).state = DETACHED;
                false
            }
        });
        if finished {
            unsafe { release(shared) };
        }
    }
}

#[inline(never)]
fn rust_main_thread_not_inlined(init: Box<ThreadInit>) {
    let rust_start = init.init();
    rust_start();
}

extern "C" fn thread_start(arg: usize) {
    let shared = arg as *mut Shared;
    let init = unsafe { (*shared).init.take().unwrap() };
    rust_main_thread_not_inlined(init);
    crate::rt::thread_cleanup();
    unsafe { crate::sys::thread_local::key::destroy_tls() };

    kernel::critical(|| unsafe {
        if (*shared).state == DETACHED {
            // Nadie hará `join`: el próximo `spawn` lo liberará.
            (*shared).next_zombie = ZOMBIES;
            ZOMBIES = shared;
        } else {
            (*shared).state = FINISHED;
            kernel::signal_sema((*shared).done_sema);
        }
    });
    kernel::exit_thread();
}

pub fn available_parallelism() -> io::Result<NonZero<usize>> {
    Ok(NonZero::new(1).unwrap())
}

pub fn current_os_id() -> Option<u64> {
    Some(kernel::thread_id() as u64)
}

pub fn set_name(_name: &CStr) {}

pub fn yield_now() {
    kernel::rotate_ready_queue(kernel::THREAD_PRIORITY);
}

pub fn sleep(dur: Duration) {
    let deadline = sched::now().saturating_add(sched::nanos_to_cycles(dur.as_nanos()));
    sched::sleep_until(deadline);
}


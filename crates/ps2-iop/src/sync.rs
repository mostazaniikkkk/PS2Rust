//! Sincronización entre hilos del IOP: semáforos del kernel y un `Mutex`
//! construido sobre ellos.
//!
//! El R3000A no tiene instrucciones atómicas, así que el target no ofrece
//! `core::sync::atomic` ni `Arc`. Para datos compartidos: [`Mutex`], o
//! [`crate::kernel::critical`] para secciones muy cortas.

use crate::kernel;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

/// Semáforo del kernel.
pub struct Semaphore {
    id: i32,
}

impl Semaphore {
    /// Crea un semáforo con `initial` permisos y un máximo de `max`.
    pub fn new(initial: i32, max: i32) -> Semaphore {
        let p = kernel::SemaParam { attr: 0, option: 0, initial, max };
        Semaphore { id: unsafe { kernel::CreateSema(&p) } }
    }

    /// Espera un permiso.
    pub fn wait(&self) {
        unsafe { kernel::WaitSema(self.id) };
    }

    /// Toma un permiso si lo hay, sin esperar.
    pub fn try_wait(&self) -> bool {
        unsafe { kernel::PollSema(self.id) >= 0 }
    }

    /// Devuelve un permiso.
    pub fn signal(&self) {
        unsafe { kernel::SignalSema(self.id) };
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        unsafe { kernel::DeleteSema(self.id) };
    }
}

/// Exclusión mutua con un semáforo del kernel. El semáforo se crea al primer
/// uso, así que `Mutex::new` sirve en un `static`.
pub struct Mutex<T> {
    sema: UnsafeCell<i32>,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for Mutex<T> {}
unsafe impl<T: Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Mutex<T> {
        Mutex { sema: UnsafeCell::new(-1), value: UnsafeCell::new(value) }
    }

    fn sema(&self) -> i32 {
        kernel::critical(|| unsafe {
            let id = &mut *self.sema.get();
            if *id < 0 {
                let p = kernel::SemaParam { attr: 0, option: 0, initial: 1, max: 1 };
                *id = kernel::CreateSema(&p);
            }
            *id
        })
    }

    /// Toma el candado; se libera al soltar la guarda.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        let sema = self.sema();
        unsafe { kernel::WaitSema(sema) };
        MutexGuard { mutex: self, sema }
    }
}

pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
    sema: i32,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        unsafe { kernel::SignalSema(self.sema) };
    }
}

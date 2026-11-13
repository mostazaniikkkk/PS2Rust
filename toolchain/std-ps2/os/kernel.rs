//! Llamadas al kernel del Emotion Engine, secciones críticas y caché.
//!
//! Sólo depende de `core`: se compila tanto dentro de la std para PS2 como en
//! programas `no_std`.

use core::arch::global_asm;

global_asm!(
    r#"
    .set push
    .set noreorder
    .set noat

    .section .text.__ps2os_syscall,"ax",@progbits
    # u32 __ps2os_syscall(a0, a1, a2, a3, numero): el número va en la pila (o32).
    .globl __ps2os_syscall
    .type __ps2os_syscall,@function
__ps2os_syscall:
    lw    $3, 16($sp)
    syscall
    jr    $ra
    nop
    .size __ps2os_syscall, . - __ps2os_syscall

    .section .text.__ps2os_di,"ax",@progbits
    # u32 __ps2os_di(): desactiva interrupciones; devuelve 1 si estaban activas.
    # El `di` del R5900 puede no surtir efecto si llega una interrupción a la
    # vez: se repite hasta que Status.EIE quede a cero.
    .globl __ps2os_di
    .type __ps2os_di,@function
    .p2align 3
__ps2os_di:
    .word 0x40026000                  # mfc0  $2, $12 (Status)
    lui   $3, 1                       # EIE = 0x10000
    and   $2, $2, $3
    beqz  $2, 2f
    nop
    .p2align 3
1:  .word 0x42000039                  # di
    .word 0x0000040F                  # sync.p
    .word 0x40046000                  # mfc0  $4, $12
    and   $4, $4, $3
    bnez  $4, 1b
    nop
    addiu $2, $zero, 1
2:  jr    $ra
    nop
    .size __ps2os_di, . - __ps2os_di

    .section .text.__ps2os_ei,"ax",@progbits
    .globl __ps2os_ei
    .type __ps2os_ei,@function
__ps2os_ei:
    .word 0x42000038                  # ei
    jr    $ra
    nop
    .size __ps2os_ei, . - __ps2os_ei

    .section .text.__ps2os_exit_handler,"ax",@progbits
    # Antes de volver de un manejador de interrupción (errata del kernel).
    .globl __ps2os_exit_handler
    .type __ps2os_exit_handler,@function
__ps2os_exit_handler:
    .word 0x0000000F                  # sync.l
    .word 0x42000038                  # ei
    jr    $ra
    nop
    .size __ps2os_exit_handler, . - __ps2os_exit_handler

    .section .text.__ps2os_count,"ax",@progbits
    # Registro Count del COP0: un tick por ciclo de CPU (294,912 MHz).
    .globl __ps2os_count
    .type __ps2os_count,@function
__ps2os_count:
    .word 0x40024800                  # mfc0  $2, $9
    jr    $ra
    nop
    .size __ps2os_count, . - __ps2os_count

    .section .text.__ps2os_status,"ax",@progbits
    .globl __ps2os_status
    .type __ps2os_status,@function
__ps2os_status:
    .word 0x40026000                  # mfc0  $2, $12
    jr    $ra
    nop
    .size __ps2os_status, . - __ps2os_status

    .section .text.__ps2os_sync,"ax",@progbits
    .globl __ps2os_sync
    .type __ps2os_sync,@function
__ps2os_sync:
    .word 0x0000000F                  # sync.l
    jr    $ra
    nop
    .size __ps2os_sync, . - __ps2os_sync

    .set pop
    "#
);

unsafe extern "C" {
    fn __ps2os_syscall(a0: u32, a1: u32, a2: u32, a3: u32, number: i32) -> u32;
    fn __ps2os_di() -> u32;
    fn __ps2os_ei();
    fn __ps2os_exit_handler();
    fn __ps2os_count() -> u32;
    fn __ps2os_status() -> u32;
    fn __ps2os_sync();
}

/// Ejecuta una syscall del kernel. Los números negativos son las variantes
/// para usar desde manejadores de interrupción (`iSignalSema`, …).
#[inline]
pub unsafe fn syscall(number: i32, a0: u32, a1: u32, a2: u32, a3: u32) -> u32 {
    unsafe { __ps2os_syscall(a0, a1, a2, a3, number) }
}

// Números de syscall del kernel del EE.
pub const SYS_EXIT: i32 = 0x04;
pub const SYS_ADD_INTC_HANDLER: i32 = 0x10;
pub const SYS_ADD_DMAC_HANDLER: i32 = 0x12;
pub const SYS_ENABLE_INTC: i32 = 0x14;
pub const SYS_ENABLE_DMAC: i32 = 0x16;
pub const SYS_CREATE_THREAD: i32 = 0x20;
pub const SYS_DELETE_THREAD: i32 = 0x21;
pub const SYS_START_THREAD: i32 = 0x22;
pub const SYS_EXIT_THREAD: i32 = 0x23;
pub const SYS_CHANGE_THREAD_PRIORITY: i32 = 0x29;
pub const SYS_ROTATE_THREAD_READY_QUEUE: i32 = 0x2b;
pub const SYS_GET_THREAD_ID: i32 = 0x2f;
pub const SYS_REFER_THREAD_STATUS: i32 = 0x30;
pub const SYS_SLEEP_THREAD: i32 = 0x32;
pub const SYS_WAKEUP_THREAD: i32 = 0x33;
pub const SYS_I_WAKEUP_THREAD: i32 = -0x34;
pub const SYS_CREATE_SEMA: i32 = 0x40;
pub const SYS_DELETE_SEMA: i32 = 0x41;
pub const SYS_SIGNAL_SEMA: i32 = 0x42;
pub const SYS_I_SIGNAL_SEMA: i32 = -0x43;
pub const SYS_WAIT_SEMA: i32 = 0x44;
pub const SYS_FLUSH_CACHE: i32 = 0x64;
pub const SYS_SIF_STOP_DMA: i32 = 0x6b;
pub const SYS_SIF_SET_DMA: i32 = 0x77;
pub const SYS_I_SIF_SET_DMA: i32 = -0x77;
pub const SYS_SIF_SET_DCHAIN: i32 = 0x78;
pub const SYS_I_SIF_SET_DCHAIN: i32 = -0x78;
pub const SYS_SIF_SET_REG: i32 = 0x79;
pub const SYS_SIF_GET_REG: i32 = 0x7a;

/// Prioridad de todos los hilos de Rust. Con una sola prioridad el
/// planificador del kernel es cooperativo: un hilo sólo cede la CPU al
/// bloquearse, dormir o llamar a `yield_now`.
pub const THREAD_PRIORITY: u32 = 64;

/// Desactiva las interrupciones; devuelve si estaban activas.
#[inline]
pub fn disable_interrupts() -> bool {
    unsafe { __ps2os_di() != 0 }
}

#[inline]
pub fn enable_interrupts() {
    unsafe { __ps2os_ei() }
}

/// Ejecuta `f` con las interrupciones desactivadas.
#[inline]
pub fn critical<R>(f: impl FnOnce() -> R) -> R {
    let was_enabled = disable_interrupts();
    let r = f();
    if was_enabled {
        enable_interrupts();
    }
    r
}

/// Llamar justo antes de volver de un manejador de interrupción.
#[inline]
pub fn exit_handler() {
    unsafe { __ps2os_exit_handler() }
}

/// Registro Count del COP0 (32 bits, da la vuelta cada ~14,5 s).
#[inline]
pub fn cpu_count() -> u32 {
    unsafe { __ps2os_count() }
}

#[inline]
pub fn cop0_status() -> u32 {
    unsafe { __ps2os_status() }
}

/// `sync.l`: vacía el búfer de escritura antes de un DMA.
#[inline]
pub fn sync() {
    unsafe { __ps2os_sync() }
}

/// Dirección sin caché equivalente a `p` (espejo 0x2000_0000 de la RAM).
#[inline]
pub fn uncached<T>(p: *const T) -> *mut T {
    (p as usize | 0x2000_0000) as *mut T
}

/// `FlushCache(0)`: escribe a RAM e invalida toda la caché de datos.
pub fn flush_dcache() {
    unsafe { syscall(SYS_FLUSH_CACHE, 0, 0, 0, 0) };
}

/// Termina el programa y vuelve al navegador de la consola.
pub fn exit(code: i32) -> ! {
    unsafe { syscall(SYS_EXIT, code as u32, 0, 0, 0) };
    loop {}
}

// ------------------------------------------------------------------ hilos

/// Parámetros de `CreateThread`.
#[repr(C)]
pub struct ThreadParam {
    pub status: i32,
    pub func: usize,
    pub stack: usize,
    pub stack_size: i32,
    pub gp: usize,
    pub initial_priority: i32,
    pub current_priority: i32,
    pub attr: u32,
    pub option: u32,
}

/// Estado de un hilo según `ReferThreadStatus`.
pub const THS_DORMANT: i32 = 0x10;

pub fn create_thread(param: &ThreadParam) -> i32 {
    unsafe { syscall(SYS_CREATE_THREAD, param as *const _ as u32, 0, 0, 0) as i32 }
}

pub fn start_thread(id: i32, arg: usize) -> i32 {
    unsafe { syscall(SYS_START_THREAD, id as u32, arg as u32, 0, 0) as i32 }
}

pub fn exit_thread() -> ! {
    unsafe { syscall(SYS_EXIT_THREAD, 0, 0, 0, 0) };
    loop {}
}

pub fn delete_thread(id: i32) -> i32 {
    unsafe { syscall(SYS_DELETE_THREAD, id as u32, 0, 0, 0) as i32 }
}

pub fn thread_id() -> i32 {
    unsafe { syscall(SYS_GET_THREAD_ID, 0, 0, 0, 0) as i32 }
}

pub fn change_thread_priority(id: i32, priority: u32) -> i32 {
    unsafe { syscall(SYS_CHANGE_THREAD_PRIORITY, id as u32, priority, 0, 0) as i32 }
}

pub fn rotate_ready_queue(priority: u32) {
    unsafe { syscall(SYS_ROTATE_THREAD_READY_QUEUE, priority, 0, 0, 0) };
}

/// Devuelve el campo `status` del hilo, o un valor negativo si no existe.
pub fn thread_status(id: i32) -> i32 {
    // ReferThreadStatus escribe una estructura de 48 bytes.
    let mut buf = [0u32; 16];
    let r = unsafe { syscall(SYS_REFER_THREAD_STATUS, id as u32, buf.as_mut_ptr() as u32, 0, 0) as i32 };
    if r < 0 { r } else { buf[0] as i32 }
}

pub fn sleep_thread() {
    unsafe { syscall(SYS_SLEEP_THREAD, 0, 0, 0, 0) };
}

pub fn wakeup_thread(id: i32) {
    unsafe { syscall(SYS_WAKEUP_THREAD, id as u32, 0, 0, 0) };
}

/// Variante para manejadores de interrupción.
pub fn i_wakeup_thread(id: i32) {
    unsafe { syscall(SYS_I_WAKEUP_THREAD, id as u32, 0, 0, 0) };
}

// ------------------------------------------------------------ semáforos

#[repr(C)]
struct SemaParam {
    count: i32,
    max_count: i32,
    init_count: i32,
    wait_threads: i32,
    attr: u32,
    option: u32,
}

pub fn create_sema(initial: i32, max: i32) -> i32 {
    let p = SemaParam { count: 0, max_count: max, init_count: initial, wait_threads: 0, attr: 0, option: 0 };
    unsafe { syscall(SYS_CREATE_SEMA, &p as *const _ as u32, 0, 0, 0) as i32 }
}

pub fn delete_sema(id: i32) {
    unsafe { syscall(SYS_DELETE_SEMA, id as u32, 0, 0, 0) };
}

pub fn signal_sema(id: i32) {
    unsafe { syscall(SYS_SIGNAL_SEMA, id as u32, 0, 0, 0) };
}

pub fn i_signal_sema(id: i32) {
    unsafe { syscall(SYS_I_SIGNAL_SEMA, id as u32, 0, 0, 0) };
}

pub fn wait_sema(id: i32) {
    unsafe { syscall(SYS_WAIT_SEMA, id as u32, 0, 0, 0) };
}

// --------------------------------------------------------- interrupciones

pub const INTC_TIM1: u32 = 10;
pub const DMAC_SIF0: u32 = 5;

pub type IntcHandler = extern "C" fn(cause: i32, arg: usize, addr: usize) -> i32;
pub type DmacHandler = extern "C" fn(channel: i32, arg: usize, addr: usize) -> i32;

pub fn add_intc_handler(cause: u32, handler: IntcHandler) -> i32 {
    unsafe { syscall(SYS_ADD_INTC_HANDLER, cause, handler as usize as u32, 0, 0) as i32 }
}

pub fn enable_intc(cause: u32) {
    unsafe { syscall(SYS_ENABLE_INTC, cause, 0, 0, 0) };
}

pub fn add_dmac_handler(channel: u32, handler: DmacHandler) -> i32 {
    unsafe { syscall(SYS_ADD_DMAC_HANDLER, channel, handler as usize as u32, 0, 0) as i32 }
}

pub fn enable_dmac(channel: u32) {
    unsafe { syscall(SYS_ENABLE_DMAC, channel, 0, 0, 0) };
}

// --------------------------------------------------------------------- SIF

/// Descriptor de una transferencia SIF (EE -> IOP).
#[repr(C)]
pub struct SifDma {
    pub src: u32,
    pub dest: u32,
    pub size: u32,
    pub attr: u32,
}

pub fn sif_set_dma(list: &[SifDma], from_interrupt: bool) -> u32 {
    let n = if from_interrupt { SYS_I_SIF_SET_DMA } else { SYS_SIF_SET_DMA };
    unsafe { syscall(n, list.as_ptr() as u32, list.len() as u32, 0, 0) }
}

/// `SifStopDma`: detiene las transferencias SIF en curso.
pub fn sif_stop_dma() {
    unsafe { syscall(SYS_SIF_STOP_DMA, 0, 0, 0, 0) };
}

pub fn sif_set_dchain(from_interrupt: bool) {
    let n = if from_interrupt { SYS_I_SIF_SET_DCHAIN } else { SYS_SIF_SET_DCHAIN };
    unsafe { syscall(n, 0, 0, 0, 0) };
}

pub fn sif_get_reg(reg: u32) -> u32 {
    unsafe { syscall(SYS_SIF_GET_REG, reg, 0, 0, 0) }
}

pub fn sif_set_reg(reg: u32, value: u32) {
    unsafe { syscall(SYS_SIF_SET_REG, reg, value, 0, 0) };
}

// -------------------------------------------------------- hardware directo

#[inline]
pub unsafe fn read32(addr: u32) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline]
pub unsafe fn write32(addr: u32, value: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

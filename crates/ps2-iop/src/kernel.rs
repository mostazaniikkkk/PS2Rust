//! Funciones del kernel del IOP.
//!
//! En el IOP no hay syscalls: cada servicio del kernel es un módulo (`thbase`,
//! `sysmem`, `sifcmd`…) que exporta funciones numeradas. Un módulo declara lo
//! que usa con **tablas de importación** dentro de su código: una cabecera con
//! el nombre de la biblioteca y, por cada función, un hueco de dos
//! instrucciones (`jr $ra` + `li $zero, número`) que el cargador de la BIOS
//! sustituye por un salto a la función real.

/// Declara una tabla de importación y los símbolos de sus funciones:
/// biblioteca, versión y pares `número => símbolo`. Las funciones se declaran
/// después en un bloque `extern "C"`.
///
/// ```ignore
/// ps2_iop::import_table!("sysclib", 1 . 1, { 27 => bios_strlen });
/// extern "C" {
///     fn bios_strlen(s: *const u8) -> usize;
/// }
/// ```
#[macro_export]
#[allow_internal_unstable(asm_experimental_arch)]
macro_rules! import_table {
    ($lib:literal, $major:literal . $minor:literal, { $($ord:literal => $name:ident),* $(,)? }) => {
        ::core::arch::global_asm!(
            concat!("    .section .text.iop.import.", $lib, ",\"ax\",@progbits"),
            "    .set push",
            "    .set noreorder",
            "    .p2align 2",
            "    .word 0x41e00000, 0",
            concat!("    .word (", $major, " << 8) | ", $minor),
            concat!("9:  .ascii \"", $lib, "\""),
            "    .space 8 - (. - 9b)",
            $(
                concat!("    .globl ", stringify!($name)),
                concat!("    .type ", stringify!($name), ",@function"),
                concat!(stringify!($name), ":"),
                "    .word 0x03e00008",
                concat!("    .word 0x24000000 | ", $ord),
            )*
            "    .word 0, 0",
            "    .set pop",
        );
    };
}

// `puts` hace falta porque LLVM convierte un `printf` de texto fijo
// terminado en salto de línea en una llamada a `puts`.
import_table!("stdio", 1 . 2, { 4 => printf, 7 => puts });

import_table!("sysmem", 1 . 1, {
    4 => AllocSysMemory,
    5 => FreeSysMemory,
    6 => QueryMemSize,
    7 => QueryMaxFreeMemSize,
    8 => QueryTotalFreeMemSize,
});

import_table!("thbase", 1 . 1, {
    4 => CreateThread,
    5 => DeleteThread,
    6 => StartThread,
    8 => ExitThread,
    9 => ExitDeleteThread,
    14 => ChangeThreadPriority,
    16 => RotateThreadReadyQueue,
    20 => GetThreadId,
    24 => SleepThread,
    25 => WakeupThread,
    33 => DelayThread,
    34 => GetSystemTime,
    40 => SysClock2USec,
});

import_table!("thsemap", 1 . 2, {
    4 => CreateSema,
    5 => DeleteSema,
    6 => SignalSema,
    7 => iSignalSema,
    8 => WaitSema,
    9 => PollSema,
});

import_table!("intrman", 1 . 2, {
    17 => CpuSuspendIntr,
    18 => CpuResumeIntr,
});

import_table!("sifcmd", 1 . 1, {
    14 => sceSifInitRpc,
    17 => sceSifRegisterRpc,
    19 => sceSifSetRpcQueue,
    22 => sceSifRpcLoop,
});

/// Parámetros de `CreateThread`.
#[repr(C)]
pub struct ThreadParam {
    pub attr: u32,
    pub option: u32,
    pub entry: extern "C" fn(*mut u8),
    pub stack_size: u32,
    pub priority: u32,
}

/// Atributo de los hilos escritos en un lenguaje de alto nivel.
pub const TH_C: u32 = 0x0200_0000;

/// Parámetros de `CreateSema`.
#[repr(C)]
pub struct SemaParam {
    pub attr: u32,
    pub option: u32,
    pub initial: i32,
    pub max: i32,
}

/// Reloj del sistema del IOP (64 bits en dos mitades).
#[repr(C)]
pub struct SysClock {
    pub lo: u32,
    pub hi: u32,
}

#[allow(non_snake_case)]
extern "C" {
    pub fn printf(format: *const u8, ...) -> i32;
    pub fn puts(s: *const u8) -> i32;

    pub fn AllocSysMemory(mode: i32, size: i32, ptr: *mut u8) -> *mut u8;
    pub fn FreeSysMemory(ptr: *mut u8) -> i32;
    pub fn QueryMemSize() -> u32;
    pub fn QueryMaxFreeMemSize() -> u32;
    pub fn QueryTotalFreeMemSize() -> u32;

    pub fn CreateThread(param: *const ThreadParam) -> i32;
    pub fn DeleteThread(id: i32) -> i32;
    pub fn StartThread(id: i32, arg: *mut u8) -> i32;
    pub fn ExitThread() -> i32;
    pub fn ExitDeleteThread() -> i32;
    pub fn ChangeThreadPriority(id: i32, priority: i32) -> i32;
    pub fn RotateThreadReadyQueue(priority: i32) -> i32;
    pub fn GetThreadId() -> i32;
    pub fn SleepThread() -> i32;
    pub fn WakeupThread(id: i32) -> i32;
    pub fn DelayThread(usec: i32) -> i32;
    pub fn GetSystemTime(clock: *mut SysClock) -> i32;
    pub fn SysClock2USec(clock: *const SysClock, sec: *mut u32, usec: *mut u32);

    pub fn CreateSema(param: *const SemaParam) -> i32;
    pub fn DeleteSema(id: i32) -> i32;
    pub fn SignalSema(id: i32) -> i32;
    pub fn iSignalSema(id: i32) -> i32;
    pub fn WaitSema(id: i32) -> i32;
    pub fn PollSema(id: i32) -> i32;

    pub fn CpuSuspendIntr(state: *mut i32) -> i32;
    pub fn CpuResumeIntr(state: i32) -> i32;

    pub fn sceSifInitRpc(mode: i32);
    pub fn sceSifRegisterRpc(
        server: *mut u8,
        sid: u32,
        func: extern "C" fn(i32, *mut u8, i32) -> *mut u8,
        buf: *mut u8,
        cancel_func: usize,
        cancel_buf: usize,
        queue: *mut u8,
    );
    pub fn sceSifSetRpcQueue(queue: *mut u8, thread_id: i32);
    pub fn sceSifRpcLoop(queue: *mut u8);
}

/// Ejecuta `f` con las interrupciones (y por tanto el cambio de hilo)
/// suspendidas. Debe ser breve.
#[inline]
pub fn critical<R>(f: impl FnOnce() -> R) -> R {
    let mut state = 0;
    unsafe { CpuSuspendIntr(&mut state) };
    let r = f();
    unsafe { CpuResumeIntr(state) };
    r
}

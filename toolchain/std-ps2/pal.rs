//! Plataforma PlayStation 2 (Emotion Engine) para la biblioteca estándar.
//!
//! Este archivo se instala como `library/std/src/sys/pal/ps2/mod.rs`. La capa
//! de sistema operativo (kernel, SIF, IOP, reloj) está en `os/` y sólo
//! depende de `core`.

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(missing_docs, nonstandard_style, static_mut_refs, dead_code)]

pub mod futex;
pub mod os;

use crate::arch::global_asm;
use crate::io;
use crate::ptr::{addr_of, read_volatile};
use os::{heap, kernel, sio};

/// Pila del hilo principal: 1 MiB.
const MAIN_STACK_SIZE: u32 = 0x10_0000;

global_asm!(
    r#"
    .section .text.start,"ax",@progbits
    .set push
    .set noreorder
    .set noat

    .globl _start
    .type _start,@function
_start:
    # .bss a cero.
    lui   $2, %hi(__bss_start)
    addiu $2, $2, %lo(__bss_start)
    lui   $3, %hi(__bss_end)
    addiu $3, $3, %lo(__bss_end)
1:  beq   $2, $3, 2f
    nop
    sw    $zero, 0($2)
    addiu $2, $2, 4
    b     1b
    nop
2:
    # SetupThread(gp, pila, tamaño, argumentos, raíz).
    move  $4, $zero
    addiu $5, $zero, -1
    lui   $6, 0x10                    # 1 MiB
    lui   $7, %hi(__ps2std_args)
    addiu $7, $7, %lo(__ps2std_args)
    lui   $8, %hi(__ps2std_root)
    addiu $8, $8, %lo(__ps2std_root)
    addiu $3, $zero, 60
    syscall
    move  $sp, $2

    # SetupHeap(_end, -1): devuelve el final del heap.
    lui   $4, %hi(_end)
    addiu $4, $4, %lo(_end)
    addiu $5, $zero, -1
    addiu $3, $zero, 61
    syscall
    lui   $8, %hi(__ps2std_heap_end)
    sw    $2, %lo(__ps2std_heap_end)($8)

    # FlushCache(0).
    move  $4, $zero
    addiu $3, $zero, 100
    syscall

    .word 0x42000038                  # ei
    addiu $sp, $sp, -32
    jal   __ps2std_start
    nop
3:  b     3b
    nop
    .size _start, . - _start

    .globl __ps2std_root
    .type __ps2std_root,@function
__ps2std_root:
    addiu $3, $zero, 35               # ExitThread
    syscall
4:  b     4b
    nop
    .size __ps2std_root, . - __ps2std_root
    .set pop

    .section .bss.__ps2std_args,"aw",@nobits
    .p2align 4
    .globl __ps2std_args
__ps2std_args:
    .space 512
    .globl __ps2std_heap_end
__ps2std_heap_end:
    .space 16
    "#
);

unsafe extern "C" {
    static _end: u8;
    static __ps2std_args: [u32; 128];
    static __ps2std_heap_end: usize;
    fn main(argc: isize, argv: *const *const u8) -> i32;
}

/// Tope de la RAM de usuario (32 MiB).
const RAM_END: usize = 0x0200_0000;

/// Primer código Rust tras el arranque: heap, prioridad del hilo y `main`.
#[unsafe(no_mangle)]
extern "C" fn __ps2std_start() -> ! {
    unsafe {
        let start = addr_of!(_end) as usize;
        let mut end = read_volatile(addr_of!(__ps2std_heap_end));
        if end <= start || end > RAM_END {
            end = RAM_END - 0x1000 - MAIN_STACK_SIZE as usize;
        }
        heap::init(start, end);
    }
    kernel::change_thread_priority(kernel::thread_id(), kernel::THREAD_PRIORITY);
    let args = addr_of!(__ps2std_args) as *const u32;
    let argc = unsafe { read_volatile(args) } as isize;
    let argv = unsafe { args.add(1) } as *const *const u8;
    let code = unsafe { main(argc.clamp(0, 16), argv) };
    exit_process(code)
}

// SAFETY: se llama una vez, durante la inicialización del runtime.
pub unsafe fn init(argc: isize, argv: *const *const u8, _sigpipe: u8) {
    unsafe { crate::sys::args::init(argc, argv) };
}

// SAFETY: se llama una vez, al limpiar el runtime.
pub unsafe fn cleanup() {}

pub fn unsupported<T>() -> io::Result<T> {
    Err(unsupported_err())
}

pub fn unsupported_err() -> io::Error {
    io::const_error!(io::ErrorKind::Unsupported, "operation not supported on the PlayStation 2")
}

/// Termina el programa. El driver `cargo ps2 run` reconoce la línea que se
/// imprime para conocer el código de salida.
pub fn exit_process(code: i32) -> ! {
    let mut buf = [0u8; 32];
    let msg = format_exit(code, &mut buf);
    sio::write(msg);
    kernel::exit(code)
}

fn format_exit(code: i32, buf: &mut [u8; 32]) -> &[u8] {
    let prefix = b"[ps2rt] exit ";
    buf[..prefix.len()].copy_from_slice(prefix);
    let mut n = prefix.len();
    let mut digits = [0u8; 12];
    let mut len = 0;
    let negative = code < 0;
    let mut v = (code as i64).unsigned_abs();
    loop {
        digits[len] = b'0' + (v % 10) as u8;
        len += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if negative {
        buf[n] = b'-';
        n += 1;
    }
    for i in (0..len).rev() {
        buf[n] = digits[i];
        n += 1;
    }
    buf[n] = b'\n';
    &buf[..n + 1]
}

/// `process::abort` y los pánicos (con `panic = "abort"`) acaban aquí. Un
/// pánico sale con 101, como en el resto de plataformas de Rust; un aborto
/// explícito, con 134.
pub fn abort_internal() -> ! {
    let code = if crate::thread::panicking() { 101 } else { 134 };
    exit_process(code)
}

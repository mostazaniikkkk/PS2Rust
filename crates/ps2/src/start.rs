//! Arranque (crt0), stubs de syscall y acceso a registros de 64 bits.

use core::arch::global_asm;

// El ELF arranca aquí, con el kernel del EE ya inicializado por el BIOS.
global_asm!(
    r#"
    .section .text.start,"ax",@progbits
    .set push
    .set noreorder
    .set noat

    .globl _start
    .type _start,@function
_start:
    # 1. Poner .bss a cero.
    lui   $2, %hi(__bss_start)
    addiu $2, $2, %lo(__bss_start)
    lui   $3, %hi(__bss_end)
    addiu $3, $3, %lo(__bss_end)
1:  beq   $2, $3, 2f
    nop
    sw    $zero, 0($2)
    addiu $2, $2, 4
    b     1b
    nop                               # delay slot vacío: errata de bucles cortos
2:
    # 2. SetupThread(gp, stack, stack_size, args, root): hilo principal y pila.
    move  $4, $zero                   # gp: no se usa (código sin PIC)
    addiu $5, $zero, -1               # stack = -1: al final de la RAM
    lui   $6, %hi(__stack_size)
    addiu $6, $6, %lo(__stack_size)
    lui   $7, %hi(__ps2_args)
    addiu $7, $7, %lo(__ps2_args)
    lui   $8, %hi(__ps2_root)
    addiu $8, $8, %lo(__ps2_root)
    addiu $3, $zero, 60
    syscall
    move  $sp, $2

    # 3. SetupHeap(_end, -1): el heap llega hasta la base de la pila.
    lui   $4, %hi(_end)
    addiu $4, $4, %lo(_end)
    addiu $5, $zero, -1
    addiu $3, $zero, 61
    syscall
    lui   $8, %hi(__ps2_heap_end)
    sw    $2, %lo(__ps2_heap_end)($8)

    # 4. FlushCache(0): baja a RAM lo que quedó en la caché de datos.
    move  $4, $zero
    addiu $3, $zero, 100
    syscall

    .word 0x42000038                  # ei
    addiu $sp, $sp, -32               # área de argumentos del ABI o32
    jal   __ps2_rust_start
    nop

    # 5. Exit(código) si main retorna.
    move  $4, $2
    addiu $3, $zero, 4
    syscall
3:  b     3b
    nop
    .size _start, . - _start

    # Función raíz del hilo principal: ExitThread().
    .globl __ps2_root
    .type __ps2_root,@function
__ps2_root:
    addiu $3, $zero, 35
    syscall
4:  b     4b
    nop
    .size __ps2_root, . - __ps2_root

    .set pop

    .section .bss.__ps2_args,"aw",@nobits
    .p2align 4
    .globl __ps2_args
__ps2_args:
    .space 512
    .globl __ps2_heap_end
__ps2_heap_end:
    .space 16
    "#
);

extern "Rust" {
    fn __ps2_main() -> i32;
}

/// Primer código Rust que se ejecuta, ya con pila y `.bss` limpio.
#[no_mangle]
extern "C" fn __ps2_rust_start() -> i32 {
    unsafe { crate::heap::init() };
    let code = unsafe { __ps2_main() };
    crate::exit(code)
}

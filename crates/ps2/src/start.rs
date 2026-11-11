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

    .section .text.__ps2_syscall,"ax",@progbits
    # u32 __ps2_syscall(a0, a1, a2, a3, numero)
    .globl __ps2_syscall
    .type __ps2_syscall,@function
__ps2_syscall:
    lw    $3, 16($sp)
    syscall
    jr    $ra
    nop
    .size __ps2_syscall, . - __ps2_syscall

    .section .text.__ps2_write64,"ax",@progbits
    # void __ps2_write64(addr, lo, hi): una sola escritura de 64 bits (sd).
    .globl __ps2_write64
    .type __ps2_write64,@function
__ps2_write64:
    .word 0x0005283C                  # dsll32 $5, $5, 0
    .word 0x0005283E                  # dsrl32 $5, $5, 0   (lo sin extensión de signo)
    .word 0x0006303C                  # dsll32 $6, $6, 0
    or    $5, $5, $6
    .word 0xFC850000                  # sd     $5, 0($4)
    jr    $ra
    nop
    .size __ps2_write64, . - __ps2_write64

    .section .text.__ps2_read64,"ax",@progbits
    # void __ps2_read64(addr, out: *mut [u32; 2]): una sola lectura de 64 bits (ld).
    .globl __ps2_read64
    .type __ps2_read64,@function
__ps2_read64:
    .word 0xDC820000                  # ld     $2, 0($4)
    sw    $2, 0($5)
    .word 0x0002103F                  # dsra32 $2, $2, 0
    sw    $2, 4($5)
    jr    $ra
    nop
    .size __ps2_read64, . - __ps2_read64

    .section .text.__ps2_cop0_count,"ax",@progbits
    # u32 __ps2_cop0_count(): registro Count del COP0 (un tick por ciclo de CPU).
    .globl __ps2_cop0_count
    .type __ps2_cop0_count,@function
__ps2_cop0_count:
    .word 0x40024800                  # mfc0 $2, $9
    jr    $ra
    nop
    .size __ps2_cop0_count, . - __ps2_cop0_count

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

//! Llamadas al kernel del Emotion Engine y acceso a hardware mapeado en memoria.

core::arch::global_asm!(
    r#"
    .set push
    .set noreorder
    .set noat
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
    "#
);

extern "C" {
    fn __ps2_syscall(a0: u32, a1: u32, a2: u32, a3: u32, number: u32) -> u32;
    fn __ps2_write64(addr: u32, lo: u32, hi: u32);
    fn __ps2_read64(addr: u32, out: *mut [u32; 2]);
    fn __ps2_cop0_count() -> u32;
}

/// Ejecuta una syscall del kernel del EE con hasta cuatro argumentos.
///
/// # Safety
/// El número y los argumentos deben ser válidos para el kernel.
pub unsafe fn syscall(number: u32, a0: u32, a1: u32, a2: u32, a3: u32) -> u32 {
    __ps2_syscall(a0, a1, a2, a3, number)
}

/// `Exit`: termina el programa y vuelve al navegador de la consola.
pub fn exit(code: i32) -> ! {
    unsafe { syscall(4, code as u32, 0, 0, 0) };
    loop {}
}

/// `SetGsCrt`: configura el modo de vídeo del CRTC.
pub fn set_gs_crt(interlace: bool, mode: u32, frame_mode: bool) {
    unsafe { syscall(2, interlace as u32, mode, frame_mode as u32, 0) };
}

/// `GsPutIMR`: máscara de interrupciones del GS.
pub fn gs_put_imr(mask: u32) {
    unsafe { syscall(113, mask, 0, 0, 0) };
}

/// `FlushCache`: 0 = caché de datos, 2 = caché de instrucciones.
pub fn flush_cache(mode: u32) {
    unsafe { syscall(100, mode, 0, 0, 0) };
}

/// Escribe un registro de hardware de 64 bits con una única instrucción `sd`.
///
/// # Safety
/// `addr` debe ser un registro de 64 bits válido.
pub unsafe fn write64(addr: u32, value: u64) {
    __ps2_write64(addr, value as u32, (value >> 32) as u32)
}

/// Lee un registro de hardware de 64 bits con una única instrucción `ld`.
///
/// # Safety
/// `addr` debe ser un registro de 64 bits válido.
pub unsafe fn read64(addr: u32) -> u64 {
    let mut out = [0u32; 2];
    __ps2_read64(addr, &mut out);
    out[0] as u64 | (out[1] as u64) << 32
}

/// Escribe un registro de hardware de 32 bits.
///
/// # Safety
/// `addr` debe ser un registro de 32 bits válido.
pub unsafe fn write32(addr: u32, value: u32) {
    core::ptr::write_volatile(addr as *mut u32, value)
}

/// Lee un registro de hardware de 32 bits.
///
/// # Safety
/// `addr` debe ser un registro de 32 bits válido.
pub unsafe fn read32(addr: u32) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

/// Contador de ciclos de la CPU (registro Count del COP0, 32 bits).
pub fn cpu_ticks() -> u32 {
    unsafe { __ps2_cop0_count() }
}

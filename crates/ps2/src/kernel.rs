//! Llamadas al kernel del Emotion Engine y acceso a hardware mapeado en memoria.

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

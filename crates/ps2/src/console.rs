//! Consola de texto del EE: el puerto serie (SIO) del Emotion Engine.
//!
//! En PCSX2 aparece en el log como consola del EE; en una consola real sale
//! por la UART interna del EE.

use core::fmt;

const SIO_ISR: *const u32 = 0x1000_F130 as *const u32;
const SIO_TXFIFO: *mut u8 = 0x1000_F180 as *mut u8;

/// Envía un byte por la consola.
pub fn put_byte(b: u8) {
    unsafe {
        // Espera a que haya sitio en la FIFO de transmisión (con límite, por
        // si no hay nadie vaciándola).
        let mut spins = 0u32;
        while core::ptr::read_volatile(SIO_ISR) & 0xf000 == 0x8000 && spins < 100_000 {
            spins += 1;
        }
        core::ptr::write_volatile(SIO_TXFIFO, b);
    }
}

/// Escritor de la consola del EE; implementa [`core::fmt::Write`].
pub struct Console;

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            put_byte(b);
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = fmt::Write::write_fmt(&mut Console, args);
}

/// Imprime en la consola del EE.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::console::_print(format_args!($($arg)*)) };
}

/// Imprime en la consola del EE, con salto de línea.
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::console::_print(format_args!("{}\n", format_args!($($arg)*))) };
}

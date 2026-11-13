//! Consola del IOP (el `printf` del módulo `stdio` de la BIOS). En PCSX2
//! aparece en el log como consola del IOP.

use crate::kernel;
use core::fmt;

/// Escritor con un búfer pequeño en la pila: se vacía en cada salto de línea
/// o al llenarse.
pub struct Console {
    buf: [u8; 256],
    len: usize,
}

impl Console {
    pub const fn new() -> Console {
        Console { buf: [0; 256], len: 0 }
    }

    pub fn flush(&mut self) {
        if self.len == 0 {
            return;
        }
        self.buf[self.len] = 0;
        unsafe { kernel::printf(c"%s".as_ptr().cast(), self.buf.as_ptr()) };
        self.len = 0;
    }
}

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            // Un 0 cortaría la cadena de C.
            self.buf[self.len] = if b == 0 { b'?' } else { b };
            self.len += 1;
            if b == b'\n' || self.len == self.buf.len() - 1 {
                self.flush();
            }
        }
        Ok(())
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        self.flush();
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let mut c = Console::new();
    let _ = fmt::Write::write_fmt(&mut c, args);
}

/// Imprime en la consola del IOP.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::console::_print(format_args!($($arg)*)) };
}

/// Imprime en la consola del IOP, con salto de línea.
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::console::_print(format_args!("{}\n", format_args!($($arg)*))) };
}

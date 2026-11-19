//! La conexión con los módulos de mandos de la BIOS.
//!
//! Hay dos generaciones, con el mismo juego de órdenes pero distinta
//! numeración y distinto formato de datos: `XPADMAN` (casi todas las
//! consolas) y el `PADMAN` original. Las órdenes van por RPC; el estado del
//! mando no: el IOP lo deja una vez por frame en una zona de memoria del EE,
//! alternando entre dos copias.

use crate::{Input, State};
use std::cell::RefCell;
use std::io;
use std::os::ps2::{flush_data_cache, load_iop_module, SifRpcClient};
use std::rc::Rc;

/// Qué módulos de la BIOS se usan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    /// `rom0:XSIO2MAN` + `rom0:XPADMAN`.
    New,
    /// `rom0:SIO2MAN` + `rom0:PADMAN`, los de las primeras consolas.
    Old,
}

// Órdenes, con la numeración de XPADMAN. En PADMAN son 0x80000100 + n - 1.
const PORT_OPEN: u32 = 0x01;
const SET_MAIN_MODE: u32 = 0x06;
const SET_ACT_DIRECT: u32 = 0x07;
const SET_ACT_ALIGN: u32 = 0x08;
const SET_BUTTON_INFO: u32 = 0x0A;
const PORT_CLOSE: u32 = 0x0E;
const INIT: u32 = 0x10;

// Estados que publica el módulo.
const RAW_DISCONNECTED: u8 = 0;
const RAW_NO_CONFIG: u8 = 2;
const RAW_EXECUTING: u8 = 5;
const RAW_STABLE: u8 = 6;
const REQUEST_BUSY: u8 = 2;

#[repr(C, align(64))]
struct Block<const N: usize>([u8; N]);

fn uncached<T>(p: *const T) -> *mut u8 {
    (p as usize | 0x2000_0000) as *mut u8
}

struct Rpc {
    client: Box<SifRpcClient>,
    buffer: Box<Block<128>>,
    driver: Driver,
    /// Zona donde XPADMAN apunta qué puertos tienen algo conectado.
    _status: Box<Block<256>>,
    open: [bool; 2],
}

impl Rpc {
    /// Envía una orden: `words` desde la palabra 1, `bytes` desde el byte 12.
    /// Devuelve la respuesta entera.
    fn command(&mut self, command: u32, words: &[u32], bytes: &[u8]) -> [u32; 8] {
        let number = match self.driver {
            Driver::New => command,
            Driver::Old => 0x8000_0100 + command - 1,
        };
        let buffer = uncached(self.buffer.0.as_ptr());
        let mut reply = [0u32; 8];
        unsafe {
            buffer.write_bytes(0, 128);
            buffer.copy_from_nonoverlapping(number.to_le_bytes().as_ptr(), 4);
            for (i, word) in words.iter().enumerate() {
                buffer.add(4 + i * 4).copy_from_nonoverlapping(word.to_le_bytes().as_ptr(), 4);
            }
            buffer.add(12).copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
            self.client.call(1, buffer, 128, buffer, 128);
            for (i, word) in reply.iter_mut().enumerate() {
                let mut raw = [0u8; 4];
                buffer.add(i * 4).copy_to_nonoverlapping(raw.as_mut_ptr(), 4);
                *word = u32::from_le_bytes(raw);
            }
        }
        reply
    }
}

/// Los puertos de mando de la consola. Sólo puede haber uno.
pub struct Pads {
    rpc: Rc<RefCell<Rpc>>,
}

impl Pads {
    /// Carga los módulos de mandos de la BIOS (los nuevos si la consola los
    /// tiene, si no los originales) y conecta con ellos.
    pub fn init() -> io::Result<Pads> {
        Pads::init_with(Driver::New).or_else(|_| Pads::init_with(Driver::Old))
    }

    /// Como [`init`](Pads::init), con una generación concreta de módulos.
    /// Las dos no pueden convivir: para cambiar hay que reiniciar el IOP
    /// (`std::os::ps2::reset_iop`).
    pub fn init_with(driver: Driver) -> io::Result<Pads> {
        let (server, modules) = match driver {
            Driver::New => (0x8000_0100, ["rom0:XSIO2MAN", "rom0:XPADMAN"]),
            Driver::Old => (0x8000_010F, ["rom0:SIO2MAN", "rom0:PADMAN"]),
        };
        let mut client = Box::new(SifRpcClient::new());
        // Puede que ya estén cargados.
        if !client.bind(server, 1) {
            for module in modules {
                if !load_iop_module(module).is_ok_and(|r| r >= 0) {
                    return Err(io::Error::new(io::ErrorKind::NotFound, format!("could not load {module}")));
                }
            }
            if !client.bind(server, 200) {
                return Err(io::Error::other("the pad module does not answer"));
            }
        }
        let mut rpc = Rpc { client, buffer: Box::new(Block([0; 128])), driver, _status: Box::new(Block([0; 256])), open: [false; 2] };
        flush_data_cache();
        if driver == Driver::New {
            let status = rpc._status.0.as_ptr() as u32;
            if rpc.command(INIT, &[0, 0, 0, status], &[])[3] != 1 {
                return Err(io::Error::other("the pad module did not start"));
            }
        }
        Ok(Pads { rpc: Rc::new(RefCell::new(rpc)) })
    }

    /// Los módulos en uso.
    pub fn driver(&self) -> Driver {
        self.rpc.borrow().driver
    }

    /// Abre un puerto: 0 es el puerto 1 de la consola y 1 el puerto 2. No
    /// hace falta que haya un mando conectado.
    pub fn open(&mut self, port: u8) -> io::Result<Pad> {
        if port > 1 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "the console has ports 0 and 1"));
        }
        let mut rpc = self.rpc.borrow_mut();
        if rpc.open[port as usize] {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "port already open"));
        }
        let area = Box::new(Block([0u8; 256]));
        flush_data_cache();
        // Las dos copias empiezan como "sin datos, ocupado".
        let layout = Layout::of(rpc.driver);
        let raw = uncached(area.0.as_ptr());
        unsafe {
            raw.write_bytes(0, 256);
            for frame in 0..2 {
                let base = raw.add(frame * layout.size);
                base.add(layout.data).write_bytes(0xFF, 32);
                base.add(layout.state).write(RAW_EXECUTING);
                base.add(layout.request).write(REQUEST_BUSY);
            }
        }
        if rpc.command(PORT_OPEN, &[port as u32, 0, 0, area.0.as_ptr() as u32], &[])[3] == 0 {
            return Err(io::Error::other("the pad module could not open the port"));
        }
        rpc.open[port as usize] = true;
        Ok(Pad { rpc: self.rpc.clone(), port, area, layout, setup: Setup::Waiting, wait_from: 0, rumble: None, last_rumble: (false, 0) })
    }
}

/// Dónde está cada dato en las copias que escribe el IOP.
#[derive(Clone, Copy)]
struct Layout {
    size: usize,
    counter: usize,
    state: usize,
    request: usize,
    data: usize,
    length: usize,
}

impl Layout {
    fn of(driver: Driver) -> Layout {
        match driver {
            Driver::New => Layout { size: 128, counter: 0x58, state: 0x70, request: 0x71, data: 0x00, length: 0x60 },
            Driver::Old => Layout { size: 64, counter: 0x00, state: 0x04, request: 0x05, data: 0x08, length: 0x28 },
        }
    }
}

/// Pasos de la puesta a punto de un mando recién conectado.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Setup {
    /// Esperando a que el mando esté estable para empezar.
    Waiting,
    /// Se pidió el modo analógico.
    Analog,
    /// Se pidió asignar los motores de vibración.
    Motors,
    /// Se pidió la lectura de presión.
    Pressure,
    Done,
}

/// Una copia del estado, ya leída.
struct Frame {
    counter: u32,
    state: u8,
    request: u8,
    data: [u8; 32],
    length: usize,
}

/// Un puerto de mando abierto.
pub struct Pad {
    rpc: Rc<RefCell<Rpc>>,
    port: u8,
    area: Box<Block<256>>,
    layout: Layout,
    setup: Setup,
    /// Contador de frames del módulo cuando se envió la última orden.
    wait_from: u32,
    /// ¿Tiene motores de vibración listos? `None` hasta saberlo.
    rumble: Option<bool>,
    last_rumble: (bool, u8),
}

impl Pad {
    /// La copia más reciente de las dos que escribe el IOP.
    fn frame(&self) -> Frame {
        let raw = uncached(self.area.0.as_ptr());
        let l = self.layout;
        let read = |index: usize| unsafe {
            let base = raw.add(index * l.size);
            let mut word = [0u8; 4];
            base.add(l.counter).copy_to_nonoverlapping(word.as_mut_ptr(), 4);
            let counter = u32::from_le_bytes(word);
            base.add(l.length).copy_to_nonoverlapping(word.as_mut_ptr(), 4);
            let mut data = [0u8; 32];
            base.add(l.data).copy_to_nonoverlapping(data.as_mut_ptr(), 32);
            Frame { counter, state: base.add(l.state).read_volatile(), request: base.add(l.request).read_volatile(), data, length: (u32::from_le_bytes(word) as usize).min(32) }
        };
        let (a, b) = (read(0), read(1));
        if a.counter < b.counter {
            b
        } else {
            a
        }
    }

    /// Puerto de la consola (0 o 1).
    pub fn port(&self) -> u8 {
        self.port
    }

    pub fn state(&self) -> State {
        let frame = self.frame();
        match frame.state {
            RAW_DISCONNECTED => State::Disconnected,
            RAW_NO_CONFIG => State::Ready,
            RAW_STABLE if frame.request != REQUEST_BUSY && self.setup == Setup::Done => State::Ready,
            _ => State::Busy,
        }
    }

    /// ¿Tiene el mando motores de vibración? `None` mientras no se sepa.
    pub fn has_rumble(&self) -> Option<bool> {
        self.rumble
    }

    fn command(&mut self, command: u32, words: &[u32], bytes: &[u8]) -> [u32; 8] {
        self.rpc.borrow_mut().command(command, &[&[self.port as u32, 0], words].concat(), bytes)
    }

    /// Avanza la puesta a punto: modo analógico (bloqueado, para que el botón
    /// ANALOG no lo cambie), motores y presión. Una orden cada vez, y la
    /// siguiente cuando el módulo ha terminado con la anterior.
    fn advance_setup(&mut self, frame: &Frame) {
        if frame.state == RAW_NO_CONFIG {
            // Mando sin modos que elegir: digital sin más.
            self.setup = Setup::Done;
            self.rumble = Some(false);
            return;
        }
        // El módulo tarda un par de frames en acusar recibo de una orden.
        let settled = frame.state == RAW_STABLE && frame.request != REQUEST_BUSY && frame.counter.wrapping_sub(self.wait_from) >= 3;
        if !settled {
            return;
        }
        self.wait_from = frame.counter;
        self.setup = match self.setup {
            Setup::Waiting => {
                // Modo 1 de la tabla del mando (analógico), bloqueado (3).
                if self.command(SET_MAIN_MODE, &[1, 3], &[])[5] == 1 {
                    Setup::Analog
                } else {
                    self.rumble = Some(false);
                    Setup::Done
                }
            }
            Setup::Analog => {
                // Motor pequeño en el byte 0 de las órdenes y grande en el 1.
                let accepted = self.command(SET_ACT_ALIGN, &[], &[0, 1, 0xFF, 0xFF, 0xFF, 0xFF])[5] == 1;
                self.rumble = Some(accepted);
                Setup::Motors
            }
            Setup::Motors => {
                // Los 12 botones con presión. Un DualShock 1 lo rechaza.
                if self.command(SET_BUTTON_INFO, &[0xFFF], &[])[4] == 1 {
                    Setup::Pressure
                } else {
                    Setup::Done
                }
            }
            Setup::Pressure | Setup::Done => Setup::Done,
        };
    }

    /// Lee el mando. Hay que llamarla una vez por frame: también es la que
    /// configura el mando cuando se conecta. `None` si no hay mando o se está
    /// configurando.
    pub fn poll(&mut self) -> Option<Input> {
        let frame = self.frame();
        if frame.state == RAW_DISCONNECTED {
            // Al volver a conectar se configura de nuevo.
            self.setup = Setup::Waiting;
            self.rumble = None;
            self.last_rumble = (false, 0);
            return None;
        }
        if self.setup != Setup::Done {
            self.advance_setup(&frame);
            return None;
        }
        if frame.length == 0 || !matches!(frame.state, RAW_STABLE | RAW_NO_CONFIG) {
            return None;
        }
        Input::parse(&frame.data[..frame.length])
    }

    /// Vibración: `small` enciende el motor pequeño (rápido) y `large` es la
    /// fuerza del grande, de 0 a 255. Se mantiene hasta que se cambie.
    /// Devuelve `false` si el mando no vibra o está ocupado.
    pub fn rumble(&mut self, small: bool, large: u8) -> bool {
        if self.setup != Setup::Done || self.rumble != Some(true) {
            return false;
        }
        if self.last_rumble == (small, large) {
            return true;
        }
        let accepted = self.command(SET_ACT_DIRECT, &[], &[small as u8, large, 0xFF, 0xFF, 0xFF, 0xFF])[5] == 1;
        if accepted {
            self.last_rumble = (small, large);
        }
        accepted
    }
}

impl Drop for Pad {
    fn drop(&mut self) {
        // El IOP deja de escribir en la zona antes de que se libere.
        self.command(PORT_CLOSE, &[0, 1], &[]);
        self.rpc.borrow_mut().open[self.port as usize] = false;
    }
}

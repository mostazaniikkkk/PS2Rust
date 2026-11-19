//! Mandos de la PlayStation 2.
//!
//! ```ignore
//! use ps2_pad::{Button, Pads};
//!
//! let mut pads = Pads::init()?;
//! let mut pad = pads.open(0)?;          // puerto 1 de la consola
//! loop {
//!     // Una vez por frame. `None` mientras no haya mando o se esté configurando.
//!     if let Some(input) = pad.poll() {
//!         if input.pressed(Button::Cross) {
//!             pad.rumble(false, 200);
//!         }
//!         let (x, y) = input.left_stick();   // -1.0..=1.0
//!     }
//! #   break;
//! }
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! Los mandos cuelgan del IOP. [`Pads::init`] carga los módulos de mandos de
//! la BIOS (`rom0:XSIO2MAN` y `rom0:XPADMAN`, o los antiguos `rom0:SIO2MAN` y
//! `rom0:PADMAN` en las primeras consolas) y habla con ellos por RPC. Al
//! conectar un DualShock, [`Pad::poll`] lo pone solo en modo analógico, con
//! botones sensibles a la presión y los motores de vibración preparados.
//!
//! [`Input`] y [`Button`] no dependen de la consola: también compilan en el PC.

#[cfg(target_os = "ps2")]
mod device;
#[cfg(target_os = "ps2")]
pub use device::{Driver, Pad, Pads};

/// Botones del mando. El valor es su bit en [`Input::buttons`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Button {
    Select = 0x0001,
    L3 = 0x0002,
    R3 = 0x0004,
    Start = 0x0008,
    Up = 0x0010,
    Right = 0x0020,
    Down = 0x0040,
    Left = 0x0080,
    L2 = 0x0100,
    R2 = 0x0200,
    L1 = 0x0400,
    R1 = 0x0800,
    Triangle = 0x1000,
    Circle = 0x2000,
    Cross = 0x4000,
    Square = 0x8000,
}

impl Button {
    /// Todos los botones, en el orden de sus bits.
    pub const ALL: [Button; 16] = [
        Button::Select,
        Button::L3,
        Button::R3,
        Button::Start,
        Button::Up,
        Button::Right,
        Button::Down,
        Button::Left,
        Button::L2,
        Button::R2,
        Button::L1,
        Button::R1,
        Button::Triangle,
        Button::Circle,
        Button::Cross,
        Button::Square,
    ];

    /// Posición de la presión de este botón en los datos del mando; `None`
    /// para los que no la miden (Select, Start, L3 y R3).
    fn pressure_index(self) -> Option<usize> {
        Some(match self {
            Button::Right => 0,
            Button::Left => 1,
            Button::Up => 2,
            Button::Down => 3,
            Button::Triangle => 4,
            Button::Circle => 5,
            Button::Cross => 6,
            Button::Square => 7,
            Button::L1 => 8,
            Button::R1 => 9,
            Button::L2 => 10,
            Button::R2 => 11,
            _ => return None,
        })
    }
}

/// Tipo de mando, según el modo en que está.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Sólo botones (o un DualShock con el modo analógico apagado).
    Digital,
    /// Botones y sticks.
    Analog,
    /// DualShock 2 con botones sensibles a la presión.
    Pressure,
    /// Otro periférico (volante, pistola…): su identificador.
    Other(u8),
}

/// Estado de un puerto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// No hay mando.
    Disconnected,
    /// Se está detectando o configurando el mando.
    Busy,
    /// Mando listo.
    Ready,
}

/// Lo que dice el mando en un instante.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input {
    /// Botones pulsados, un bit por botón (ver [`Button`]).
    pub buttons: u16,
    pub kind: Kind,
    /// Sticks: (x, y) de 0 a 255, con el centro en 128 y la `y` hacia abajo.
    /// En un mando digital están centrados.
    pub left: (u8, u8),
    pub right: (u8, u8),
    /// Presión de cada botón, de 0 a 255, en el orden del mando. Sólo con
    /// [`Kind::Pressure`]; si no, 255 para los pulsados.
    pub pressure: [u8; 12],
}

impl Input {
    /// Interpreta los datos que entrega el mando (al menos 4 bytes: estado,
    /// modo y botones). `None` si no son válidos.
    pub fn parse(data: &[u8]) -> Option<Input> {
        if data.len() < 4 || data[0] != 0 {
            return None;
        }
        // Los botones llegan en lógica negativa.
        let buttons = !u16::from_le_bytes([data[2], data[3]]);
        let kind = match data[1] >> 4 {
            4 => Kind::Digital,
            7 if data[1] & 15 >= 9 => Kind::Pressure,
            5 | 7 => Kind::Analog,
            _ => Kind::Other(data[1]),
        };
        let byte = |i: usize| data.get(i).copied();
        let analog = kind != Kind::Digital && data.len() >= 8;
        let stick = |i: usize| if analog { (data[i], data[i + 1]) } else { (128, 128) };
        let mut pressure = [0u8; 12];
        for button in Button::ALL {
            if let Some(i) = button.pressure_index() {
                let down = buttons & button as u16 != 0;
                pressure[i] = match (kind, byte(8 + i)) {
                    (Kind::Pressure, Some(p)) if down => p.max(1),
                    (Kind::Pressure, _) => 0,
                    _ => 255 * down as u8,
                };
            }
        }
        Some(Input { buttons, kind, right: stick(4), left: stick(6), pressure })
    }

    /// ¿Está pulsado el botón?
    pub fn pressed(&self, button: Button) -> bool {
        self.buttons & button as u16 != 0
    }

    /// Fuerza con la que se pulsa un botón, de 0 a 255. Los que no miden
    /// presión dan 0 o 255.
    pub fn pressure(&self, button: Button) -> u8 {
        match button.pressure_index() {
            Some(i) => self.pressure[i],
            None => 255 * self.pressed(button) as u8,
        }
    }

    fn axis(v: u8) -> f32 {
        ((v as f32 - 128.0) / 127.0).clamp(-1.0, 1.0)
    }

    /// Stick izquierdo de -1.0 a 1.0: `x` hacia la derecha, `y` hacia abajo.
    pub fn left_stick(&self) -> (f32, f32) {
        (Input::axis(self.left.0), Input::axis(self.left.1))
    }

    /// Stick derecho de -1.0 a 1.0.
    pub fn right_stick(&self) -> (f32, f32) {
        (Input::axis(self.right.0), Input::axis(self.right.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digital_pad() {
        // Cruz y Arriba pulsados.
        let input = Input::parse(&[0, 0x41, !0x10, !0x40]).unwrap();
        assert_eq!(input.kind, Kind::Digital);
        assert_eq!(input.buttons, 0x4010);
        assert!(input.pressed(Button::Cross) && input.pressed(Button::Up) && !input.pressed(Button::Start));
        assert_eq!((input.left, input.right), ((128, 128), (128, 128)));
        assert_eq!((input.pressure(Button::Cross), input.pressure(Button::Circle)), (255, 0));
        assert_eq!(input.left_stick(), (0.0, 0.0));
    }

    #[test]
    fn analog_and_pressure() {
        let mut data = [0u8; 32];
        data[1] = 0x73;
        data[2..8].copy_from_slice(&[0xff, 0xff, 255, 128, 0, 1]);
        let input = Input::parse(&data).unwrap();
        assert_eq!((input.kind, input.buttons), (Kind::Analog, 0));
        assert_eq!((input.right, input.left), ((255, 128), (0, 1)));
        assert_eq!(input.right_stick(), (1.0, 0.0));
        assert_eq!(input.left_stick().0, -1.0);
        // Con presión: Cruz a media fuerza, L3 pulsado (no mide presión).
        data[1] = 0x79;
        data[2..4].copy_from_slice(&(!0x4002u16).to_le_bytes());
        data[14] = 120;
        data[13] = 99; // Círculo no está pulsado: su presión no cuenta.
        let input = Input::parse(&data).unwrap();
        assert_eq!(input.kind, Kind::Pressure);
        assert_eq!((input.pressure(Button::Cross), input.pressure(Button::Circle), input.pressure(Button::L3)), (120, 0, 255));
    }

    #[test]
    fn invalid_data() {
        assert!(Input::parse(&[0xff, 0x41, 0xff, 0xff]).is_none());
        assert!(Input::parse(&[0, 0x41]).is_none());
        assert_eq!(Input::parse(&[0, 0x23, 0xff, 0xff]).unwrap().kind, Kind::Other(0x23));
        assert_eq!(Button::ALL.iter().fold(0u16, |m, &b| m | b as u16), 0xffff);
    }
}

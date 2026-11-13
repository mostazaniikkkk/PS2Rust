//! Capa de sistema operativo de la PlayStation 2 para la std de Rust.
//!
//! Sólo depende de `core`. La std la incluye como `sys::pal::ps2::os`;
//! `std::os::ps2` expone algunas funciones (carga de módulos del IOP, ciclos
//! de CPU, estado del heap).

pub mod cdvd;
pub mod fileio;
pub mod heap;
pub mod kernel;
pub mod loadfile;
pub mod sched;
pub mod sif;
pub mod sio;

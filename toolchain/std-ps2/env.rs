//! Variables de entorno: la PS2 no tiene; se guardan en memoria.

pub use super::common::Env;
use crate::collections::BTreeMap;
use crate::ffi::{OsStr, OsString};
use crate::io;
use crate::sync::Mutex;

static ENV: Mutex<BTreeMap<OsString, OsString>> = Mutex::new(BTreeMap::new());

pub fn env() -> Env {
    let vars = ENV.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    Env::new(vars)
}

pub fn getenv(k: &OsStr) -> Option<OsString> {
    ENV.lock().unwrap().get(k).cloned()
}

pub unsafe fn setenv(k: &OsStr, v: &OsStr) -> io::Result<()> {
    ENV.lock().unwrap().insert(k.to_owned(), v.to_owned());
    Ok(())
}

pub unsafe fn unsetenv(k: &OsStr) -> io::Result<()> {
    ENV.lock().unwrap().remove(k);
    Ok(())
}

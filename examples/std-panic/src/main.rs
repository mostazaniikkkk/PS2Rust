//! Pánico en un hilo secundario con la std: el mensaje sale por la consola
//! y el programa termina con código 101 (con `panic = "abort"` no hay
//! desenrollado, así que el pánico de cualquier hilo termina el programa).

use std::hint::black_box;
use std::thread;

fn main() {
    println!("antes del panico");
    let worker = thread::Builder::new().name("calculo".into()).spawn(|| {
        let data = vec![1u32, 2, 3, 4];
        let index = black_box(7);
        data[index]
    });
    let value = worker.unwrap().join();
    println!("esto no se imprime: {value:?}");
}

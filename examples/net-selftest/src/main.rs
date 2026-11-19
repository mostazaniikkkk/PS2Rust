//! Batería de `std::net` en la PlayStation 2. Habla con el servidor de
//! pruebas del PC (`toolchain/selftest-host --net-server`), cuya dirección
//! lee de `net-test.cfg`, junto al ejecutable. La salida debe ser idéntica a
//! la de la misma batería en el PC (`selftest-host --net`). El código de
//! salida es el número de fallos.

mod net_suite;
mod web_suite;

/// La consola de PCSX2 no conserva el texto que no es ASCII: va como `%XX`.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b == b'\n' || ((0x20..=0x7e).contains(&b) && !matches!(b, b'%' | b'\\' | b'~')) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn main() {
    let Some(config) = std::fs::read_to_string("net-test.cfg").ok().and_then(|text| net_suite::Config::parse(&text)) else {
        println!("falta net-test.cfg: arranca antes el servidor de pruebas (selftest-host --net-server <carpeta>)");
        std::process::exit(200);
    };
    let (mut out, mut failed) = net_suite::run(&config);
    let (web_out, web_failed) = web_suite::run(&config);
    out.push_str(&web_out);
    failed += web_failed;
    print!("{}", escaped(&out));
    std::process::exit(failed.min(199) as i32);
}

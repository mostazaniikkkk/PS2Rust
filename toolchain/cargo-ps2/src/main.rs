//! `cargo ps2` — driver del compilador de Rust para PlayStation 2.
//!
//! Orquesta el `rustc` instalado (backend MIPS de LLVM) con un target propio
//! (`mipsel-sony-ps2`), reconstruye `core`/`alloc` para él, enlaza con
//! `rust-lld` y después legaliza y verifica el código máquina para el R5900.

mod elf;
mod emu;
mod r5900;

use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

const TARGET_NAME: &str = "mipsel-sony-ps2";
const TARGET_JSON: &str = include_str!("../assets/mipsel-sony-ps2.json");
const LINKER_SCRIPT: &str = include_str!("../assets/ps2.ld");

const HELP: &str = "\
cargo ps2 — compilador de Rust para PlayStation 2 (Emotion Engine / R5900)

USO:
    cargo ps2 build [opciones] [-- <args extra de cargo>]
    cargo ps2 run   [opciones] [-- <args extra de cargo>]
    cargo ps2 check <archivo.elf>...
    cargo ps2 legalize <entrada> <salida.elf>
    cargo ps2 target-spec | linker-script

OPCIONES DE build / run:
    -p, --package <nombre>   Paquete a compilar (por defecto, el del directorio actual)
        --bin <nombre>       Binario concreto del paquete
        --debug              Perfil dev en vez de release
        --manifest-path <p>  Cargo.toml a usar
    -v, --verbose            Muestra los comandos y todo el log del emulador

OPCIONES DE run:
        --emulator <exe>     Ruta a pcsx2-qt.exe (o variable de entorno PCSX2_EXE)
        --timeout <seg>      Tiempo máximo de ejecución (por defecto 60)
        --until <texto>      Termina cuando la consola del EE imprima este texto
        --screenshot <png>   Captura la ventana del emulador antes de cerrar
        --log <archivo>      Guarda la salida de la consola del EE
        --keep-open          No cierra el emulador al terminar el programa

`check` verifica que un ELF cualquiera sólo use instrucciones del R5900.
El ejecutable final queda en target/ps2/<nombre>.elf.
";

struct Opts {
    package: Option<String>,
    bin: Option<String>,
    debug: bool,
    manifest: Option<String>,
    verbose: bool,
    emulator: Option<String>,
    timeout: u64,
    until: Option<String>,
    screenshot: Option<String>,
    log: Option<String>,
    keep_open: bool,
    extra: Vec<String>,
}

fn die(msg: impl AsRef<str>) -> ! {
    eprintln!("error: {}", msg.as_ref());
    exit(1)
}

fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    // Invocado como subcomando de cargo: `cargo ps2 …` => argv[1] == "ps2".
    if args.first().map(String::as_str) == Some("ps2") {
        args.remove(0);
    }
    let Some(cmd) = args.first().cloned() else {
        print!("{HELP}");
        return;
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "build" => {
            let o = parse_opts(rest);
            build(&o);
        }
        "run" => {
            let o = parse_opts(rest);
            let elfs = build(&o);
            if elfs.len() != 1 {
                die(format!("`run` necesita exactamente un binario y hay {}: usa -p/--bin", elfs.len()));
            }
            let code = emu::run(&elfs[0], &o).unwrap_or_else(|e| die(e));
            exit(code);
        }
        "check" => {
            if rest.is_empty() {
                die("uso: cargo ps2 check <archivo.elf>...");
            }
            let mut bad = false;
            for f in rest {
                let mut bytes = fs::read(f).unwrap_or_else(|e| die(format!("no pude leer {f}: {e}")));
                match process_elf(&mut bytes, false) {
                    Ok(rep) => {
                        if rep.errors.is_empty() {
                            println!("{f}: OK — {} instrucciones, todas válidas para el R5900", rep.scanned);
                        } else {
                            bad = true;
                            report_errors(f, &rep);
                        }
                    }
                    Err(e) => {
                        bad = true;
                        eprintln!("{f}: {e}");
                    }
                }
            }
            exit(bad as i32);
        }
        "legalize" => {
            if rest.len() != 2 {
                die("uso: cargo ps2 legalize <entrada> <salida.elf>");
            }
            legalize_file(Path::new(&rest[0]), Path::new(&rest[1]));
        }
        "target-spec" => print!("{TARGET_JSON}"),
        "linker-script" => print!("{LINKER_SCRIPT}"),
        "help" | "-h" | "--help" => print!("{HELP}"),
        other => die(format!("subcomando desconocido `{other}`; prueba `cargo ps2 help`")),
    }
}

fn parse_opts(args: &[String]) -> Opts {
    let mut o = Opts {
        package: None,
        bin: None,
        debug: false,
        manifest: None,
        verbose: false,
        emulator: None,
        timeout: 60,
        until: None,
        screenshot: None,
        log: None,
        keep_open: false,
        extra: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| -> String {
            it.next().cloned().unwrap_or_else(|| die(format!("falta el valor de {name}")))
        };
        match a.as_str() {
            "-p" | "--package" => o.package = Some(val(a)),
            "--bin" => o.bin = Some(val(a)),
            "--debug" => o.debug = true,
            "--release" => o.debug = false,
            "--manifest-path" => o.manifest = Some(val(a)),
            "-v" | "--verbose" => o.verbose = true,
            "--emulator" => o.emulator = Some(val(a)),
            "--timeout" => o.timeout = val(a).parse().unwrap_or_else(|_| die("--timeout espera segundos")),
            "--until" => o.until = Some(val(a)),
            "--screenshot" => o.screenshot = Some(val(a)),
            "--log" => o.log = Some(val(a)),
            "--keep-open" => o.keep_open = true,
            "--" => {
                o.extra.extend(it.by_ref().cloned());
            }
            other => die(format!("opción desconocida `{other}`")),
        }
    }
    o
}

/// Directorio `target` del workspace sobre el que se está trabajando.
fn target_dir(o: &Opts) -> PathBuf {
    let mut c = Command::new(cargo());
    c.args(["metadata", "--format-version", "1", "--no-deps"]);
    if let Some(m) = &o.manifest {
        c.args(["--manifest-path", m]);
    }
    let out = c.stderr(Stdio::inherit()).output().unwrap_or_else(|e| die(format!("no pude ejecutar cargo: {e}")));
    if !out.status.success() {
        exit(out.status.code().unwrap_or(1));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    match json_string_field(&text, "target_directory") {
        Some(d) => PathBuf::from(d),
        None => die("cargo metadata no devolvió target_directory"),
    }
}

fn cargo() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// Extrae `"campo":"valor"` de una línea JSON, deshaciendo los escapes.
fn json_string_field(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\":\"");
    let start = json.find(&key)? + key.len();
    let mut out = String::new();
    let mut chars = json[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// Compila, enlaza, legaliza y verifica. Devuelve los `.elf` finales.
fn build(o: &Opts) -> Vec<PathBuf> {
    let target = target_dir(o);
    let support = target.join("ps2-toolchain");
    let out_dir = target.join("ps2");
    fs::create_dir_all(&support).unwrap_or_else(|e| die(format!("no pude crear {}: {e}", support.display())));
    fs::create_dir_all(&out_dir).unwrap_or_else(|e| die(format!("no pude crear {}: {e}", out_dir.display())));
    let spec = support.join(format!("{TARGET_NAME}.json"));
    let script = support.join("ps2.ld");
    write_if_changed(&spec, TARGET_JSON);
    write_if_changed(&script, LINKER_SCRIPT);

    let script_arg = script.to_string_lossy().replace('\\', "/");
    let rustflags = format!(
        "target.{TARGET_NAME}.rustflags=['-Clink-arg=-T{script_arg}', '-Clink-arg=--gc-sections', '-Clink-arg=-znorelro']"
    );

    let mut c = Command::new(cargo());
    c.arg("build");
    if !o.debug {
        c.arg("--release");
    }
    c.args(["-Zbuild-std=core,alloc,compiler_builtins", "-Zbuild-std-features=compiler-builtins-mem"]);
    c.arg("-Zjson-target-spec");
    c.arg("--target").arg(&spec);
    c.arg("--config").arg(&rustflags);
    c.arg("--message-format=json-render-diagnostics");
    if let Some(m) = &o.manifest {
        c.args(["--manifest-path", m]);
    }
    if let Some(p) = &o.package {
        c.args(["-p", p]);
    }
    if let Some(b) = &o.bin {
        c.args(["--bin", b]);
    }
    c.args(&o.extra);
    // El target y build-std son inestables: se habilitan sobre el rustc estable.
    c.env("RUSTC_BOOTSTRAP", "1");
    c.stdout(Stdio::piped());
    if o.verbose {
        eprintln!("+ {c:?}");
    }
    let mut child = c.spawn().unwrap_or_else(|e| die(format!("no pude ejecutar cargo: {e}")));
    let mut executables = Vec::new();
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let Ok(line) = line else { break };
        if line.contains("\"reason\":\"compiler-artifact\"") {
            if let Some(exe) = json_string_field(&line, "executable") {
                executables.push(PathBuf::from(exe));
            }
        }
    }
    let status = child.wait().unwrap_or_else(|e| die(format!("cargo falló: {e}")));
    if !status.success() {
        exit(status.code().unwrap_or(1));
    }
    if executables.is_empty() {
        die("la compilación no produjo ningún ejecutable (¿el paquete es una biblioteca?)");
    }

    let mut elfs = Vec::new();
    for exe in executables {
        let name = exe.file_name().unwrap().to_string_lossy().into_owned();
        let out = out_dir.join(format!("{name}.elf"));
        legalize_file(&exe, &out);
        elfs.push(out);
    }
    elfs
}

fn write_if_changed(path: &Path, content: &str) {
    if fs::read_to_string(path).map(|c| c == content).unwrap_or(false) {
        return;
    }
    fs::write(path, content).unwrap_or_else(|e| die(format!("no pude escribir {}: {e}", path.display())));
}

fn process_elf(bytes: &mut [u8], apply: bool) -> Result<r5900::Report, String> {
    let info = elf::parse(bytes)?;
    if info.load_start < 0x0010_0000 || info.load_end > 0x0200_0000 {
        return Err(format!(
            "el programa ocupa {:#010x}..{:#010x}, fuera de la RAM de usuario del PS2 (0x00100000..0x02000000)",
            info.load_start, info.load_end
        ));
    }
    if info.entry < info.load_start || info.entry >= info.load_end {
        return Err(format!("el punto de entrada {:#010x} está fuera del programa", info.entry));
    }
    let mut rep = r5900::Report::default();
    for c in &info.code {
        r5900::process(&mut bytes[c.offset..c.offset + c.size], c.addr, apply, &mut rep);
    }
    Ok(rep)
}

fn report_errors(file: &str, rep: &r5900::Report) {
    eprintln!("{file}: {} instrucciones incompatibles con el R5900:", rep.errors.len());
    for e in rep.errors.iter().take(40) {
        eprintln!("  {:#010x}: {:08x}  {}", e.addr, e.word, e.msg);
    }
    if rep.errors.len() > 40 {
        eprintln!("  … y {} más", rep.errors.len() - 40);
    }
}

fn legalize_file(input: &Path, output: &Path) {
    let mut bytes = fs::read(input).unwrap_or_else(|e| die(format!("no pude leer {}: {e}", input.display())));
    let rep = process_elf(&mut bytes, true).unwrap_or_else(|e| die(format!("{}: {e}", input.display())));
    if !rep.errors.is_empty() {
        report_errors(&input.display().to_string(), &rep);
        exit(1);
    }
    // Segunda pasada en modo estricto: lo que se escribe debe ser R5900 puro.
    let strict = process_elf(&mut bytes.clone(), false).unwrap_or_else(|e| die(e));
    if !strict.errors.is_empty() {
        report_errors(&output.display().to_string(), &strict);
        die("la legalización dejó instrucciones inválidas (bug del legalizador)");
    }
    fs::write(output, &bytes).unwrap_or_else(|e| die(format!("no pude escribir {}: {e}", output.display())));
    let detail: Vec<String> = rep.patched.iter().map(|(k, v)| format!("{v}× {k}")).collect();
    eprintln!(
        "    R5900: {} instrucciones verificadas, {} legalizadas{}",
        rep.scanned,
        rep.total_patches(),
        if detail.is_empty() { String::new() } else { format!(" ({})", detail.join(", ")) }
    );
    eprintln!("      ELF: {} ({} bytes)", output.display(), bytes.len());
}

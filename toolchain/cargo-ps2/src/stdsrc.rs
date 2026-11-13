//! Prepara una copia de la biblioteca estándar con la plataforma PS2.
//!
//! Se copia `library/` del componente `rust-src` instalado, se insertan las
//! ramas `target_os = "ps2"` en los selectores de plataforma de std y se
//! añaden los archivos de `toolchain/std-ps2`. Cada parche parte del archivo
//! original y comprueba que su ancla aparece exactamente una vez: si una
//! versión de Rust cambia esos archivos, se avisa en vez de compilar algo roto.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Archivos propios: (ruta dentro de `library/std/src`, contenido).
const FILES: &[(&str, &str)] = &[
    ("sys/pal/ps2/mod.rs", include_str!("../../std-ps2/pal.rs")),
    ("sys/pal/ps2/futex.rs", include_str!("../../std-ps2/futex.rs")),
    ("sys/pal/ps2/os/mod.rs", include_str!("../../std-ps2/os/mod.rs")),
    ("sys/pal/ps2/os/cdvd.rs", include_str!("../../std-ps2/os/cdvd.rs")),
    ("sys/pal/ps2/os/fileio.rs", include_str!("../../std-ps2/os/fileio.rs")),
    ("sys/pal/ps2/os/heap.rs", include_str!("../../std-ps2/os/heap.rs")),
    ("sys/pal/ps2/os/kernel.rs", include_str!("../../std-ps2/os/kernel.rs")),
    ("sys/pal/ps2/os/loadfile.rs", include_str!("../../std-ps2/os/loadfile.rs")),
    ("sys/pal/ps2/os/sched.rs", include_str!("../../std-ps2/os/sched.rs")),
    ("sys/pal/ps2/os/sif.rs", include_str!("../../std-ps2/os/sif.rs")),
    ("sys/pal/ps2/os/sio.rs", include_str!("../../std-ps2/os/sio.rs")),
    ("sys/alloc/ps2.rs", include_str!("../../std-ps2/alloc.rs")),
    ("sys/args/ps2.rs", include_str!("../../std-ps2/args.rs")),
    ("sys/env/ps2.rs", include_str!("../../std-ps2/env.rs")),
    ("sys/fs/ps2.rs", include_str!("../../std-ps2/fs.rs")),
    ("sys/io/error/ps2.rs", include_str!("../../std-ps2/io_error.rs")),
    ("sys/paths/ps2.rs", include_str!("../../std-ps2/paths.rs")),
    ("sys/random/ps2.rs", include_str!("../../std-ps2/random.rs")),
    ("sys/stdio/ps2.rs", include_str!("../../std-ps2/stdio.rs")),
    ("sys/thread/ps2.rs", include_str!("../../std-ps2/thread.rs")),
    ("sys/thread_local/key/ps2.rs", include_str!("../../std-ps2/tls.rs")),
    ("sys/time/ps2.rs", include_str!("../../std-ps2/time.rs")),
    ("os/ps2/mod.rs", include_str!("../../std-ps2/os_ext.rs")),
    ("os/ps2/ffi.rs", include_str!("../../std-ps2/os_ext_ffi.rs")),
];

const SELECT: &str = "cfg_select! {\n";

/// Un parche: inserta `text` justo después (o antes) de `anchor`.
struct Patch {
    file: &'static str,
    anchor: &'static str,
    text: &'static str,
    before: bool,
}

const fn after(file: &'static str, anchor: &'static str, text: &'static str) -> Patch {
    Patch { file, anchor, text, before: false }
}

const fn before(file: &'static str, anchor: &'static str, text: &'static str) -> Patch {
    Patch { file, anchor, text, before: true }
}

const PATCHES: &[Patch] = &[
    after("build.rs", "        || target_os == \"vexos\"\n", "        || target_os == \"ps2\"\n"),
    after("src/os/mod.rs", "#[cfg(target_os = \"xous\")]\npub mod xous;\n", "#[cfg(target_os = \"ps2\")]\npub mod ps2;\n"),
    before(
        "src/sys/env_consts.rs",
        "// The fallback when none of the other gates match.",
        "#[cfg(target_os = \"ps2\")]\npub mod os {\n    pub const FAMILY: &str = \"\";\n    pub const OS: &str = \"ps2\";\n    pub const DLL_PREFIX: &str = \"\";\n    pub const DLL_SUFFIX: &str = \"\";\n    pub const DLL_EXTENSION: &str = \"\";\n    pub const EXE_SUFFIX: &str = \".elf\";\n    pub const EXE_EXTENSION: &str = \"elf\";\n}\n\n",
    ),
    after("src/sys/pal/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod ps2;\n        pub use self::ps2::*;\n    }\n"),
    after("src/sys/alloc/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod ps2;\n    }\n"),
    after("src/sys/stdio/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod ps2;\n        pub use ps2::*;\n    }\n"),
    after(
        "src/sys/time/mod.rs",
        SELECT,
        "    target_os = \"ps2\" => {\n        mod ps2;\n        use ps2 as imp;\n        pub(crate) use ps2::from_ps2_datetime;\n    }\n",
    ),
    after(
        "src/sys/thread/mod.rs",
        SELECT,
        "    target_os = \"ps2\" => {\n        mod ps2;\n        pub use ps2::{Thread, available_parallelism, current_os_id, set_name, sleep, yield_now, DEFAULT_MIN_STACK_SIZE};\n    }\n",
    ),
    after(
        "src/sys/fs/mod.rs",
        SELECT,
        "    target_os = \"ps2\" => {\n        mod ps2;\n        use ps2 as imp;\n        pub use ps2::ps2_resolve;\n    }\n",
    ),
    after("src/sys/random/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod ps2;\n        pub use ps2::fill_bytes;\n    }\n"),
    after("src/sys/io/error/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod ps2;\n        pub use ps2::*;\n    }\n"),
    before("src/sys/args/mod.rs", SELECT, "#[cfg(target_os = \"ps2\")]\nmod common;\n\n"),
    after("src/sys/args/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod ps2;\n        pub use ps2::*;\n    }\n"),
    before("src/sys/env/mod.rs", SELECT, "#[cfg(target_os = \"ps2\")]\nmod common;\n\n"),
    after("src/sys/env/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod ps2;\n        pub use ps2::*;\n    }\n"),
    after(
        "src/sys/paths/mod.rs",
        SELECT,
        "    target_os = \"ps2\" => {\n        mod ps2;\n        #[allow(dead_code)]\n        mod unsupported;\n        mod imp {\n            pub use super::ps2::{getcwd, chdir, current_exe, temp_dir, home_dir};\n            pub use super::unsupported::{SplitPaths, split_paths, JoinPathsError, join_paths};\n        }\n    }\n",
    ),
    after(
        "src/sys/exit.rs",
        "pub fn exit(code: i32) -> ! {\n    cfg_select! {\n",
        "        target_os = \"ps2\" => {\n            crate::sys::exit_process(code)\n        }\n",
    ),
    after("src/sys/sync/mutex/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod futex;\n        pub use futex::Mutex;\n    }\n"),
    after("src/sys/sync/condvar/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod futex;\n        pub use futex::Condvar;\n    }\n"),
    after("src/sys/sync/rwlock/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod futex;\n        pub use futex::RwLock;\n    }\n"),
    after("src/sys/sync/once/mod.rs", SELECT, "    target_os = \"ps2\" => {\n        mod futex;\n        pub use futex::{Once, OnceState};\n    }\n"),
    after(
        "src/sys/sync/thread_parking/mod.rs",
        SELECT,
        "    target_os = \"ps2\" => {\n        mod futex;\n        pub use futex::Parker;\n    }\n",
    ),
    // Almacenamiento por hilo: claves propias y destructores llamados por el
    // propio runtime al terminar cada hilo.
    before(
        "src/sys/thread_local/mod.rs",
        "        any(\n            target_os = \"hermit\",\n            target_os = \"xous\",\n        ) => {",
        "        target_os = \"ps2\" => {\n            pub(crate) fn enable() {}\n        }\n",
    ),
    before(
        "src/sys/thread_local/mod.rs",
        "        target_os = \"xous\" => {\n            mod racy;",
        "        target_os = \"ps2\" => {\n            mod racy;\n            mod ps2;\n            pub(super) use racy::LazyKey;\n            pub(crate) use ps2::destroy_tls;\n            pub(super) use ps2::{Key, get, set};\n            use ps2::{create, destroy};\n        }\n",
    ),
];

fn rustc_info() -> Result<(PathBuf, String), String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let sysroot = Command::new(&rustc)
        .args(["--print", "sysroot"])
        .output()
        .map_err(|e| format!("no pude ejecutar rustc: {e}"))?;
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    let vv = Command::new(&rustc).arg("-vV").output().map_err(|e| format!("no pude ejecutar rustc: {e}"))?;
    let vv = String::from_utf8_lossy(&vv.stdout).into_owned();
    let hash = vv
        .lines()
        .find_map(|l| l.strip_prefix("commit-hash: "))
        .unwrap_or("desconocido")
        .chars()
        .take(12)
        .collect::<String>();
    Ok((sysroot, hash))
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn write_if_changed(path: &Path, content: &str) -> io::Result<()> {
    if fs::read_to_string(path).map(|c| c == content).unwrap_or(false) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, content)
}

/// Devuelve la ruta de la `library/` parcheada, lista para `-Zbuild-std`.
pub fn prepare(cache_dir: &Path) -> Result<PathBuf, String> {
    let (sysroot, hash) = rustc_info()?;
    let original = sysroot.join("lib/rustlib/src/rust/library");
    if !original.join("std/src/lib.rs").is_file() {
        return Err(format!(
            "no encuentro el código de la std en {}: instala `rustup component add rust-src`",
            original.display()
        ));
    }
    let base = cache_dir.join(format!("std-{hash}"));
    let library = base.join("library");
    let marker = base.join(".copia-completa");
    if !marker.is_file() {
        eprintln!("    Preparando la std para PS2 (copia única de rust-src en {})", base.display());
        let _ = fs::remove_dir_all(&base);
        copy_dir(&original, &library).map_err(|e| format!("no pude copiar rust-src: {e}"))?;
        fs::write(&marker, "").map_err(|e| e.to_string())?;
    }

    // Parches: siempre desde el original, así son idempotentes.
    let mut files: Vec<&str> = PATCHES.iter().map(|p| p.file).collect();
    files.sort();
    files.dedup();
    for file in files {
        let src = original.join("std").join(file);
        let mut text = fs::read_to_string(&src).map_err(|e| format!("no pude leer {}: {e}", src.display()))?;
        for p in PATCHES.iter().filter(|p| p.file == file) {
            let count = text.matches(p.anchor).count();
            if count != 1 {
                return Err(format!(
                    "esta versión de rust-src no es compatible con la std para PS2: el ancla de {} aparece {} veces \
                     (se esperaba 1). Probado con rustc 1.97.1.\n  ancla: {:?}",
                    file, count, p.anchor
                ));
            }
            let at = text.find(p.anchor).unwrap();
            let pos = if p.before { at } else { at + p.anchor.len() };
            text.insert_str(pos, p.text);
        }
        write_if_changed(&library.join("std").join(file), &text).map_err(|e| e.to_string())?;
    }
    for (rel, content) in FILES {
        write_if_changed(&library.join("std/src").join(rel), content).map_err(|e| e.to_string())?;
    }
    Ok(library)
}

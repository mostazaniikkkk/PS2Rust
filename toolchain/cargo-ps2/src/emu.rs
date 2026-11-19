//! Ejecuta un ELF en PCSX2 y retransmite la consola del Emotion Engine.

use crate::Opts;
use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Línea que imprime el runtime (`ps2::exit`) cuando el programa termina.
const EXIT_MARKER: &str = "[ps2rt] exit ";

const SCREENSHOT_PS1: &str = r#"
param([int]$ProcId, [string]$Out)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Ps2Win {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
}
"@
[void][Ps2Win]::SetProcessDPIAware()
$p = Get-Process -Id $ProcId -ErrorAction Stop
$h = $p.MainWindowHandle
if ($h -eq [IntPtr]::Zero) { Write-Error "el emulador no tiene ventana"; exit 2 }
[void][Ps2Win]::ShowWindow($h, 9)
[void][Ps2Win]::SetForegroundWindow($h)
Start-Sleep -Milliseconds 700
$r = New-Object Ps2Win+RECT
[void][Ps2Win]::GetWindowRect($h, [ref]$r)
$w = $r.R - $r.L; $ht = $r.B - $r.T
if ($w -le 0 -or $ht -le 0) { Write-Error "ventana sin tamaño"; exit 3 }
$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
# Se pide a la ventana que se pinte a sí misma (con su contenido de vídeo):
# así no salen otras ventanas ni avisos que tenga encima. Si no puede, se
# copia esa zona de la pantalla.
$hdc = $g.GetHdc()
$ok = [Ps2Win]::PrintWindow($h, $hdc, 2)
$g.ReleaseHdc($hdc)
if (-not $ok) { $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size) }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
"#;

/// Quita el prefijo `[  12,3456] ` que PCSX2 pone a cada línea del log.
fn strip_timestamp(line: &str) -> &str {
    if line.starts_with('[') {
        if let Some(end) = line.find("] ") {
            if end <= 14 {
                return &line[end + 2..];
            }
        }
        if let Some(end) = line.find(']') {
            if end <= 14 && end + 1 == line.len() {
                return "";
            }
        }
    }
    line
}

/// Mensajes del propio PCSX2 y de los módulos de la BIOS en el IOP (FILEIO,
/// LOADFILE) que se intercalan con la consola del EE. Con `-v` se muestran.
fn is_emulator_noise(line: &str) -> bool {
    const PREFIXES: [&str; 33] = [
        "Frame rate:",
        "Setting vsync mode:",
        "ELF changed, active CRC",
        "Applying core settings",
        "Updating CPU configuration",
        "iR3000A Recompiler reset",
        "mVU Reset",
        "EE/iR5900 Recompiler Reset",
        "UpdateVSyncRate:",
        "Set GS CRTC configuration.",
        "DVE powered",
        "Opening GS",
        "Closing GS",
        "BIOS: Successfully found the instruction pattern",
        "IOPHLE ",
        "[Unsafe Settings]",
        "OSD [",
        "read/write allocate memory",
        "open name ",
        "open fd = ",
        "dopen name ",
        "dopen fd = ",
        "mkdir name ",
        "rmdir name ",
        "remove file ",
        "Unknown device '",
        "Known devices are ",
        "loadmodule: ",
        "RegisterLibraryEntries:",
        "getstat name ",
        "cdvdLoadElf()",
        "DEV9: ",
        "microVU",
    ];
    PREFIXES.iter().any(|p| line.starts_with(p))
}

pub fn run(elf: &Path, o: &Opts) -> Result<i32, String> {
    let emulator = o
        .emulator
        .clone()
        .or_else(|| env::var("PCSX2_EXE").ok())
        .ok_or("no sé dónde está PCSX2: usa --emulator <pcsx2-qt.exe> o define PCSX2_EXE")?;
    if !Path::new(&emulator).is_file() {
        return Err(format!("no existe el emulador `{emulator}`"));
    }
    let log = elf.with_extension("emulog.txt");
    let _ = fs::remove_file(&log);

    let mut cmd = Command::new(&emulator);
    cmd.args(["-batch", "-fastboot", "-nofullscreen", "-logfile"]).arg(&log).arg("--").arg(elf);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if o.verbose {
        eprintln!("+ {cmd:?}");
    }
    eprintln!("  Running `{}` en PCSX2", elf.display());
    let mut child = cmd.spawn().map_err(|e| format!("no pude lanzar PCSX2: {e}"))?;

    let start = Instant::now();
    let mut pos = 0u64;
    let mut pending = Vec::new();
    let mut started = false; // ¿ya arrancó nuestro ELF?
    let mut captured = String::new();
    let mut result: Option<i32> = None;
    let elf_name = elf.file_name().unwrap().to_string_lossy().into_owned();

    'outer: loop {
        if let Ok(mut f) = fs::File::open(&log) {
            if f.seek(SeekFrom::Start(pos)).is_ok() {
                let mut chunk = Vec::new();
                if let Ok(n) = f.read_to_end(&mut chunk) {
                    pos += n as u64;
                    pending.extend_from_slice(&chunk);
                }
            }
        }
        while let Some(nl) = pending.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = pending.drain(..=nl).collect();
            let text = String::from_utf8_lossy(&raw);
            let line = strip_timestamp(text.trim_end_matches(['\r', '\n']));
            if !started {
                if o.verbose {
                    eprintln!("[pcsx2] {line}");
                }
                // PCSX2 anuncia el ELF justo antes de saltar a su punto de entrada.
                if line.contains(&elf_name) && line.contains("is executing") {
                    started = true;
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix(EXIT_MARKER) {
                result = Some(rest.trim().parse().unwrap_or(1));
                break 'outer;
            }
            if is_emulator_noise(line) {
                if o.verbose {
                    eprintln!("[pcsx2] {line}");
                }
                continue;
            }
            if o.keys {
                if let Some(order) = line.strip_prefix("[tecla] ") {
                    keys::order(child.id(), order);
                    continue;
                }
            }
            println!("{line}");
            captured.push_str(line);
            captured.push('\n');
            if let Some(u) = &o.until {
                if line.contains(u.as_str()) {
                    result = Some(0);
                    break 'outer;
                }
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            if !started {
                return Err(format!("PCSX2 terminó ({status}) antes de arrancar el ELF; mira {}", log.display()));
            }
            eprintln!("  PCSX2 se cerró ({status})");
            result = Some(result.unwrap_or(1));
            break;
        }
        if start.elapsed() > Duration::from_secs(o.timeout) {
            let _ = child.kill();
            let _ = child.wait();
            save_log(o, &captured)?;
            return Err(format!(
                "tiempo agotado tras {} s{}",
                o.timeout,
                if started { "" } else { " (el ELF nunca llegó a arrancar)" }
            ));
        }
        sleep(Duration::from_millis(50));
    }

    save_log(o, &captured)?;
    let mut shot_err = None;
    if let Some(png) = &o.screenshot {
        // Deja que el emulador presente el último frame antes de capturar.
        sleep(Duration::from_millis(1500));
        if let Err(e) = screenshot(child.id(), png, elf) {
            shot_err = Some(e);
        }
    }
    if !o.keep_open {
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Some(e) = shot_err {
        return Err(e);
    }
    Ok(result.unwrap_or(1))
}

fn save_log(o: &Opts, captured: &str) -> Result<(), String> {
    if let Some(path) = &o.log {
        fs::write(path, captured).map_err(|e| format!("no pude escribir {path}: {e}"))?;
    }
    Ok(())
}

fn screenshot(pid: u32, png: &str, elf: &Path) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("--screenshot sólo está implementado en Windows".into());
    }
    let script = elf.with_extension("screenshot.ps1");
    fs::write(&script, SCREENSHOT_PS1).map_err(|e| format!("no pude escribir {}: {e}", script.display()))?;
    let out = std::path::absolute(png).map_err(|e| e.to_string())?;
    if let Some(dir) = out.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let status = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg(pid.to_string())
        .arg(&out)
        .status()
        .map_err(|e| format!("no pude ejecutar powershell: {e}"))?;
    let _ = fs::remove_file(&script);
    if !status.success() {
        return Err("la captura de pantalla falló".into());
    }
    eprintln!("  Captura: {}", out.display());
    Ok(())
}

/// Teclas simuladas para las pruebas del mando (`--keys`). Se envían como
/// mensajes a las ventanas del emulador, sin tocar el foco ni el teclado
/// real: lo que el usuario esté escribiendo en otra ventana no se ve afectado.
#[cfg(windows)]
mod keys {
    use std::ffi::c_void;

    type Hwnd = *mut c_void;
    type EnumProc = unsafe extern "system" fn(Hwnd, isize) -> i32;

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(callback: EnumProc, param: isize) -> i32;
        fn EnumChildWindows(parent: Hwnd, callback: EnumProc, param: isize) -> i32;
        fn GetWindowThreadProcessId(window: Hwnd, process: *mut u32) -> u32;
        fn IsWindowVisible(window: Hwnd) -> i32;
        fn PostMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
        fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
    }

    struct Search {
        pid: u32,
        windows: Vec<Hwnd>,
    }

    unsafe extern "system" fn collect_top(window: Hwnd, param: isize) -> i32 {
        let search = &mut *(param as *mut Search);
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        if pid == search.pid && IsWindowVisible(window) != 0 {
            search.windows.push(window);
            EnumChildWindows(window, collect_child, param);
        }
        1
    }

    unsafe extern "system" fn collect_child(window: Hwnd, param: isize) -> i32 {
        (*(param as *mut Search)).windows.push(window);
        1
    }

    /// Código de tecla virtual y si es una tecla "extendida".
    fn virtual_key(name: &str) -> Option<(u32, bool)> {
        Some(match name {
            "Up" => (0x26, true),
            "Down" => (0x28, true),
            "Left" => (0x25, true),
            "Right" => (0x27, true),
            "Return" => (0x0D, false),
            "Backspace" => (0x08, false),
            "Space" => (0x20, false),
            _ if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() => (name.as_bytes()[0].to_ascii_uppercase() as u32, false),
            _ => return None,
        })
    }

    /// Ejecuta una orden `pulsa X` o `suelta X` sobre el emulador `pid`.
    pub fn order(pid: u32, order: &str) {
        let mut words = order.split_whitespace();
        let (action, name) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        let Some((key, extended)) = virtual_key(name) else {
            eprintln!("  --keys: tecla desconocida en `{order}`");
            return;
        };
        let down = match action {
            "pulsa" => true,
            "suelta" => false,
            _ => return,
        };
        unsafe {
            let mut search = Search { pid, windows: Vec::new() };
            EnumWindows(collect_top, &mut search as *mut Search as isize);
            let scan = MapVirtualKeyW(key, 0) as isize;
            let lparam = 1 | scan << 16 | (extended as isize) << 24 | if down { 0 } else { 0xC000_0000 };
            // WM_KEYDOWN / WM_KEYUP a la ventana y a sus hijas (la superficie
            // de vídeo es una de ellas).
            for window in search.windows {
                PostMessageW(window, if down { 0x0100 } else { 0x0101 }, key as usize, lparam);
            }
        }
    }
}

#[cfg(not(windows))]
mod keys {
    pub fn order(_pid: u32, _order: &str) {
        eprintln!("  --keys sólo está implementado en Windows");
    }
}

# PS2Rust — Rust para PlayStation 2

Compila programas en Rust a ejecutables ELF para el Emotion Engine (MIPS R5900)
de la PlayStation 2. No usa C ni PS2SDK: el arranque, las llamadas al kernel, el
heap y los gráficos están escritos en Rust.

| [`cube`](examples/cube/src/main.rs) | [`hello`](examples/hello/src/main.rs) |
|---|---|
| ![Cubo con sombreado Gouraud en PCSX2](docs/cube.png) | ![Texto en pantalla en PCSX2](docs/hello.png) |

```rust
#![no_std]
#![no_main]
use ps2::println;

ps2::entry!(main);

fn main() {
    println!("Hola desde Rust en una PlayStation 2");
}
```

```
cargo ps2 run -p hello
```

## Cómo funciona

No es un fork de `rustc`. Es el `rustc` que ya tienes instalado, dirigido a un
target nuevo, más un paso propio que adapta el código máquina al R5900:

1. **Target `mipsel-sony-ps2`** ([spec](toolchain/cargo-ps2/assets/mipsel-sony-ps2.json)):
   MIPS II little-endian, ABI o32, FPU de precisión simple (`f64` por software),
   sin PIC, y con el relleno de delay slots de LLVM desactivado para esquivar la
   errata de bucles cortos del R5900.
2. **`core` y `alloc` se recompilan** para ese target con `-Zbuild-std`.
3. **Enlazado** con `rust-lld` y un [linker script](toolchain/cargo-ps2/assets/ps2.ld)
   que carga el programa en `0x00100000`.
4. **Legalización para el R5900** ([r5900.rs](toolchain/cargo-ps2/src/r5900.rs)).
   LLVM no conoce el R5900, así que el ELF enlazado se parchea in situ:

   | LLVM emite | El R5900 | Se reescribe como |
   |---|---|---|
   | `ll` / `sc` + `beqz` (atómicos) | no los tiene | `lw` / `sw` + `li 1` |
   | `c.ult.s`, `c.ule.s`, `c.un.s`… | sólo C.F, C.EQ, C.LT, C.LE | la variante ordenada (su FPU no tiene NaN) |
   | `trunc.w.s` | no existe | `cvt.w.s` (en el R5900 siempre trunca) |
   | `sqrt.s fd, fs` | el operando va en `ft` | mismo opcode, campo movido |

5. **Verificación**: se decodifica cada instrucción del ELF final. Lo que el
   R5900 no implementa (`dmult`, `ddiv`, `movf`, dobles en FPU, `ldc1`…) o un
   bucle corto con el delay slot ocupado hacen fallar la compilación.

Todo eso lo orquesta `cargo ps2`, un binario sin dependencias en
[toolchain/cargo-ps2](toolchain/cargo-ps2).

## Requisitos

- Rust estable con el componente `rust-src` (`rustup component add rust-src`).
  Probado con rustc 1.97.1. `cargo ps2` activa `RUSTC_BOOTSTRAP=1` porque el
  target JSON, `build-std` y el ensamblador en línea para MIPS son inestables.
- Para `cargo ps2 run`: PCSX2 con una BIOS (la BIOS no se incluye: vuélcala de
  tu consola). La ruta del emulador se toma de `--emulator` o de la variable de
  entorno `PCSX2_EXE`; también se puede fijar en [.cargo/config.toml](.cargo/config.toml).

## Uso

Desde la raíz de este repositorio no hay que instalar nada (hay un alias en
`.cargo/config.toml`):

```
cargo ps2 build -p cube            # compila -> target/ps2/cube.elf
cargo ps2 run -p hello             # compila y ejecuta en PCSX2, mostrando la consola
cargo ps2 run -p cube --until "240 frames" --screenshot cube.png
cargo ps2 check juego.elf          # ¿este ELF sólo usa instrucciones del R5900?
cargo ps2 help
```

Para usarlo en un proyecto propio, fuera de este repositorio:

```
cargo install --git https://github.com/mostazaniikkkk/PS2Rust cargo-ps2
```

```toml
# Cargo.toml de tu proyecto
[dependencies]
ps2 = { git = "https://github.com/mostazaniikkkk/PS2Rust" }

[profile.release]
panic = "abort"
```

El `.elf` resultante se puede cargar en PCSX2 (Sistema → Ejecutar ELF) o en una
consola con un lanzador de homebrew.

## El runtime (`crates/ps2`)

| Módulo | Qué ofrece |
|---|---|
| `ps2::entry!` | Punto de entrada; `main` devuelve `()` o un `i32` |
| `print!` / `println!` | Consola del EE (puerto serie; PCSX2 la muestra en su log) |
| `heap` | Asignador global: `Vec`, `String`, `Box`, `Rc`, `Arc`, `BTreeMap`… sobre los ~30 MiB libres |
| `gs` | Graphics Synthesizer: NTSC/PAL, doble búfer, vsync, rectángulos, líneas, triángulos Gouraud, mezcla alfa, texto |
| `math` | `sqrt`, `sin`, `cos`, `floor` en `f32` sobre la FPU |
| `time` | Contador de ciclos de la CPU, microsegundos |
| `kernel` | Syscalls del kernel del EE y registros de hardware de 32 y 64 bits |

Un pánico imprime el mensaje por la consola, pone el fondo en rojo y termina
con código 101.

## Ejemplos

- [hello](examples/hello/src/main.rs): consola, heap, flotantes y texto en pantalla.
- [cube](examples/cube/src/main.rs): cubo con sombreado Gouraud a 60 fps, toda la geometría en `f32`.
- [selftest](examples/selftest/src/suite.rs): batería de 33 comprobaciones y 144 líneas de salida.
- [panic](examples/panic/src/main.rs): acceso fuera de rango detectado en la consola.

## Verificación

```
powershell -ExecutionPolicy Bypass -File verify.ps1 -Emulator C:uta\pcsx2-qt.exe
```

El script compila todo, lo ejecuta en PCSX2 y comprueba los resultados. La
prueba principal es diferencial: [suite.rs](examples/selftest/src/suite.rs) se
compila tal cual para el PC y para la PS2, y las dos salidas deben coincidir
byte a byte. Cubre enteros de 32/64/128 bits, `f64` por software, `f32` en la
FPU, formato, colecciones, `Rc`/`Arc`, atómicos, estrés del heap, cadenas
UTF-8, genéricos, closures, iteradores y varios algoritmos.

## Límites conocidos

- **Probado sólo en emuladores** (PCSX2 2.7.513 con BIOS real y Play! 0.47), no
  en una consola física.
- **ABI o32 de 32 bits.** No se puede enlazar con las bibliotecas C de PS2SDK
  (que usan n32). `u64` e `i64` funcionan pero se calculan con pares de
  registros de 32 bits; no hay acceso a los registros de 128 bits (MMI) ni a
  las unidades vectoriales.
- **Atómicos de un solo núcleo.** `lw`/`sw` no es atómico frente a una
  interrupción que modifique el mismo dato. El runtime no instala manejadores
  de interrupción ni crea hilos, así que dentro de él es correcto; si añades
  interrupciones, no compartas con ellas atómicos de lectura-modificación-escritura.
- **`f32` sigue la FPU de la PS2**, no IEEE 754: sin NaN, infinitos ni
  denormales, y redondeo hacia cero. `f64` sí es IEEE 754 (por software, lento).
- **Sin `std`**: sólo `core` y `alloc`. No hay mando, sonido, tarjeta de memoria
  ni archivos; todo eso vive en el procesador de E/S (IOP) y necesita SIF RPC.
- PCSX2 decodifica la consola del EE como Shift-JIS: el texto no ASCII, `\` y
  `~` se pierden en su log (no en pantalla; `gs::text` dibuja `ñ`, `¡` y `¿`).

## Licencia

[CC0 1.0](LICENSE): dominio público. Haz lo que quieras con esto, sin pedir
permiso ni dar crédito.

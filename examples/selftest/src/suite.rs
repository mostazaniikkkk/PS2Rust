//! Batería de pruebas del lenguaje, `core` y `alloc`.
//!
//! Este mismo archivo se compila para PlayStation 2 y para el PC anfitrión.
//! Las dos ejecuciones deben imprimir exactamente lo mismo: así se comprueba
//! el compilador contra una implementación de referencia, línea por línea.
//!
//! Los datos de entrada pasan por `black_box` para que el cálculo ocurra en la
//! consola y no en tiempo de compilación.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BinaryHeap, VecDeque};
use alloc::format;
use alloc::rc::{Rc, Weak};
use alloc::string::{String, ToString};
#[cfg(target_has_atomic = "ptr")]
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt::{self, Write};
use core::hint::black_box;
use core::ops::{Add, Mul};
#[cfg(target_has_atomic = "ptr")]
use core::sync::atomic::{AtomicBool, AtomicI16, AtomicPtr, AtomicU32, AtomicU8, AtomicUsize, Ordering};

/// Lo que depende de la plataforma.
pub struct Env {
    /// Raíz cuadrada `f32` de la plataforma (FPU en el EE, por software en el
    /// IOP, `f32::sqrt` en el PC).
    pub sqrt: fn(f32) -> f32,
    /// Título de la sección de `f32`.
    pub f32_title: &'static str,
    /// ¿Hay atómicos y `Arc`? El R3000A del IOP no tiene con qué hacerlos.
    #[allow(dead_code)]
    pub atomics: bool,
    /// Palabras de 32 bits del bloque grande de la prueba del heap.
    pub big_block_words: usize,
}

impl Env {
    /// Emotion Engine: FPU, atómicos y 32 MiB de RAM.
    #[allow(dead_code)]
    pub fn ee(sqrt: fn(f32) -> f32) -> Env {
        Env { sqrt, f32_title: "f32 (FPU del Emotion Engine)", atomics: true, big_block_words: 1 << 20 }
    }

    /// IOP: sin FPU, sin atómicos y 2 MiB de RAM para todo.
    #[allow(dead_code)]
    pub fn iop(sqrt: fn(f32) -> f32) -> Env {
        Env { sqrt, f32_title: "f32 (por software: el IOP no tiene FPU)", atomics: false, big_block_words: 1 << 15 }
    }
}

/// Escritor que codifica como `%XX` todo byte que no sea ASCII "seguro".
///
/// La consola del EE en PCSX2 se interpreta como Shift-JIS, así que los bytes
/// UTF-8 no ASCII, `\` y `~` no llegan intactos al log. Con este envoltorio el
/// transporte es sin pérdida y la comparación con el PC es byte a byte.
pub struct Escaped<W: Write>(pub W);

impl<W: Write> Write for Escaped<W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            let safe = b == b'\n' || ((0x20..=0x7e).contains(&b) && !matches!(b, b'%' | b'\\' | b'~'));
            if safe {
                self.0.write_char(b as char)?;
            } else {
                write!(self.0, "%{b:02X}")?;
            }
        }
        Ok(())
    }
}

struct T<'a> {
    out: &'a mut dyn Write,
    total: u32,
    failed: u32,
}

impl T<'_> {
    fn section(&mut self, name: &str) {
        let _ = writeln!(self.out, "== {name}");
    }

    fn check(&mut self, name: &str, ok: bool) {
        self.total += 1;
        if !ok {
            self.failed += 1;
        }
        let _ = writeln!(self.out, "[{}] {}", if ok { "ok" } else { "FALLO" }, name);
    }

    fn show(&mut self, name: &str, value: fmt::Arguments) {
        let _ = writeln!(self.out, "   {name} = {value}");
    }
}

macro_rules! show {
    ($t:expr, $name:expr, $($fmt:tt)*) => { $t.show($name, format_args!($($fmt)*)) };
}

/// Ejecuta toda la batería y devuelve el número de comprobaciones fallidas.
pub fn run(out: &mut dyn Write, env: &Env) -> u32 {
    let mut t = T { out, total: 0, failed: 0 };
    integers(&mut t);
    wide_integers(&mut t);
    soft_f64(&mut t);
    hard_f32(&mut t, env);
    formatting(&mut t);
    collections(&mut t);
    smart_pointers(&mut t);
    atomics(&mut t, env);
    heap_stress(&mut t, env);
    strings(&mut t);
    language(&mut t);
    algorithms(&mut t);
    let (total, failed) = (t.total, t.failed);
    let _ = writeln!(t.out, "== resultado: {total} comprobaciones, {failed} fallos");
    failed
}

// ---------------------------------------------------------------- enteros

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u32) -> u32 {
        ((self.next() >> 33) % n as u64) as u32
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn integers(t: &mut T) {
    t.section("enteros de 32 bits");
    let a = black_box(0xDEAD_BEEFu32);
    let b = black_box(-123_456_789i32);
    show!(t, "rotate_left", "{:#010x}", a.rotate_left(13));
    show!(t, "swap_bytes", "{:#010x}", a.swap_bytes());
    show!(t, "bits", "{} {} {}", a.count_ones(), a.leading_zeros(), a.trailing_zeros());
    show!(t, "mul/div/rem", "{} {} {}", b.wrapping_mul(7919), b / 1000, b % 1000);
    show!(t, "rem_euclid", "{} {}", b.rem_euclid(1000), b.div_euclid(1000));
    show!(t, "shifts", "{} {} {}", b >> 5, (b as u32) >> 5, b << 3);
    show!(t, "overflowing_add", "{:?}", i32::MAX.overflowing_add(black_box(1)));
    show!(t, "saturating", "{} {}", i32::MIN.saturating_sub(black_box(1)), 200u8.saturating_add(black_box(100)));
    show!(t, "pow/isqrt", "{} {}", black_box(3u32).pow(19), black_box(1_000_000_007u32).isqrt());
    t.check("checked_div por cero", black_box(7i32).checked_div(black_box(0)).is_none());
    t.check("checked_div MIN/-1", i32::MIN.checked_div(black_box(-1)).is_none());
    t.check("checked_mul desborda", black_box(65_536u32).checked_mul(black_box(65_536)).is_none());
    t.check("u8 wrapping", black_box(250u8).wrapping_add(10) == 4);
    t.check("i16 abs_diff", black_box(-300i16).abs_diff(black_box(200)) == 500);
}

fn wide_integers(t: &mut T) {
    t.section("enteros de 64 y 128 bits");
    let mut rng = XorShift(black_box(0x9E37_79B9_7F4A_7C15));
    let mut acc = 0u64;
    for _ in 0..1000 {
        acc = acc.rotate_left(5) ^ rng.next();
    }
    show!(t, "xorshift64* x1000", "{acc:#018x}");

    let a = black_box(0x0123_4567_89AB_CDEFu64);
    let b = black_box(0xF_EDCB_A987u64);
    show!(t, "u64 mul", "{:#018x}", a.wrapping_mul(b));
    show!(t, "u64 div/rem", "{} {}", a / b, a % b);
    let c = black_box(-9_876_543_210_123i64);
    show!(t, "i64 div/rem", "{} {}", c / 1_000_007, c % 1_000_007);
    show!(t, "i64 shifts", "{} {} {}", c >> black_box(17), c << black_box(9), (c as u64) >> black_box(40));
    show!(t, "u64 bits", "{} {}", a.leading_zeros(), a.count_ones());

    let mut fib = (0u64, 1u64);
    for _ in 0..black_box(90) {
        fib = (fib.1, fib.0 + fib.1);
    }
    t.check("fib(90)", fib.0 == 2_880_067_194_370_816_120);
    t.check("gcd(1071, 462) = 21", gcd(black_box(1071), black_box(462)) == 21);

    let mut fact = 1u128;
    for i in 1..=black_box(30u128) {
        fact *= i;
    }
    show!(t, "30!", "{fact}");
    t.check("30! exacto", fact == 265_252_859_812_191_058_636_308_480_000_000);
    show!(t, "u128 div/rem", "{} {}", fact / 1_000_000_007, fact % 1_000_000_007);
    let big = black_box(-170_141_183_460_469_231_731_687_303_715_884_105_727i128);
    show!(t, "i128", "{} {:#x}", big / 3, (big as u128) >> 64);
}

// ---------------------------------------------------------------- flotantes

fn soft_f64(t: &mut T) {
    t.section("f64 (IEEE 754 por software)");
    let mut harmonic = 0.0f64;
    for k in 1..=black_box(1000) {
        harmonic += 1.0 / k as f64;
    }
    show!(t, "H(1000)", "{harmonic}");
    // Raíz de 2 por Newton.
    let mut x = black_box(1.0f64);
    for _ in 0..6 {
        x = 0.5 * (x + 2.0 / x);
    }
    show!(t, "sqrt(2) Newton", "{x}");
    t.check("sqrt(2)^2 ~ 2", (x * x - 2.0).abs() < 1e-15);
    let pi: f64 = black_box("3.141592653589793").parse().unwrap();
    show!(t, "parse pi", "{} {:.5} {:e}", pi * 2.0, pi, pi * 1e10);
    show!(t, "conversiones", "{} {} {}", (pi * 1e9) as i64, black_box(-7.99f64) as i32, black_box(1u64 << 53) as f64);
    show!(t, "f64 -> f32 -> f64", "{}", (pi as f32) as f64);
    let zero = black_box(0.0f64);
    t.check("0/0 es NaN", (zero / zero).is_nan());
    t.check("1/0 es infinito", (1.0 / zero).is_infinite());
    t.check("NaN != NaN", zero / zero != zero / zero);
    show!(t, "especiales", "{} {} {}", 1.0 / zero, -1.0 / zero, f64::MAX);
    show!(t, "0.1 + 0.2", "{}", black_box(0.1f64) + black_box(0.2f64));
    show!(t, "trunc/fract", "{} {}", black_box(-2.75f64) as i64, black_box(1e300f64) * 1e10);
}

fn hard_f32(t: &mut T, env: &Env) {
    t.section(env.f32_title);
    let a = black_box(1.5f32);
    let b = black_box(2.25f32);
    show!(t, "aritmetica", "{} {} {} {}", a * b + 0.125, a - b, b / a, -a * a);
    show!(t, "division", "{} {}", black_box(10.0f32) / black_box(4.0), black_box(1.0f32) / black_box(1024.0));
    show!(t, "f32 -> i32", "{} {} {}", black_box(7.75f32) as i32, black_box(-7.75f32) as i32, black_box(3.0e9f32) as i32);
    show!(t, "f32 -> u32/u8", "{} {} {}", black_box(3.0e9f32) as u32, black_box(-5.0f32) as u32, black_box(300.0f32) as u8);
    show!(t, "i32 -> f32", "{} {} {}", black_box(-12_345i32) as f32, black_box(4_000_000_000u32) as f32, black_box(255u8) as f32);
    show!(t, "sqrt", "{} {} {}", (env.sqrt)(black_box(144.0)), (env.sqrt)(black_box(2.25)), (env.sqrt)(black_box(0.0)));
    show!(t, "abs/min/max", "{} {} {} {}", black_box(-2.5f32).abs(), a.min(b), a.max(b), black_box(9.0f32).clamp(a, b));

    // Todas las comparaciones entre todos los pares: una máscara por par.
    let values = [black_box(-2.5f32), black_box(0.0), black_box(1.0), black_box(1.0), black_box(3.5)];
    let mut table = String::new();
    for &x in &values {
        for &y in &values {
            let mask = (x < y) as u8
                | ((x <= y) as u8) << 1
                | ((x > y) as u8) << 2
                | ((x >= y) as u8) << 3
                | ((x == y) as u8) << 4
                | ((x != y) as u8) << 5;
            let _ = write!(table, "{mask:02x} ");
        }
    }
    show!(t, "comparaciones", "{}", table.trim_end());

    let mut halves = 0.0f32;
    let mut term = black_box(1.0f32);
    for _ in 0..20 {
        term *= 0.5;
        halves += term;
    }
    show!(t, "suma de 1/2^k", "{halves}");
    let mut harmonic = 0.0f32;
    for k in 1..=black_box(100) {
        harmonic += 1.0 / k as f32;
    }
    show!(t, "H(100) a 3 decimales", "{harmonic:.3}");
    t.check("f32 -> f64 exacto", black_box(0.15625f32) as f64 == 0.15625f64);
    // Producto escalar y norma: lo típico de un juego.
    let v = [black_box(3.0f32), black_box(4.0), black_box(12.0)];
    let norm = (env.sqrt)(v.iter().map(|c| c * c).sum());
    t.check("norma (3,4,12) = 13", norm == 13.0);
}

// ---------------------------------------------------------------- formato

#[derive(Debug, Clone, PartialEq, PartialOrd, Default)]
struct Player {
    name: String,
    level: u8,
    position: (i16, i16),
    items: Vec<Item>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Item {
    Potion(u8),
    Key { door: u16 },
    Map,
}

fn formatting(t: &mut T) {
    t.section("formato");
    let n = black_box(48_879u32);
    show!(t, "anchos", "[{:>8}] [{:<8}] [{:^8}] [{:+}]", n, n, n, n);
    show!(t, "bases", "{:#x} {:#X} {:#o} {:#b} {:08x}", n, n, n, black_box(37u8), n);
    show!(t, "flotantes", "{:8.3} {:<8.1} {:+.2e} {:.0}", black_box(3.14159f64), black_box(2.5f64), black_box(123456.789f64), black_box(0.5f64));
    let p = Player {
        name: "Ratchet".to_string(),
        level: black_box(42),
        position: (-120, 77),
        items: vec![Item::Potion(3), Item::Key { door: 512 }, Item::Map],
    };
    show!(t, "debug", "{:?}", p);
    show!(t, "debug bonito", "{:#?}", p.items);
    show!(t, "opciones", "{:?} {:?} {:?}", Some("x"), None::<u8>, Err::<(), _>(Item::Map));
    show!(t, "caracteres", "{:?} {:?} {}", 'ñ', "tab\tcomillas\"", '\u{1F3AE}'.len_utf8());
    show!(t, "relleno", "{:*^21}", " PS2 ");
    let s = format!("{0}-{1}-{0} {name}", black_box(1), black_box(2), name = "fin");
    t.check("argumentos posicionales", s == "1-2-1 fin");
}

// ---------------------------------------------------------------- colecciones

fn collections(t: &mut T) {
    t.section("colecciones");
    let mut v: Vec<u32> = (1..=black_box(1000u32)).map(|i| i * i % 1009).collect();
    show!(t, "vec suma", "{}", v.iter().map(|&x| x as u64).sum::<u64>());
    v.sort_unstable();
    v.dedup();
    show!(t, "ordenar + dedup", "{} elementos, [{}, {}, {} .. {}]", v.len(), v[0], v[1], v[2], v[v.len() - 1]);
    t.check("binary_search", v.binary_search(&v[137]) == Ok(137));
    v.retain(|x| x % 7 == 0);
    v.reverse();
    show!(t, "retain + reverse", "{:?}", &v[..6]);
    let mut grid = vec![vec![0u8; 8]; 8];
    for (y, row) in grid.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            *cell = ((x * y) % 10) as u8;
        }
    }
    show!(t, "vec de vecs", "{:?}", grid[7]);

    let text = black_box("la consola corre rust y rust corre en la consola de sony");
    let mut words: BTreeMap<&str, u32> = BTreeMap::new();
    for w in text.split(' ') {
        *words.entry(w).or_insert(0) += 1;
    }
    show!(t, "btreemap", "{:?}", words);
    show!(t, "rango", "{:?}", words.range("d".."m").map(|(k, _)| *k).collect::<Vec<_>>());

    let mut dq: VecDeque<i32> = (1..=black_box(8)).collect();
    dq.rotate_left(3);
    dq.push_front(-1);
    let back = dq.pop_back();
    show!(t, "vecdeque", "{:?} {:?}", dq, back);

    let mut heap = BinaryHeap::new();
    for x in [5u32, 1, 8, 3, 9, 2, 7] {
        heap.push(black_box(x) * 11 % 17);
    }
    let mut order = Vec::new();
    while let Some(x) = heap.pop() {
        order.push(x);
    }
    show!(t, "binaryheap", "{:?}", order);

    let mut items = vec![Item::Map, Item::Key { door: 9 }, Item::Potion(200), Item::Potion(3), Item::Key { door: 2 }];
    items.sort();
    show!(t, "ordenar enums", "{:?}", items);
}

trait Shape {
    fn area(&self) -> u32;
    fn name(&self) -> &'static str;
}

struct Rect(u32, u32);
struct Square(u32);

impl Shape for Rect {
    fn area(&self) -> u32 {
        self.0 * self.1
    }
    fn name(&self) -> &'static str {
        "rect"
    }
}

impl Shape for Square {
    fn area(&self) -> u32 {
        self.0 * self.0
    }
    fn name(&self) -> &'static str {
        "cuadrado"
    }
}

struct Noisy<'a>(&'a RefCell<Vec<u32>>, u32);

impl Drop for Noisy<'_> {
    fn drop(&mut self) {
        self.0.borrow_mut().push(self.1);
    }
}

fn smart_pointers(t: &mut T) {
    t.section("punteros inteligentes");
    let shapes: Vec<Box<dyn Shape>> = vec![Box::new(Rect(black_box(3), 7)), Box::new(Square(black_box(5))), Box::new(Rect(2, 2))];
    let total: u32 = shapes.iter().map(|s| s.area()).sum();
    let names: Vec<&str> = shapes.iter().map(|s| s.name()).collect();
    show!(t, "dyn Trait", "{total} {names:?}");

    let shared = Rc::new(RefCell::new(Vec::new()));
    let other = Rc::clone(&shared);
    other.borrow_mut().push(black_box(10));
    shared.borrow_mut().push(20);
    let weak: Weak<RefCell<Vec<i32>>> = Rc::downgrade(&shared);
    show!(t, "rc", "{:?} fuertes={} debiles={}", shared.borrow(), Rc::strong_count(&shared), Rc::weak_count(&shared));
    drop(shared);
    drop(other);
    t.check("weak tras soltar", weak.upgrade().is_none());

    let log = RefCell::new(Vec::new());
    {
        let _a = Noisy(&log, 1);
        let b = Noisy(&log, 2);
        let _c = Noisy(&log, 3);
        drop(b);
    }
    show!(t, "orden de drop", "{:?}", log.borrow());

    let callbacks: Vec<Box<dyn Fn(i32) -> i32>> = vec![Box::new(|x| x + 1), Box::new(|x| x * x), Box::new(move |x| x - black_box(100))];
    show!(t, "box dyn Fn", "{}", callbacks.iter().fold(black_box(6), |acc, f| f(acc)));
}

#[cfg(not(target_has_atomic = "ptr"))]
fn atomics(_t: &mut T, _env: &Env) {}

#[cfg(target_has_atomic = "ptr")]
fn atomics(t: &mut T, env: &Env) {
    if !env.atomics {
        return;
    }
    t.section("atomicos y Arc");
    let counter = Arc::new(AtomicU32::new(0));
    let clones: Vec<Arc<AtomicU32>> = (0..black_box(5)).map(|_| Arc::clone(&counter)).collect();
    for (i, c) in clones.iter().enumerate() {
        c.fetch_add(i as u32 + 1, Ordering::SeqCst);
    }
    show!(t, "arc", "valor={} fuertes={}", counter.load(Ordering::SeqCst), Arc::strong_count(&counter));
    drop(clones);
    t.check("arc::try_unwrap", Arc::try_unwrap(counter).ok().map(|a| a.into_inner()) == Some(15));

    let a = AtomicU32::new(black_box(5));
    let mut spins = 0;
    // Bucle CAS clásico: duplicar hasta pasar de 1000.
    loop {
        let cur = a.load(Ordering::Relaxed);
        if cur > 1000 {
            break;
        }
        if a.compare_exchange(cur, cur * 2, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
            spins += 1;
        }
    }
    show!(t, "cas", "{} en {} pasos", a.load(Ordering::SeqCst), spins);
    t.check("cas fallido devuelve el actual", a.compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst) == Err(1280));
    show!(t, "u32 fetch_*", "{} {} {} {}", a.fetch_add(20, Ordering::SeqCst), a.fetch_sub(300, Ordering::SeqCst), a.fetch_and(0xff0, Ordering::SeqCst), a.fetch_xor(0xffff, Ordering::SeqCst));
    show!(t, "u32 swap/max", "{} {} {}", a.swap(7, Ordering::SeqCst), a.fetch_max(99, Ordering::SeqCst), a.load(Ordering::SeqCst));

    // Atómicos más pequeños que una palabra: LLVM los implementa con máscaras
    // sobre la palabra de 32 bits que los contiene.
    let bytes = [AtomicU8::new(0x11), AtomicU8::new(black_box(250)), AtomicU8::new(0x33), AtomicU8::new(0x44)];
    let old = bytes[1].fetch_add(black_box(10), Ordering::SeqCst);
    show!(t, "u8 fetch_add", "{} -> {}", old, bytes[1].load(Ordering::SeqCst));
    t.check("vecinos intactos", bytes[0].load(Ordering::SeqCst) == 0x11 && bytes[2].load(Ordering::SeqCst) == 0x33 && bytes[3].load(Ordering::SeqCst) == 0x44);
    let small = AtomicI16::new(black_box(-32_000));
    show!(t, "i16 fetch_sub", "{} -> {}", small.fetch_sub(1000, Ordering::SeqCst), small.load(Ordering::SeqCst));
    let flag = AtomicBool::new(false);
    show!(t, "bool", "{} {} {}", flag.swap(true, Ordering::SeqCst), flag.fetch_nand(true, Ordering::SeqCst), flag.load(Ordering::SeqCst));
    let size = AtomicUsize::new(black_box(usize::MAX));
    show!(t, "usize wrap", "{} {}", size.fetch_add(2, Ordering::SeqCst) == usize::MAX, size.load(Ordering::SeqCst));
    let mut one = 1u32;
    let mut two = 2u32;
    let ptr = AtomicPtr::new(&mut one as *mut u32);
    let prev = ptr.swap(&mut two, Ordering::SeqCst);
    t.check("atomicptr swap", unsafe { *prev == 1 && *ptr.load(Ordering::SeqCst) == 2 });
}

fn heap_stress(t: &mut T, env: &Env) {
    t.section("heap");
    let mut rng = XorShift(black_box(0xC0FF_EE00_1234_5678));
    let mut live: Vec<Vec<u8>> = Vec::new();
    let mut checksum = 0u64;
    for round in 0..black_box(2000u32) {
        if live.len() < 64 || rng.below(3) != 0 {
            let len = 1 + rng.below(700) as usize;
            let fill = (round % 251) as u8;
            let mut block = vec![fill; len];
            block[len / 2] = block[len / 2].wrapping_add(1);
            live.push(block);
        }
        if live.len() > 96 || rng.below(4) == 0 {
            let victim = rng.below(live.len() as u32) as usize;
            let block = live.swap_remove(victim);
            checksum = checksum.wrapping_mul(31).wrapping_add(block.iter().map(|&b| b as u64).sum::<u64>());
        }
    }
    show!(t, "2000 rondas", "vivos={} checksum={:#018x}", live.len(), checksum);
    let intact = live.iter().all(|b| {
        let fill = b[0].min(b[b.len() - 1]);
        b.iter().filter(|&&x| x != fill).count() <= 1
    });
    t.check("bloques vivos intactos", intact);
    drop(live);

    // Un bloque grande: 4 MiB de u32 en el EE, 128 KiB en el IOP.
    let mut big = vec![0u32; black_box(env.big_block_words)];
    for (i, x) in big.iter_mut().enumerate() {
        *x = (i as u32).wrapping_mul(2_654_435_761);
    }
    show!(t, "bloque grande", "{} KiB {:#010x}", big.len() * 4 / 1024, big.iter().fold(0u32, |a, &x| a.rotate_left(1) ^ x));
    drop(big);

    #[repr(align(64))]
    struct Aligned([u8; 64]);
    let boxed = Box::new(Aligned([7; 64]));
    t.check("alineacion a 64 bytes", (&*boxed as *const Aligned as usize) % 64 == 0 && boxed.0[63] == 7);
    let mut grow: Vec<u16> = Vec::new();
    for i in 0..black_box(50_000u32) {
        grow.push(i as u16);
    }
    grow.shrink_to_fit();
    t.check("vec crece por realloc", grow.len() == 50_000 && grow[49_999] == 49_999);
}

// ---------------------------------------------------------------- texto

fn strings(t: &mut T) {
    t.section("cadenas");
    let s = black_box("El ñandú juega con la PlayStation 2 — 日本語 ok");
    show!(t, "longitudes", "{} bytes, {} chars", s.len(), s.chars().count());
    show!(t, "al reves", "{}", s.chars().rev().collect::<String>());
    show!(t, "mayusculas", "{}", s.to_uppercase());
    show!(t, "split", "{:?}", s.split_whitespace().filter(|w| w.len() > 4).collect::<Vec<_>>());
    show!(t, "find/replace", "{:?} {}", s.find("PlayStation"), s.replace("ñandú", "zorro").replace(' ', "_"));
    let numbers = black_box("12, -7, 0x1F, 40000, abc, 99");
    let parsed: Vec<Result<i32, String>> = numbers
        .split(", ")
        .map(|p| match p.strip_prefix("0x") {
            Some(hex) => i32::from_str_radix(hex, 16).map_err(|e| e.to_string()),
            None => p.parse::<i32>().map_err(|e| e.to_string()),
        })
        .collect();
    show!(t, "parse", "{:?}", parsed);
    let mut built = String::new();
    for (i, c) in ('a'..='j').enumerate() {
        if i % 3 == 0 {
            built.push(c.to_ascii_uppercase());
        } else {
            built.push(c);
        }
        built.insert(0, char::from_digit(i as u32, 10).unwrap());
    }
    show!(t, "construccion", "{built}");
    show!(t, "bytes", "{:02x?}", "ñ€".as_bytes());
    t.check("comparacion", "consola" < "consolas" && "Z" < "a" && "ñ" > "z");
    t.check("utf-8 invalido", core::str::from_utf8(black_box(&[0xff, 0xfe][..])).is_err());
}

// ---------------------------------------------------------------- lenguaje

#[derive(Debug, Clone, Copy, PartialEq)]
struct Vec2 {
    x: i32,
    y: i32,
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2 { x: self.x + o.x, y: self.y + o.y }
    }
}

impl Mul<i32> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: i32) -> Vec2 {
        Vec2 { x: self.x * k, y: self.y * k }
    }
}

struct Matrix<const R: usize, const C: usize>([[i32; C]; R]);

impl<const R: usize, const C: usize> Matrix<R, C> {
    fn transpose(&self) -> Matrix<C, R> {
        let mut out = [[0; R]; C];
        for r in 0..R {
            for c in 0..C {
                out[c][r] = self.0[r][c];
            }
        }
        Matrix(out)
    }
}

#[derive(Debug)]
enum Expr {
    Num(i64),
    Add(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
}

fn eval(e: &Expr) -> i64 {
    match e {
        Expr::Num(n) => *n,
        Expr::Add(a, b) => eval(a) + eval(b),
        Expr::Mul(a, b) => eval(a) * eval(b),
        Expr::Neg(a) => -eval(a),
    }
}

fn parse_pair(s: &str) -> Result<(u8, u8), String> {
    let (a, b) = s.split_once(':').ok_or_else(|| format!("falta ':' en {s:?}"))?;
    let a = a.trim().parse::<u8>().map_err(|e| format!("{a:?}: {e}"))?;
    let b = b.trim().parse::<u8>().map_err(|e| format!("{b:?}: {e}"))?;
    Ok((a, b))
}

fn largest<T: PartialOrd + Copy>(items: &[T]) -> Option<T> {
    let mut it = items.iter().copied();
    let first = it.next()?;
    Some(it.fold(first, |m, x| if x > m { x } else { m }))
}

fn ackermann(m: u32, n: u32) -> u32 {
    match (m, n) {
        (0, n) => n + 1,
        (m, 0) => ackermann(m - 1, 1),
        (m, n) => ackermann(m - 1, ackermann(m, n - 1)),
    }
}

fn classify(values: &[i32]) -> &'static str {
    match values {
        [] => "vacio",
        [x] if *x < 0 => "un negativo",
        [_] => "uno",
        [first, .., last] if first == last => "extremos iguales",
        [_, rest @ ..] => {
            if rest.len() > 2 {
                "largo"
            } else {
                "corto"
            }
        }
    }
}

fn language(t: &mut T) {
    t.section("lenguaje");
    let v = Vec2 { x: black_box(3), y: -4 } + Vec2 { x: 10, y: 20 } * black_box(3);
    show!(t, "operadores", "{v:?}");
    let m = Matrix([[1, 2, 3], [black_box(4), 5, 6]]).transpose();
    show!(t, "const generics", "{:?}", m.0);
    let expr = Expr::Add(
        Box::new(Expr::Mul(Box::new(Expr::Num(black_box(6))), Box::new(Expr::Num(7)))),
        Box::new(Expr::Neg(Box::new(Expr::Num(black_box(2))))),
    );
    show!(t, "enum recursivo", "{} <- {:?}", eval(&expr), expr);
    show!(t, "operador ?", "{:?} {:?} {:?}", parse_pair("12: 250"), parse_pair("7"), parse_pair("1:300"));
    show!(t, "genericos", "{:?} {:?} {:?}", largest(&[3, 9, 2]), largest(&[1.5, -2.0]), largest::<char>(&[]));
    t.check("ackermann(2, 3) = 9", ackermann(black_box(2), black_box(3)) == 9);
    show!(t, "patrones de slice", "{} {} {} {} {}", classify(&[]), classify(&[-1]), classify(&[4, 5, 4]), classify(&[1, 2, 3, 4]), classify(&[1, 2]));

    let data: Vec<i32> = (1..=black_box(20)).collect();
    let evens_squared: i32 = data.iter().filter(|x| *x % 2 == 0).map(|x| x * x).sum();
    let windows: Vec<i32> = data.windows(3).map(|w| w.iter().product()).step_by(5).collect();
    let chunks: Vec<i32> = data.chunks(6).map(|c| c.iter().sum()).collect();
    let mut zipped: Vec<(i32, char)> = data.iter().copied().zip("rust".chars()).collect();
    zipped.reverse();
    show!(t, "iteradores", "{evens_squared} {windows:?} {chunks:?} {zipped:?}");
    let pos = data.iter().position(|&x| x * x > 200);
    let (small, large): (Vec<i32>, Vec<i32>) = data.iter().partition(|&&x| x < 5);
    show!(t, "position/partition", "{:?} {} {}", pos, small.len(), large.len());
    show!(t, "min/max_by_key", "{:?} {:?}", data.iter().max_by_key(|&&x| (x % 7, x)), data.iter().min_by_key(|&&x| (x * 13) % 11));

    let mut counter = 0;
    let mut bump = |by: i32| {
        counter += by;
        counter
    };
    let steps = [bump(1), bump(10), bump(100)];
    show!(t, "closures FnMut", "{steps:?} {counter}");
    let adders: Vec<Box<dyn Fn(i32) -> i32>> = (1..=3).map(|k| Box::new(move |x| x + k) as Box<dyn Fn(i32) -> i32>).collect();
    show!(t, "closures move", "{:?}", adders.iter().map(|f| f(black_box(10))).collect::<Vec<_>>());

    let mut a = [black_box(1), 2, 3];
    let mut b = [7, 8, 9];
    core::mem::swap(&mut a, &mut b);
    let c = a.map(|x| x * 2);
    show!(t, "arrays", "{a:?} {b:?} {c:?} {}", c.contains(&16));
    let label = 'outer: loop {
        for i in 0.. {
            if i * i > black_box(500) {
                break 'outer i;
            }
        }
    };
    t.check("break con valor", label == 23);
    let text = if let Some(x) = black_box(Some(5)).filter(|x| x % 2 == 1).map(|x| x * 3) { x } else { 0 };
    t.check("combinadores de Option", text == 15);
}

// ---------------------------------------------------------------- algoritmos

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn algorithms(t: &mut T) {
    t.section("algoritmos");
    let limit = black_box(10_000usize);
    let mut sieve = vec![true; limit];
    sieve[0] = false;
    sieve[1] = false;
    let mut i = 2;
    while i * i < limit {
        if sieve[i] {
            let mut j = i * i;
            while j < limit {
                sieve[j] = false;
                j += i;
            }
        }
        i += 1;
    }
    let primes = sieve.iter().filter(|&&p| p).count();
    t.check("primos < 10000 = 1229", primes == 1229);
    t.check("crc32(\"123456789\")", crc32(black_box(b"123456789")) == 0xCBF4_3926);

    let mut best = (0u32, 0u32);
    for start in 1..black_box(3000u32) {
        let mut n = start as u64;
        let mut steps = 0;
        while n != 1 {
            n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 };
            steps += 1;
        }
        if steps > best.1 {
            best = (start, steps);
        }
    }
    show!(t, "collatz < 3000", "{best:?}");

    // Ordenación de 5000 enteros de 64 bits pseudoaleatorios.
    let mut rng = XorShift(black_box(42));
    let mut data: Vec<u64> = (0..5000).map(|_| rng.next()).collect();
    data.sort();
    let sorted = data.windows(2).all(|w| w[0] <= w[1]);
    t.check("sort de 5000 u64", sorted);
    show!(t, "mediana", "{:#018x}", data[2500]);

    // Multiplicación de matrices 16x16 en f64 (software) y traza.
    let n = 16;
    let a: Vec<f64> = (0..n * n).map(|i| ((i * 7 + 3) % 11) as f64 - 5.0).collect();
    let b: Vec<f64> = (0..n * n).map(|i| ((i * 5 + 1) % 13) as f64 * 0.25).collect();
    let mut trace = 0.0;
    for i in 0..n {
        for k in 0..n {
            trace += a[i * n + k] * b[k * n + i];
        }
    }
    show!(t, "traza(A*B)", "{trace}");

    // Torres de Hanoi: 2^n - 1 movimientos.
    fn hanoi(n: u32, moves: &mut u32) {
        if n > 0 {
            hanoi(n - 1, moves);
            *moves += 1;
            hanoi(n - 1, moves);
        }
    }
    let mut moves = 0;
    hanoi(black_box(12), &mut moves);
    t.check("hanoi(12) = 4095", moves == 4095);
}

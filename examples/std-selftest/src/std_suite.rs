//! Batería de pruebas de la biblioteca estándar: hilos, sincronización,
//! tiempo, archivos, entorno y E/S.
//!
//! Se compila igual para PlayStation 2 y para el PC; las dos salidas deben
//! coincidir byte a byte. Por eso no se imprimen rutas absolutas ni tiempos,
//! sólo resultados deterministas (los tiempos se comprueban con márgenes).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::hint::black_box;
use std::io::{self, BufRead, BufReader, BufWriter, Cursor, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Condvar, LazyLock, Mutex, Once, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct T {
    out: String,
    total: u32,
    failed: u32,
}

impl T {
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

    fn show(&mut self, name: &str, value: std::fmt::Arguments) {
        let _ = writeln!(self.out, "   {name} = {value}");
    }
}

macro_rules! show {
    ($t:expr, $name:expr, $($fmt:tt)*) => { $t.show($name, format_args!($($fmt)*)) };
}

/// Ejecuta la batería. Devuelve la salida (para imprimirla de una vez) y el
/// número de fallos.
pub fn run() -> (String, u32) {
    let mut t = T { out: String::new(), total: 0, failed: 0 };
    collections(&mut t);
    io_in_memory(&mut t);
    threads(&mut t);
    sync(&mut t);
    time(&mut t);
    filesystem(&mut t);
    environment(&mut t);
    let _ = writeln!(t.out, "== resultado std: {} comprobaciones, {} fallos", t.total, t.failed);
    (t.out, t.failed)
}

// ------------------------------------------------------------- colecciones

fn collections(t: &mut T) {
    t.section("std: colecciones con hash");
    let text = black_box("el veloz murcielago hindu comia feliz cardillo y kiwi la ciguena tocaba el saxofon detras del palenque de paja");
    let mut freq: HashMap<char, u32> = HashMap::new();
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        *freq.entry(c).or_default() += 1;
    }
    let mut top: Vec<(char, u32)> = freq.iter().map(|(&c, &n)| (c, n)).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    show!(t, "letras distintas", "{}", freq.len());
    show!(t, "mas frecuentes", "{:?}", &top[..5]);

    let mut map: HashMap<u64, String> = HashMap::new();
    for i in 0..black_box(5000u64) {
        map.insert(i.wrapping_mul(0x9e37_79b9_7f4a_7c15), format!("v{i}"));
    }
    for i in (0..5000u64).step_by(3) {
        map.remove(&i.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    }
    let present = (0..5000u64).filter(|i| map.contains_key(&i.wrapping_mul(0x9e37_79b9_7f4a_7c15))).count();
    t.check("HashMap con 5000 claves y borrados", map.len() == 3333 && present == 3333);
    t.check("HashMap::get", map.get(&(7u64.wrapping_mul(0x9e37_79b9_7f4a_7c15))).map(String::as_str) == Some("v7"));

    let a: HashSet<i32> = (0..black_box(40)).filter(|x| x % 2 == 0).collect();
    let b: HashSet<i32> = (0..40).filter(|x| x % 3 == 0).collect();
    let both: BTreeSet<i32> = a.intersection(&b).copied().collect();
    let either = a.union(&b).count();
    show!(t, "HashSet", "{:?} {} {}", both, either, a.symmetric_difference(&b).count());

    let mut words: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, w) in text.split(' ').enumerate() {
        words.entry(w).or_default().push(i);
    }
    let mut repeated: Vec<_> = words.into_iter().filter(|(_, v)| v.len() > 1).collect();
    repeated.sort();
    show!(t, "palabras repetidas", "{:?}", repeated);
}

// ------------------------------------------------------------ E/S en memoria

fn io_in_memory(t: &mut T) {
    t.section("std: io en memoria");
    let mut buf = Vec::new();
    {
        let mut w = BufWriter::new(&mut buf);
        for i in 0..black_box(5) {
            writeln!(w, "linea {i}: {}", "#".repeat(i)).unwrap();
        }
        w.flush().unwrap();
    }
    let reader = BufReader::new(Cursor::new(&buf));
    let lines: Vec<String> = reader.lines().map(Result::unwrap).collect();
    show!(t, "BufWriter + lines", "{:?}", lines);

    let mut c = Cursor::new(vec![0u8; 16]);
    c.seek(SeekFrom::Start(4)).unwrap();
    c.write_all(b"PS2!").unwrap();
    c.seek(SeekFrom::End(-2)).unwrap();
    c.write_all(&[0xAA, 0xBB]).unwrap();
    c.seek(SeekFrom::Current(-6)).unwrap();
    let mut tail = [0u8; 6];
    c.read_exact(&mut tail).unwrap();
    show!(t, "Cursor", "{:02x?} {:02x?}", &c.get_ref()[..8], tail);
    let err = Cursor::new(b"ab".to_vec()).read_exact(&mut [0u8; 3]).unwrap_err();
    show!(t, "read_exact corto", "{:?}", err.kind());
    let mut chain = Cursor::new(b"hola ".to_vec()).chain(Cursor::new(b"consola".to_vec()));
    let mut s = String::new();
    chain.read_to_string(&mut s).unwrap();
    show!(t, "chain", "{s}");
    let e = io::Error::new(ErrorKind::InvalidData, "dato raro");
    show!(t, "io::Error", "{} {:?}", e, e.kind());
}

// ------------------------------------------------------------------- hilos

fn threads(t: &mut T) {
    t.section("std: hilos");
    let handles: Vec<_> = (1..=black_box(6u64))
        .map(|k| {
            thread::spawn(move || {
                let mut sum = 0u64;
                for i in 0..20_000 * k {
                    sum = sum.wrapping_add(i * i % 7919);
                    if i % 5000 == 0 {
                        thread::yield_now();
                    }
                }
                (k, sum)
            })
        })
        .collect();
    let results: Vec<(u64, u64)> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    show!(t, "spawn + join", "{:?}", results);

    let named = thread::Builder::new()
        .name("trabajador".into())
        .stack_size(64 * 1024)
        .spawn(|| thread::current().name().map(str::to_owned))
        .unwrap();
    show!(t, "Builder con nombre", "{:?}", named.join().unwrap());
    t.check("ids distintos", {
        let main_id = thread::current().id();
        thread::spawn(move || thread::current().id() != main_id).join().unwrap()
    });

    // Canal: tres productores, un consumidor.
    let (tx, rx) = mpsc::channel::<(u32, u32)>();
    for p in 0..3 {
        let tx = tx.clone();
        thread::spawn(move || {
            for i in 0..100 {
                tx.send((p, i)).unwrap();
                if i % 10 == 0 {
                    thread::yield_now();
                }
            }
        });
    }
    drop(tx);
    let mut per_producer = [0u32; 3];
    let mut total = 0u64;
    for (p, i) in rx {
        per_producer[p as usize] += 1;
        total += i as u64;
    }
    show!(t, "mpsc", "{:?} {}", per_producer, total);

    let (tx, rx) = mpsc::sync_channel::<u32>(2);
    let producer = thread::spawn(move || {
        for i in 0..20 {
            tx.send(i * i).unwrap();
        }
    });
    let received: Vec<u32> = rx.iter().collect();
    producer.join().unwrap();
    show!(t, "sync_channel(2)", "{} {}", received.len(), received.iter().sum::<u32>());
    let (tx, rx) = mpsc::channel::<u8>();
    t.check("recv_timeout vacio", rx.recv_timeout(Duration::from_millis(20)) == Err(mpsc::RecvTimeoutError::Timeout));
    drop(tx);
    t.check("canal cerrado", rx.recv().is_err());

    // Ámbito: préstamos de la pila del hilo principal.
    let data: Vec<u32> = (1..=black_box(1000)).collect();
    let mut partial = [0u32; 4];
    thread::scope(|s| {
        for (chunk, slot) in data.chunks(250).zip(partial.iter_mut()) {
            s.spawn(move || *slot = chunk.iter().sum());
        }
    });
    show!(t, "thread::scope", "{:?} {}", partial, partial.iter().sum::<u32>());

    thread_local! {
        static COUNTER: Cell<u32> = const { Cell::new(0) };
        static LOG: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    }
    COUNTER.with(|c| c.set(100));
    let others: Vec<u32> = (0..3)
        .map(|k| {
            thread::spawn(move || {
                for _ in 0..=k {
                    COUNTER.with(|c| c.set(c.get() + 1));
                    LOG.with(|l| l.borrow_mut().push(k));
                }
                COUNTER.with(Cell::get) * 10 + LOG.with(|l| l.borrow().len() as u32)
            })
            .join()
            .unwrap()
        })
        .collect();
    show!(t, "thread_local", "principal={} hilos={:?}", COUNTER.with(Cell::get), others);

    // Destructores de thread_local al terminar el hilo.
    static DROPPED: AtomicUsize = AtomicUsize::new(0);
    struct Noisy;
    impl Drop for Noisy {
        fn drop(&mut self) {
            DROPPED.fetch_add(1, Ordering::SeqCst);
        }
    }
    thread_local! { static NOISY: RefCell<Option<Noisy>> = const { RefCell::new(None) }; }
    for _ in 0..3 {
        thread::spawn(|| NOISY.with(|n| *n.borrow_mut() = Some(Noisy))).join().unwrap();
    }
    show!(t, "destructores TLS", "{}", DROPPED.load(Ordering::SeqCst));

    // Hilo suelto (sin join): su memoria se recupera al crear el siguiente.
    let flag = Arc::new(AtomicBool::new(false));
    let f = Arc::clone(&flag);
    drop(thread::spawn(move || f.store(true, Ordering::SeqCst)));
    thread::sleep(Duration::from_millis(5));
    let again = thread::spawn(|| 7).join().unwrap();
    t.check("hilo suelto", flag.load(Ordering::SeqCst) && again == 7);

    let many: u32 = (0..black_box(40u32)).map(|i| thread::spawn(move || i).join().unwrap()).sum();
    t.check("40 hilos seguidos", many == 780);
}

// ----------------------------------------------------------- sincronización

fn sync(t: &mut T) {
    t.section("std: sincronizacion");
    let counter = Arc::new(Mutex::new(0u64));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let c = Arc::clone(&counter);
            thread::spawn(move || {
                for i in 0..1000 {
                    let mut g = c.lock().unwrap();
                    let v = *g;
                    if i % 97 == 0 {
                        // Ceder con el candado tomado: los demás deben esperar.
                        thread::yield_now();
                    }
                    *g = v + 1;
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    show!(t, "Mutex 8x1000", "{}", *counter.lock().unwrap());
    t.check("try_lock libre", counter.try_lock().is_ok());

    // Condvar: ping-pong entre dos hilos.
    let pair = Arc::new((Mutex::new((0u32, false)), Condvar::new()));
    let p2 = Arc::clone(&pair);
    let pong = thread::spawn(move || {
        let (m, cv) = &*p2;
        let mut rounds = 0;
        loop {
            let mut g = cv.wait_while(m.lock().unwrap(), |s| !s.1).unwrap();
            if g.0 >= 10 {
                break rounds;
            }
            g.0 += 1;
            g.1 = false;
            rounds += 1;
            cv.notify_all();
        }
    });
    {
        let (m, cv) = &*pair;
        for _ in 0..10 {
            let mut g = m.lock().unwrap();
            g.1 = true;
            cv.notify_all();
            g = cv.wait_while(g, |s| s.1).unwrap();
            drop(g);
        }
        let mut g = m.lock().unwrap();
        g.1 = true;
        cv.notify_all();
    }
    show!(t, "Condvar ping-pong", "{}", pong.join().unwrap());
    let (m, cv) = (Mutex::new(()), Condvar::new());
    let (_g, res) = cv.wait_timeout(m.lock().unwrap(), Duration::from_millis(15)).unwrap();
    t.check("Condvar::wait_timeout vence", res.timed_out());

    let table = Arc::new(RwLock::new(vec![1u32, 2, 3]));
    let readers: Vec<_> = (0..4)
        .map(|k| {
            let t = Arc::clone(&table);
            thread::spawn(move || {
                let g = t.read().unwrap();
                thread::yield_now();
                g.iter().sum::<u32>() * k
            })
        })
        .collect();
    table.write().unwrap().push(4);
    let sums: Vec<u32> = readers.into_iter().map(|h| h.join().unwrap()).collect();
    show!(t, "RwLock", "{:?} {:?}", table.read().unwrap(), sums.iter().map(|s| s % 6 == 0 || s % 10 == 0).collect::<Vec<_>>());

    let barrier = Arc::new(Barrier::new(4));
    let arrived = Arc::new(AtomicUsize::new(0));
    let hs: Vec<_> = (0..4)
        .map(|_| {
            let b = Arc::clone(&barrier);
            let a = Arc::clone(&arrived);
            thread::spawn(move || {
                a.fetch_add(1, Ordering::SeqCst);
                let leader = b.wait().is_leader();
                (a.load(Ordering::SeqCst), leader)
            })
        })
        .collect();
    let seen: Vec<(usize, bool)> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    show!(t, "Barrier", "{} {}", seen.iter().all(|s| s.0 == 4), seen.iter().filter(|s| s.1).count());

    static INIT: Once = Once::new();
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    for _ in 0..3 {
        INIT.call_once(|| {
            CALLS.fetch_add(1, Ordering::SeqCst);
        });
    }
    static CELL: OnceLock<String> = OnceLock::new();
    let hs: Vec<_> = (0..3).map(|i| thread::spawn(move || CELL.get_or_init(|| format!("hilo {i}")).len())).collect();
    let lens: Vec<usize> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    static LAZY: LazyLock<Vec<u32>> = LazyLock::new(|| (1..=5).map(|x| x * x).collect());
    show!(t, "Once/OnceLock/LazyLock", "{} {:?} {:?}", CALLS.load(Ordering::SeqCst), lens, *LAZY);

    // park/unpark.
    let ready = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&ready);
    let parked = thread::spawn(move || {
        while !r.load(Ordering::SeqCst) {
            thread::park();
        }
        "despierto"
    });
    thread::sleep(Duration::from_millis(5));
    ready.store(true, Ordering::SeqCst);
    parked.thread().unpark();
    show!(t, "park/unpark", "{}", parked.join().unwrap());
}

// ------------------------------------------------------------------- tiempo

fn time(t: &mut T) {
    t.section("std: tiempo");
    let start = Instant::now();
    thread::sleep(Duration::from_millis(50));
    let slept = start.elapsed();
    t.check("sleep(50 ms) dura entre 50 y 250 ms", slept >= Duration::from_millis(50) && slept < Duration::from_millis(250));
    let a = Instant::now();
    let b = Instant::now();
    t.check("Instant monotono", b >= a && b.duration_since(a) < Duration::from_millis(50));

    // Los hilos que duermen menos se despiertan antes.
    let order = Arc::new(Mutex::new(Vec::new()));
    let hs: Vec<_> = [40u64, 10, 30, 20]
        .into_iter()
        .map(|ms| {
            let o = Arc::clone(&order);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(ms));
                o.lock().unwrap().push(ms);
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    show!(t, "orden al despertar", "{:?}", order.lock().unwrap());

    let now = SystemTime::now();
    let secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    t.check("SystemTime posterior a 2024", secs > 1_704_067_200);
    let later = SystemTime::now();
    t.check("SystemTime avanza", later >= now);
}

// ----------------------------------------------------------------- archivos

const DIR: &str = "std_selftest_tmp";

fn checksum(data: &[u8]) -> u32 {
    data.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

fn filesystem(t: &mut T) {
    t.section("std: archivos");
    let dir = Path::new(DIR);
    let _ = fs::remove_dir_all(dir);
    t.check("create_dir_all", fs::create_dir_all(dir.join("sub")).is_ok());
    t.check("create_dir sobre existente falla", fs::create_dir(dir).map_err(|e| e.kind()) == Err(ErrorKind::AlreadyExists));

    let notes = dir.join("notas.txt");
    fs::write(&notes, "primera linea\nsegunda linea\n").unwrap();
    {
        let mut f = OpenOptions::new().append(true).open(&notes).unwrap();
        writeln!(f, "tercera, agregada").unwrap();
    }
    let text = fs::read_to_string(&notes).unwrap();
    show!(t, "write + append + read", "{:?}", text);
    show!(t, "metadata", "len={} file={} dir={}", fs::metadata(&notes).unwrap().len(), fs::metadata(&notes).unwrap().is_file(), fs::metadata(dir).unwrap().is_dir());
    let modified = fs::metadata(&notes).unwrap().modified().unwrap();
    let skew = match SystemTime::now().duration_since(modified) {
        Ok(d) => d,
        Err(e) => e.duration(),
    };
    t.check("fecha de modificacion razonable (+-1 dia)", skew < Duration::from_secs(86_400));

    // Archivo grande en varios trozos, lectura parcial con seek.
    let big: Vec<u8> = (0..black_box(100_000u32)).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    {
        let mut w = BufWriter::new(File::create(dir.join("grande.bin")).unwrap());
        for chunk in big.chunks(7_000) {
            w.write_all(chunk).unwrap();
        }
    }
    let back = fs::read(dir.join("grande.bin")).unwrap();
    t.check("100 KB escritos y leidos", back.len() == big.len() && checksum(&back) == checksum(&big));
    {
        // Cerrado al salir del bloque: un archivo abierto no se puede borrar.
        let mut f = File::open(dir.join("grande.bin")).unwrap();
        f.seek(SeekFrom::Start(54_321)).unwrap();
        let mut piece = [0u8; 9];
        f.read_exact(&mut piece).unwrap();
        t.check("seek + read_exact", piece[..] == big[54_321..54_330]);
        show!(t, "posicion", "{} {}", f.stream_position().unwrap(), f.seek(SeekFrom::End(-1)).unwrap());
    }

    show!(t, "copy", "{}", fs::copy(&notes, dir.join("sub/copia.txt")).unwrap());
    fs::rename(dir.join("sub/copia.txt"), dir.join("sub/movida.txt")).unwrap();
    t.check("rename", !dir.join("sub/copia.txt").exists() && dir.join("sub/movida.txt").exists());
    fs::write(dir.join("sub/b.dat"), [1, 2, 3]).unwrap();

    let mut names: Vec<(String, bool)> = fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name().to_string_lossy().into_owned(), e.file_type().unwrap().is_dir())
        })
        .collect();
    names.sort();
    show!(t, "read_dir", "{:?}", names);
    let mut sub: Vec<String> = fs::read_dir(dir.join("sub")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    sub.sort();
    show!(t, "read_dir sub", "{:?}", sub);

    // Nombres de todas las longitudes y alineaciones: las rutas se copian
    // byte a byte hasta el IOP (regresión de un fallo de cargas no alineadas).
    let lens = dir.join("largos");
    fs::create_dir(&lens).unwrap();
    for n in 1..=16usize {
        let name: String = "partida_guardada".chars().take(n).collect();
        fs::write(lens.join(&name), name.as_bytes()).unwrap();
    }
    let mut listed: Vec<String> = fs::read_dir(&lens).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    listed.sort();
    let contents_ok = listed.iter().all(|n| fs::read_to_string(lens.join(n)).map(|c| &c == n).unwrap_or(false));
    show!(t, "nombres de 1 a 16", "{:?} {}", listed, contents_ok);
    fs::write("partida.txt", "literal").unwrap();
    show!(t, "literal en el directorio actual", "{:?}", fs::read_to_string("partida.txt"));
    fs::remove_file("partida.txt").unwrap();

    let missing = File::open(dir.join("no_existe.txt")).map(drop).map_err(|e| e.kind());
    show!(t, "abrir inexistente", "{:?}", missing);
    let dup = OpenOptions::new().write(true).create_new(true).open(&notes).map(drop).map_err(|e| e.kind());
    show!(t, "create_new sobre existente", "{:?}", dup);
    t.check("remove_dir con contenido falla", fs::remove_dir(dir.join("sub")).is_err());

    fs::remove_file(&notes).unwrap();
    t.check("remove_file", !notes.exists() && fs::metadata(&notes).is_err());
    t.check("remove_dir_all", fs::remove_dir_all(dir).is_ok() && !dir.exists());
}

// ----------------------------------------------------------------- entorno

fn environment(t: &mut T) {
    t.section("std: entorno");
    t.check("args no vacio", std::env::args().count() >= 1);
    // SAFETY: un solo hilo toca el entorno en este punto.
    unsafe { std::env::set_var("PS2_PRUEBA", "Emotion Engine") };
    show!(t, "var", "{:?} {:?}", std::env::var("PS2_PRUEBA"), std::env::var("NO_DEFINIDA").is_err());
    unsafe { std::env::remove_var("PS2_PRUEBA") };
    t.check("remove_var", std::env::var_os("PS2_PRUEBA").is_none());
    t.check("current_dir", std::env::current_dir().is_ok());
    let p: PathBuf = ["datos", "nivel1", "mapa.bin"].iter().collect();
    let parts: Vec<String> = p.iter().map(|c| c.to_string_lossy().into_owned()).collect();
    show!(t, "PathBuf", "{:?} {:?} {:?}", parts, p.extension(), p.parent().and_then(Path::file_name));
}

//! El formato de las muestras del SPU2: ADPCM de 4 bits en bloques de 16
//! bytes (28 muestras), el mismo de la PlayStation original.
//!
//! Cada bloque: un byte con el predictor (0..=4) y el desplazamiento (0..=12),
//! un byte de marcas (1 = fin, 2 = repetir, 4 = inicio del bucle) y 14 bytes
//! con dos muestras de 4 bits cada uno (la primera en los bits bajos).

/// Coeficientes de los cinco predictores, en 1/64, sobre las dos muestras
/// anteriores.
const FILTERS: [(i32, i32); 5] = [(0, 0), (60, 0), (115, -52), (98, -55), (122, -60)];

/// Muestras por bloque.
pub const SAMPLES_PER_BLOCK: usize = 28;

fn clamp16(v: i32) -> i32 {
    v.clamp(-32768, 32767)
}

/// Codifica un bloque con un predictor y un desplazamiento dados. Devuelve
/// los 28 valores de 4 bits, el error cuadrático y la historia final.
fn encode_block(samples: &[i16; 28], filter: usize, shift: u32, history: (i32, i32)) -> ([i8; 28], u64, (i32, i32)) {
    let (k1, k2) = FILTERS[filter];
    let (mut s1, mut s2) = history;
    let mut nibbles = [0i8; 28];
    let mut error = 0u64;
    for (i, &sample) in samples.iter().enumerate() {
        let predicted = (s1 * k1 + s2 * k2) >> 6;
        // El decodificador reconstruye `nibble << (12 - shift)`.
        let step = 12 - shift;
        let residual = sample as i32 - predicted;
        let nibble = ((residual + ((1 << step) >> 1)) >> step).clamp(-8, 7);
        let decoded = clamp16(predicted + (nibble << step));
        let diff = (sample as i32 - decoded) as i64;
        error += (diff * diff) as u64;
        nibbles[i] = nibble as i8;
        s2 = s1;
        s1 = decoded;
    }
    (nibbles, error, (s1, s2))
}

/// Codifica PCM mono de 16 bits. Con `looped`, la muestra se repite entera
/// hasta que se suelte la voz; si no, suena una vez. El resultado se puede
/// subir tal cual a la memoria de sonido.
pub fn encode(pcm: &[i16], looped: bool) -> Vec<u8> {
    let blocks = pcm.len().div_ceil(SAMPLES_PER_BLOCK).max(1);
    let mut out = Vec::with_capacity((blocks + 1) * 16);
    let mut history = (0i32, 0i32);
    for block in 0..blocks {
        let mut samples = [0i16; 28];
        let start = block * SAMPLES_PER_BLOCK;
        let available = pcm.len().saturating_sub(start).min(SAMPLES_PER_BLOCK);
        samples[..available].copy_from_slice(&pcm[start..start + available]);
        // Se prueba todo y se queda lo que menos error deja.
        let mut best: Option<(usize, u32, [i8; 28], u64, (i32, i32))> = None;
        for filter in 0..FILTERS.len() {
            for shift in 0..=12 {
                let (nibbles, error, end) = encode_block(&samples, filter, shift, history);
                if best.as_ref().map_or(true, |b| error < b.3) {
                    best = Some((filter, shift, nibbles, error, end));
                }
            }
        }
        let (filter, shift, nibbles, _, end) = best.unwrap();
        history = end;
        let last = block + 1 == blocks;
        let flags = match (looped, block == 0, last) {
            (true, true, true) => 0x07,
            (true, true, false) => 0x06,
            (true, false, true) => 0x03,
            (true, false, false) => 0x02,
            (false, _, true) => 0x01,
            (false, _, false) => 0x00,
        };
        out.push((filter as u8) << 4 | shift as u8);
        out.push(flags);
        for pair in nibbles.chunks(2) {
            out.push((pair[0] as u8 & 15) | (pair[1] as u8 & 15) << 4);
        }
    }
    if !looped {
        // Bloque final mudo: la voz se queda parada en él.
        out.push(0);
        out.push(0x07);
        out.extend_from_slice(&[0; 14]);
    }
    out
}

/// Decodifica bloques ADPCM como lo hace el SPU2. Sirve para comprobar el
/// codificador y lo que se lee de la memoria de sonido.
pub fn decode(data: &[u8]) -> Vec<i16> {
    let mut out = Vec::with_capacity(data.len() / 16 * SAMPLES_PER_BLOCK);
    let (mut s1, mut s2) = (0i32, 0i32);
    for block in data.chunks_exact(16) {
        let (filter, shift) = ((block[0] >> 4) as usize, (block[0] & 15) as u32);
        if filter >= FILTERS.len() || shift > 12 {
            break;
        }
        let (k1, k2) = FILTERS[filter];
        for i in 0..SAMPLES_PER_BLOCK {
            let raw = (block[2 + i / 2] >> (4 * (i % 2))) & 15;
            let nibble = ((raw as i8) << 4 >> 4) as i32;
            let decoded = clamp16(((s1 * k1 + s2 * k2) >> 6) + (nibble << (12 - shift)));
            out.push(decoded as i16);
            s2 = s1;
            s1 = decoded;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(len: usize, period: f64, amplitude: f64) -> Vec<i16> {
        (0..len).map(|i| ((i as f64 * std::f64::consts::TAU / period).sin() * amplitude) as i16).collect()
    }

    /// Relación señal/ruido en decibelios.
    fn snr(original: &[i16], decoded: &[i16]) -> f64 {
        let signal: f64 = original.iter().map(|&s| (s as f64).powi(2)).sum();
        let noise: f64 = original.iter().zip(decoded).map(|(&a, &b)| (a as f64 - b as f64).powi(2)).sum();
        10.0 * (signal / noise.max(1.0)).log10()
    }

    #[test]
    fn round_trip_keeps_the_signal() {
        for (period, amplitude) in [(109.0, 20000.0), (24.0, 30000.0), (400.0, 500.0)] {
            let pcm = sine(2800, period, amplitude);
            let encoded = encode(&pcm, false);
            assert_eq!(encoded.len(), (100 + 1) * 16);
            let decoded = decode(&encoded);
            let quality = snr(&pcm, &decoded[..pcm.len()]);
            assert!(quality > 25.0, "periodo {period}: {quality:.1} dB");
        }
    }

    #[test]
    fn flags_mark_the_end_and_the_loop() {
        let pcm = sine(28 * 3, 30.0, 9000.0);
        let once = encode(&pcm, false);
        let flags: Vec<u8> = once.chunks(16).map(|b| b[1]).collect();
        assert_eq!(flags, [0, 0, 1, 7]);
        let looped = encode(&pcm, true);
        let flags: Vec<u8> = looped.chunks(16).map(|b| b[1]).collect();
        assert_eq!(flags, [6, 2, 3]);
        assert_eq!(encode(&pcm[..28], true)[1], 7);
        // Una muestra que no llena el último bloque se completa con silencio.
        assert_eq!(encode(&pcm[..30], false).len(), 3 * 16);
        assert_eq!(encode(&[], false).len(), 2 * 16);
    }

    #[test]
    fn silence_and_extremes() {
        assert!(decode(&encode(&[0; 56], false)).iter().all(|&s| s == 0));
        let square: Vec<i16> = (0..280).map(|i| if i / 14 % 2 == 0 { 32767 } else { -32768 }).collect();
        let decoded = decode(&encode(&square, false));
        assert!(snr(&square, &decoded[..280]) > 10.0);
    }
}

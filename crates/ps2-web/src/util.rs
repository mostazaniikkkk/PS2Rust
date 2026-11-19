//! SHA-1, Base64 y números aleatorios: lo que pide el saludo de WebSocket.

/// SHA-1 (RFC 3174). Sólo se usa para el saludo de WebSocket, no como
/// protección de nada.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i / 20 {
                0 => ((b & c) | (!b & d), 0x5A82_7999),
                1 => (b ^ c ^ d, 0x6ED9_EBA1),
                2 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(v);
        }
    }
    let mut out = [0u8; 20];
    for (chunk, word) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Bytes impredecibles para la clave del saludo y las máscaras de las
/// tramas. Salen de las semillas aleatorias de `HashMap` de la std.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0u8; N];
    for chunk in out.chunks_mut(8) {
        let value = RandomState::new().build_hasher().finish().to_le_bytes();
        chunk.copy_from_slice(&value[..chunk.len()]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha1_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
        assert_eq!(hex(&sha1(&[b'a'; 1000])), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
    }

    #[test]
    fn base64_vectors() {
        for (input, output) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(input.as_bytes()), output);
        }
        // El ejemplo del saludo de la RFC 6455.
        let accept = base64(&sha1(b"dGhlIHNhbXBsZSBub25jZQ==258EAFA5-E914-47DA-95CA-C5AB0DC85B11"));
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn random_bytes_differ() {
        assert_ne!(random_bytes::<16>(), random_bytes::<16>());
    }
}

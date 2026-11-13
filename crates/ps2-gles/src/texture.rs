//! Conversión de las imágenes de `glTexImage2D` al formato único del
//! rasterizador (RGBA de 8 bits) y descompresión de las texturas con paleta
//! (`OES_compressed_paletted_texture`).

use crate::consts::*;
use crate::raster::TexFormat;
use alloc::vec::Vec;

pub fn format_of(format: GLenum) -> Option<TexFormat> {
    Some(match format {
        GL_ALPHA => TexFormat::Alpha,
        GL_LUMINANCE => TexFormat::Luminance,
        GL_LUMINANCE_ALPHA => TexFormat::LuminanceAlpha,
        GL_RGB => TexFormat::Rgb,
        GL_RGBA => TexFormat::Rgba,
        _ => return None,
    })
}

/// Bytes por píxel de una combinación formato/tipo válida en GLES 1.0.
pub fn bytes_per_pixel(format: GLenum, kind: GLenum) -> Option<usize> {
    Some(match (format, kind) {
        (GL_RGBA, GL_UNSIGNED_BYTE) => 4,
        (GL_RGB, GL_UNSIGNED_BYTE) => 3,
        (GL_RGBA, GL_UNSIGNED_SHORT_4_4_4_4) | (GL_RGBA, GL_UNSIGNED_SHORT_5_5_5_1) | (GL_RGB, GL_UNSIGNED_SHORT_5_6_5) => 2,
        (GL_LUMINANCE_ALPHA, GL_UNSIGNED_BYTE) => 2,
        (GL_LUMINANCE, GL_UNSIGNED_BYTE) | (GL_ALPHA, GL_UNSIGNED_BYTE) => 1,
        _ => return None,
    })
}

fn expand(value: u16, bits: u32) -> u8 {
    ((value as u32 * 255 + (1 << bits) / 2 - (bits == 1) as u32 * 1) / ((1 << bits) - 1)) as u8
}

fn rgba_of(format: GLenum, kind: GLenum, p: &[u8]) -> [u8; 4] {
    match (format, kind) {
        (GL_RGBA, GL_UNSIGNED_BYTE) => [p[0], p[1], p[2], p[3]],
        (GL_RGB, GL_UNSIGNED_BYTE) => [p[0], p[1], p[2], 255],
        (GL_LUMINANCE_ALPHA, _) => [p[0], p[0], p[0], p[1]],
        (GL_LUMINANCE, _) => [p[0], p[0], p[0], 255],
        (GL_ALPHA, _) => [255, 255, 255, p[0]],
        _ => {
            let v = u16::from_le_bytes([p[0], p[1]]);
            match kind {
                GL_UNSIGNED_SHORT_4_4_4_4 => [expand(v >> 12, 4), expand((v >> 8) & 15, 4), expand((v >> 4) & 15, 4), expand(v & 15, 4)],
                GL_UNSIGNED_SHORT_5_5_5_1 => [expand(v >> 11, 5), expand((v >> 6) & 31, 5), expand((v >> 1) & 31, 5), expand(v & 1, 1)],
                _ => [expand(v >> 11, 5), expand((v >> 5) & 63, 6), expand(v & 31, 5), 255],
            }
        }
    }
}

/// Convierte `w`×`h` píxeles a RGBA de 8 bits. Las filas de `data` están
/// alineadas a `alignment` bytes (`GL_UNPACK_ALIGNMENT`).
///
/// # Safety
///
/// `data` debe apuntar a una imagen completa de ese tamaño y formato.
pub unsafe fn to_rgba(format: GLenum, kind: GLenum, w: usize, h: usize, alignment: usize, data: *const u8) -> Vec<u8> {
    let bpp = bytes_per_pixel(format, kind).unwrap_or(4);
    let row = (w * bpp + alignment - 1) / alignment * alignment;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let line = core::slice::from_raw_parts(data.add(y * row), w * bpp);
        for x in 0..w {
            out.extend_from_slice(&rgba_of(format, kind, &line[x * bpp..x * bpp + bpp]));
        }
    }
    out
}

/// Datos de un formato con paleta: (bits por índice, formato de la paleta,
/// tipo de la paleta).
pub fn palette_format(internal: GLenum) -> Option<(usize, GLenum, GLenum)> {
    Some(match internal {
        GL_PALETTE4_RGB8_OES => (4, GL_RGB, GL_UNSIGNED_BYTE),
        GL_PALETTE4_RGBA8_OES => (4, GL_RGBA, GL_UNSIGNED_BYTE),
        GL_PALETTE4_R5_G6_B5_OES => (4, GL_RGB, GL_UNSIGNED_SHORT_5_6_5),
        GL_PALETTE4_RGBA4_OES => (4, GL_RGBA, GL_UNSIGNED_SHORT_4_4_4_4),
        GL_PALETTE4_RGB5_A1_OES => (4, GL_RGBA, GL_UNSIGNED_SHORT_5_5_5_1),
        GL_PALETTE8_RGB8_OES => (8, GL_RGB, GL_UNSIGNED_BYTE),
        GL_PALETTE8_RGBA8_OES => (8, GL_RGBA, GL_UNSIGNED_BYTE),
        GL_PALETTE8_R5_G6_B5_OES => (8, GL_RGB, GL_UNSIGNED_SHORT_5_6_5),
        GL_PALETTE8_RGBA4_OES => (8, GL_RGBA, GL_UNSIGNED_SHORT_4_4_4_4),
        GL_PALETTE8_RGB5_A1_OES => (8, GL_RGBA, GL_UNSIGNED_SHORT_5_5_5_1),
        _ => return None,
    })
}

pub const PALETTE_FORMATS: [GLenum; 10] = [
    GL_PALETTE4_RGB8_OES,
    GL_PALETTE4_RGBA8_OES,
    GL_PALETTE4_R5_G6_B5_OES,
    GL_PALETTE4_RGBA4_OES,
    GL_PALETTE4_RGB5_A1_OES,
    GL_PALETTE8_RGB8_OES,
    GL_PALETTE8_RGBA8_OES,
    GL_PALETTE8_R5_G6_B5_OES,
    GL_PALETTE8_RGBA4_OES,
    GL_PALETTE8_RGB5_A1_OES,
];

/// Descomprime una textura con paleta: la paleta seguida de los índices de
/// `levels` niveles de mipmap. Devuelve el formato base y, por nivel, su
/// tamaño y sus píxeles RGBA; `None` si `data` es más corto de lo debido.
pub fn decode_paletted(internal: GLenum, w: usize, h: usize, levels: usize, data: &[u8]) -> Option<(GLenum, Vec<(usize, usize, Vec<u8>)>)> {
    let (bits, format, kind) = palette_format(internal)?;
    let entry = bytes_per_pixel(format, kind)?;
    let entries = 1usize << bits;
    let palette = data.get(..entries * entry)?;
    let mut offset = entries * entry;
    let mut out = Vec::new();
    let (mut lw, mut lh) = (w, h);
    for _ in 0..levels {
        let count = lw * lh;
        let bytes = if bits == 4 { (count + 1) / 2 } else { count };
        let indices = data.get(offset..offset + bytes)?;
        offset += bytes;
        let mut pixels = Vec::with_capacity(count * 4);
        for i in 0..count {
            let index = if bits == 8 {
                indices[i] as usize
            } else if i % 2 == 0 {
                (indices[i / 2] >> 4) as usize
            } else {
                (indices[i / 2] & 15) as usize
            };
            pixels.extend_from_slice(&rgba_of(format, kind, &palette[index * entry..index * entry + entry]));
        }
        out.push((lw, lh, pixels));
        lw = (lw / 2).max(1);
        lh = (lh / 2).max(1);
    }
    Some((format, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_formats_expand_to_full_range() {
        assert_eq!(rgba_of(GL_RGB, GL_UNSIGNED_SHORT_5_6_5, &0xffffu16.to_le_bytes()), [255, 255, 255, 255]);
        assert_eq!(rgba_of(GL_RGB, GL_UNSIGNED_SHORT_5_6_5, &0xf800u16.to_le_bytes()), [255, 0, 0, 255]);
        assert_eq!(rgba_of(GL_RGBA, GL_UNSIGNED_SHORT_4_4_4_4, &0x0f08u16.to_le_bytes()), [0, 255, 0, 136]);
        assert_eq!(rgba_of(GL_RGBA, GL_UNSIGNED_SHORT_5_5_5_1, &0x07c1u16.to_le_bytes()), [0, 255, 0, 255]);
        assert_eq!(rgba_of(GL_RGBA, GL_UNSIGNED_SHORT_5_5_5_1, &0x07c0u16.to_le_bytes()), [0, 255, 0, 0]);
        assert_eq!(rgba_of(GL_ALPHA, GL_UNSIGNED_BYTE, &[7]), [255, 255, 255, 7]);
    }

    #[test]
    fn unpack_alignment_skips_row_padding() {
        // 3×2 RGB con filas alineadas a 4 bytes: 9 bytes de datos + 3 de relleno.
        let mut data = [0u8; 24];
        data[0] = 1;
        data[12] = 2;
        let out = unsafe { to_rgba(GL_RGB, GL_UNSIGNED_BYTE, 3, 2, 4, data.as_ptr()) };
        assert_eq!((out.len(), out[0], out[12]), (24, 1, 2));
    }

    #[test]
    fn paletted_4_bit_with_two_levels() {
        let mut data = alloc::vec![0u8; 16 * 3];
        data[3..6].copy_from_slice(&[10, 20, 30]); // entrada 1
        data[6..9].copy_from_slice(&[40, 50, 60]); // entrada 2
        data.extend_from_slice(&[0x12, 0x21]); // nivel 0: 2×2
        data.push(0x20); // nivel 1: 1×1
        let (format, levels) = decode_paletted(GL_PALETTE4_RGB8_OES, 2, 2, 2, &data).unwrap();
        assert_eq!(format, GL_RGB);
        assert_eq!(levels[0].2, [10, 20, 30, 255, 40, 50, 60, 255, 40, 50, 60, 255, 10, 20, 30, 255]);
        assert_eq!((levels[1].0, levels[1].1, &levels[1].2[..]), (1, 1, &[40, 50, 60, 255][..]));
        assert!(decode_paletted(GL_PALETTE4_RGB8_OES, 2, 2, 2, &data[..49]).is_none());
    }
}

//! Device-independent bitmaps re-encoded as PNG, which a browser and resvg
//! both draw.

use super::Reader;
use crate::model::Color;

const MAX_PIXELS: usize = 40_000_000;
const ZCOMPRESS: u32 = 0x0100_0000;

/// A BMP file as a PNG.
pub(crate) fn png_from_bmp(bmp: &[u8]) -> Option<Vec<u8>> {
    dib(
        bmp.get(14..)?,
        Some((read_u32(bmp, 10)? as usize).checked_sub(14)?),
    )
}

/// A DIB behind a BMP file header, as a PNG.
pub(super) fn png_from_file_dib(data: &[u8]) -> Option<Vec<u8>> {
    if data.get(..2)? != b"BM" {
        return None;
    }
    let pixels_at = (read_u32(data, 10)? as usize).checked_sub(14)?;
    dib(data.get(14..)?, Some(pixels_at))
}

/// A DIB whose pixels follow its header and palette, as a PNG.
pub(super) fn png_from_packed_dib(data: &[u8]) -> Option<Vec<u8>> {
    dib(data, None)
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

struct Header {
    size: usize,
    width: usize,
    height: usize,
    top_down: bool,
    bits: u16,
    compression: u32,
    size_image: usize,
    colours: usize,
    masks: Option<[u32; 3]>,
}

impl Header {
    fn read(data: &[u8]) -> Option<Self> {
        let mut r = Reader::new(data);
        let size = r.u32()? as usize;
        let (width, height, bits, compression, size_image, used) = if size == 12 {
            let (w, h) = (i32::from(r.u16()?), i32::from(r.i16()?));
            r.skip(2)?;
            (w, h, r.u16()?, 0, 0, 0)
        } else if size >= 40 {
            let (w, h) = (r.i32()?, r.i32()?);
            r.skip(2)?;
            let bits = r.u16()?;
            let compression = r.u32()?;
            let size_image = r.u32()? as usize;
            r.skip(8)?;
            (w, h, bits, compression, size_image, r.u32()? as usize)
        } else {
            return None;
        };
        let masks = match compression & !ZCOMPRESS {
            3 | 6 => {
                let mut m = Reader::new(data.get(40..)?);
                Some([m.u32()?, m.u32()?, m.u32()?])
            }
            _ => None,
        };
        let colours = match bits {
            1 | 4 | 8 if used == 0 || used > 1 << bits => 1 << bits,
            _ => used.min(256),
        };
        Some(Self {
            size,
            width: usize::try_from(width).ok().filter(|&w| w > 0)?,
            height: height.unsigned_abs() as usize,
            top_down: height < 0,
            bits,
            compression,
            size_image,
            colours,
            masks,
        })
    }

    fn stride(&self) -> usize {
        (self.width * usize::from(self.bits)).div_ceil(32) * 4
    }

    fn palette_at(&self) -> usize {
        match (self.size, self.masks, self.compression) {
            (40, Some(_), 6) => 56,
            (40, Some(_), _) => 52,
            _ => self.size,
        }
    }
}

enum Decoded {
    Rgb(usize, usize, Vec<u8>),
    /// A JPEG or PNG the DIB wraps.
    Encoded(Vec<u8>),
}

fn dib(data: &[u8], pixels_at: Option<usize>) -> Option<Vec<u8>> {
    Some(match decode(data, pixels_at)? {
        Decoded::Rgb(w, h, rgb) => {
            lj_image::png::encode(w as u32, h as u32, 8, lj_image::png::Kind::Rgb, &rgb)
        }
        Decoded::Encoded(bytes) => bytes,
    })
}

/// The mean colour of a DIB whose pixels follow its header and palette.
pub(super) fn mean_of_packed_dib(data: &[u8]) -> Option<Color> {
    let Decoded::Rgb(_, _, rgb) = decode(data, None)? else {
        return None;
    };
    let n = (rgb.len() / 3).max(1) as u64;
    let mut sum = [0u64; 3];
    for px in rgb.as_chunks::<3>().0 {
        for k in 0..3 {
            sum[k] += u64::from(px[k]);
        }
    }
    let [r, g, b] = sum.map(|s| (s / n) as u8);
    Some(Color(r, g, b))
}

fn decode(data: &[u8], pixels_at: Option<usize>) -> Option<Decoded> {
    let mut h = Header::read(data)?;
    if !(1..=MAX_PIXELS).contains(&h.width.checked_mul(h.height)?) {
        return None;
    }
    let entry = if h.size == 12 { 3 } else { 4 };
    let palette_at = h.palette_at();
    if palette_at > data.len() {
        return None;
    }
    let palette: Vec<[u8; 3]> = (0..h.colours)
        .map_while(|i| {
            let p = data.get(palette_at + i * entry..palette_at + i * entry + 3)?;
            Some([p[2], p[1], p[0]])
        })
        .collect();
    let mut pixels = data.get(pixels_at.unwrap_or(palette_at + h.colours * entry)..)?;
    let inflated;
    if h.compression & ZCOMPRESS != 0 {
        let mut r = Reader::new(pixels);
        let (coded, uncoded) = (r.u32()? as usize, r.u32()? as usize);
        h.compression = r.u32()?;
        h.size_image = uncoded;
        if uncoded > MAX_PIXELS * 4 {
            return None;
        }
        inflated = inflate(r.bytes(coded.min(r.rest().len()))?, uncoded)?;
        pixels = &inflated;
    }
    if h.compression == 4 || h.compression == 5 {
        return Some(Decoded::Encoded(pixels.get(..h.size_image)?.to_vec()));
    }
    let (w, height) = (h.width, h.height);
    let mut rgb = vec![255u8; w * height * 3];
    match h.compression {
        1 | 2 => {
            let indices = rle(pixels, w, height, h.compression == 2);
            for (i, &index) in indices.iter().enumerate() {
                let c = palette
                    .get(usize::from(index))
                    .copied()
                    .unwrap_or([0, 0, 0]);
                rgb[i * 3..i * 3 + 3].copy_from_slice(&c);
            }
        }
        0 | 3 | 6 if matches!(h.bits, 1 | 4 | 8 | 16 | 24 | 32) => {
            let stride = h.stride();
            for row in 0..height {
                let Some(line) = pixels.get(row * stride..(row + 1) * stride) else {
                    break;
                };
                let out = if h.top_down { row } else { height - 1 - row } * w * 3;
                for x in 0..w {
                    let c = pixel(&h, &palette, line, x)?;
                    rgb[out + x * 3..out + x * 3 + 3].copy_from_slice(&c);
                }
            }
        }
        _ => return None,
    }
    Some(Decoded::Rgb(w, height, rgb))
}

fn pixel(h: &Header, palette: &[[u8; 3]], line: &[u8], x: usize) -> Option<[u8; 3]> {
    let index = |i: u8| palette.get(usize::from(i)).copied().unwrap_or([0, 0, 0]);
    Some(match h.bits {
        1 => index((line[x / 8] >> (7 - x % 8)) & 1),
        4 => index((line[x / 2] >> if x.is_multiple_of(2) { 4 } else { 0 }) & 15),
        8 => index(line[x]),
        16 => {
            let v = u32::from(u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]));
            h.masks
                .unwrap_or([0x7C00, 0x03E0, 0x001F])
                .map(|m| channel(v, m))
        }
        24 => [line[x * 3 + 2], line[x * 3 + 1], line[x * 3]],
        32 => {
            let v = u32::from_le_bytes(line[x * 4..x * 4 + 4].try_into().ok()?);
            h.masks
                .unwrap_or([0xFF_0000, 0xFF00, 0xFF])
                .map(|m| channel(v, m))
        }
        _ => return None,
    })
}

fn channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let v = (value & mask) >> mask.trailing_zeros();
    let max = mask >> mask.trailing_zeros();
    (u64::from(v) * 255 / u64::from(max)) as u8
}

fn inflate(coded: &[u8], len: usize) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::with_capacity(len);
    flate2::read::ZlibDecoder::new(coded)
        .take(len as u64)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

/// Palette indices of a run-length encoded bitmap, top row first.
fn rle(data: &[u8], width: usize, height: usize, nibbles: bool) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    let (mut x, mut y, mut i) = (0usize, 0usize, 0usize);
    let mut put = |x: &mut usize, y: usize, v: u8| {
        if *x < width && y < height {
            out[(height - 1 - y) * width + *x] = v;
        }
        *x += 1;
    };
    let nibble = |b: u8, k: usize| if k.is_multiple_of(2) { b >> 4 } else { b & 15 };
    while i + 1 < data.len() && y < height {
        let (n, v) = (usize::from(data[i]), data[i + 1]);
        i += 2;
        if n > 0 {
            for k in 0..n {
                put(&mut x, y, if nibbles { nibble(v, k) } else { v });
            }
            continue;
        }
        match v {
            0 => {
                x = 0;
                y += 1;
            }
            1 => break,
            2 => {
                let (Some(&dx), Some(&dy)) = (data.get(i), data.get(i + 1)) else {
                    break;
                };
                x += usize::from(dx);
                y += usize::from(dy);
                i += 2;
            }
            count => {
                let count = usize::from(count);
                for k in 0..count {
                    let Some(&b) = data.get(i + if nibbles { k / 2 } else { k }) else {
                        break;
                    };
                    put(&mut x, y, if nibbles { nibble(b, k) } else { b });
                }
                i += if nibbles { count.div_ceil(2) } else { count }.next_multiple_of(2);
            }
        }
    }
    out
}

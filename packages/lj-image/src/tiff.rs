//! The first image of a TIFF file, as a PNG: strips or tiles, uncompressed,
//! LZW, Deflate, PackBits or CCITT fax, in grey, RGB, palette or CMYK.

use std::io::Read as _;

use crate::bilevel::{self, CcittSettings, EncodingMode};
use crate::png::{self, Kind};
use crate::{lzw, packbits, MAX_PIXELS};

pub fn to_png(data: &[u8]) -> Option<Vec<u8>> {
    let file = File::new(data)?;
    let ifd = Ifd::read(&file, file.u32(4)? as usize)?;
    Image::new(&file, &ifd)?.png(&ifd)
}

struct File<'a> {
    data: &'a [u8],
    big_endian: bool,
}

impl<'a> File<'a> {
    fn new(data: &'a [u8]) -> Option<File<'a>> {
        let big_endian = match data.get(..4)? {
            b"II*\0" => false,
            b"MM\0*" => true,
            _ => return None,
        };
        Some(File { data, big_endian })
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let bytes = self.data.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(match self.big_endian {
            true => u16::from_be_bytes(bytes),
            false => u16::from_le_bytes(bytes),
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let bytes = self.data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(match self.big_endian {
            true => u32::from_be_bytes(bytes),
            false => u32::from_le_bytes(bytes),
        })
    }
}

/// The tags of one image directory, each as its list of integers.
struct Ifd(Vec<(u16, Vec<u32>)>);

impl Ifd {
    fn read(file: &File<'_>, at: usize) -> Option<Ifd> {
        let count = usize::from(file.u16(at)?);
        let mut tags = Vec::with_capacity(count);
        for index in 0..count {
            let entry = at + 2 + 12 * index;
            let (tag, kind, n) = (
                file.u16(entry)?,
                file.u16(entry + 2)?,
                file.u32(entry + 4)? as usize,
            );
            let size = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 => 4,
                _ => continue,
            };
            let start = match n.checked_mul(size)? {
                0..=4 => entry + 8,
                _ => file.u32(entry + 8)? as usize,
            };
            if file.data.len() < start.saturating_add(n.saturating_mul(size)) {
                continue;
            }
            let values = (0..n)
                .map(|k| match size {
                    1 => u32::from(file.data[start + k]),
                    2 => u32::from(file.u16(start + 2 * k).unwrap_or(0)),
                    _ => file.u32(start + 4 * k).unwrap_or(0),
                })
                .collect();
            tags.push((tag, values));
        }
        Some(Ifd(tags))
    }

    fn all(&self, tag: u16) -> &[u32] {
        self.0
            .iter()
            .find(|(t, _)| *t == tag)
            .map_or(&[], |(_, values)| values)
    }

    fn get(&self, tag: u16) -> Option<u32> {
        self.all(tag).first().copied()
    }
}

const WIDTH: u16 = 256;
const HEIGHT: u16 = 257;
const BITS: u16 = 258;
const COMPRESSION: u16 = 259;
const PHOTOMETRIC: u16 = 262;
const FILL_ORDER: u16 = 266;
const STRIP_OFFSETS: u16 = 273;
const SAMPLES: u16 = 277;
const ROWS_PER_STRIP: u16 = 278;
const STRIP_BYTES: u16 = 279;
const PLANAR: u16 = 284;
const T4_OPTIONS: u16 = 292;
const PREDICTOR: u16 = 317;
const COLOR_MAP: u16 = 320;
const TILE_WIDTH: u16 = 322;
const TILE_HEIGHT: u16 = 323;
const TILE_OFFSETS: u16 = 324;
const TILE_BYTES: u16 = 325;
const EXTRA_SAMPLES: u16 = 338;

/// Every sample of the image, one byte each: a palette index, or a value
/// scaled to eight bits.
struct Image {
    width: usize,
    height: usize,
    samples: usize,
    bits: usize,
    samples_data: Vec<u8>,
}

impl Image {
    fn new(file: &File<'_>, ifd: &Ifd) -> Option<Image> {
        let (width, height) = (ifd.get(WIDTH)? as usize, ifd.get(HEIGHT)? as usize);
        if width == 0 || height == 0 || (width as u64) * (height as u64) > MAX_PIXELS {
            return None;
        }
        let samples = ifd.get(SAMPLES).unwrap_or(1).clamp(1, 8) as usize;
        let bits = ifd.get(BITS).unwrap_or(1) as usize;
        if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
            return None;
        }
        let compression = ifd.get(COMPRESSION).unwrap_or(1);
        let planar = ifd.get(PLANAR) == Some(2) && samples > 1;
        let per_chunk = if planar { 1 } else { samples };
        let (planes, chunk_width, chunk_height, offsets, counts) = match ifd.get(TILE_WIDTH) {
            Some(tile_width) => (
                if planar { samples } else { 1 },
                tile_width as usize,
                ifd.get(TILE_HEIGHT)? as usize,
                ifd.all(TILE_OFFSETS),
                ifd.all(TILE_BYTES),
            ),
            None => (
                if planar { samples } else { 1 },
                width,
                (ifd.get(ROWS_PER_STRIP).unwrap_or(u32::MAX) as usize).clamp(1, height),
                ifd.all(STRIP_OFFSETS),
                ifd.all(STRIP_BYTES),
            ),
        };
        if chunk_width == 0
            || chunk_height == 0
            || (chunk_width as u64) * (chunk_height as u64) > MAX_PIXELS
        {
            return None;
        }
        let (across, down) = (width.div_ceil(chunk_width), height.div_ceil(chunk_height));
        let mut image = Image {
            width,
            height,
            samples,
            bits,
            samples_data: vec![0; width * height * samples],
        };
        let reverse_bits = ifd.get(FILL_ORDER) == Some(2);
        let predictor = ifd.get(PREDICTOR) == Some(2);
        let row_bytes = (chunk_width as u64 * (per_chunk * bits) as u64).div_ceil(8);
        // One chunk of the largest image: MAX_PIXELS pixels of eight 16-bit samples.
        if row_bytes * chunk_height as u64 > MAX_PIXELS * 16 {
            return None;
        }
        let row_bytes = row_bytes as usize;
        for plane in 0..planes {
            for down_index in 0..down {
                for across_index in 0..across {
                    let index = (plane * down + down_index) * across + across_index;
                    let (Some(&offset), count) = (offsets.get(index), counts.get(index)) else {
                        continue;
                    };
                    let offset = offset as usize;
                    let end = match count {
                        Some(&count) => offset.saturating_add(count as usize),
                        None => file.data.len(),
                    }
                    .min(file.data.len());
                    let Some(raw) = file.data.get(offset..end) else {
                        continue;
                    };
                    let top = down_index * chunk_height;
                    // A strip holds only the rows left; a tile is always whole.
                    let rows = match ifd.get(TILE_WIDTH) {
                        Some(_) => chunk_height,
                        None => chunk_height.min(height - top),
                    };
                    let mut bytes = match reverse_bits {
                        true => raw.iter().map(|b| b.reverse_bits()).collect(),
                        false => raw.to_vec(),
                    };
                    bytes = match compression {
                        1 => bytes,
                        2..=4 => fax(&bytes, compression, ifd, chunk_width, rows)?,
                        5 => lzw::decode(&bytes, true, row_bytes * rows + 4096).unwrap_or_default(),
                        8 | 32946 => inflate(&bytes, row_bytes * rows),
                        32773 => packbits::decode(&bytes, row_bytes * rows),
                        _ => return None,
                    };
                    bytes.resize(row_bytes * rows, 0);
                    if predictor {
                        undo_predictor(&mut bytes, row_bytes, per_chunk, bits, file.big_endian);
                    }
                    let left = across_index * chunk_width;
                    image.place(
                        &bytes,
                        row_bytes,
                        per_chunk,
                        plane,
                        left,
                        top,
                        chunk_width,
                        rows,
                        file.big_endian,
                    );
                }
            }
        }
        Some(image)
    }

    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        bytes: &[u8],
        row_bytes: usize,
        per_chunk: usize,
        plane: usize,
        left: usize,
        top: usize,
        width: usize,
        rows: usize,
        big_endian: bool,
    ) {
        let bits = self.bits;
        for row in 0..rows.min(self.height.saturating_sub(top)) {
            let line = &bytes[row * row_bytes..(row + 1) * row_bytes];
            let out_row = (top + row) * self.width;
            for column in 0..width.min(self.width.saturating_sub(left)) {
                for sample in 0..per_chunk {
                    let index = column * per_chunk + sample;
                    let value = match bits {
                        8 => line[index],
                        16 => line[2 * index + usize::from(!big_endian)],
                        _ => {
                            let bit = index * bits;
                            (line[bit / 8] >> (8 - bits - bit % 8)) & ((1u8 << bits) - 1)
                        }
                    };
                    let out = (out_row + left + column) * self.samples + plane + sample;
                    self.samples_data[out] = value;
                }
            }
        }
    }

    fn png(self, ifd: &Ifd) -> Option<Vec<u8>> {
        let photometric = ifd.get(PHOTOMETRIC).unwrap_or(1);
        let extra = ifd.get(EXTRA_SAMPLES);
        let pixels = self.width * self.height;
        let max = if self.bits >= 8 {
            255
        } else {
            (1u16 << self.bits) - 1
        };
        let scale = |value: u8| (u16::from(value) * 255 / max) as u8;
        let (colours, kind) = match photometric {
            0 | 1 => (1, Kind::Gray),
            2 if self.samples >= 3 => (3, Kind::Rgb),
            3 => (1, Kind::Rgb),
            5 if self.samples >= 4 => (4, Kind::Rgb),
            _ => return None,
        };
        let alpha = matches!(extra, Some(1 | 2)) && self.samples > colours;
        let premultiplied = extra == Some(1);
        let map = ifd.all(COLOR_MAP);
        let entries = 1usize << self.bits.min(8);
        let (width, height) = (self.width as u32, self.height as u32);
        let mut out = Vec::with_capacity(pixels * 4);
        for pixel in self.samples_data.chunks_exact(self.samples) {
            let mut rgb = match photometric {
                0 => [255 - scale(pixel[0]); 3],
                1 => [scale(pixel[0]); 3],
                2 => [pixel[0], pixel[1], pixel[2]].map(scale),
                3 => {
                    let index = usize::from(pixel[0]);
                    let at = |channel: usize| {
                        map.get(channel * entries + index)
                            .map_or(0, |&v| (v >> 8) as u8)
                    };
                    [at(0), at(1), at(2)]
                }
                _ => {
                    let k = u16::from(255 - pixel[3]);
                    [0, 1, 2].map(|c| (u16::from(255 - pixel[c]) * k / 255) as u8)
                }
            };
            let a = if alpha { scale(pixel[colours]) } else { 255 };
            if premultiplied && a > 0 && a < 255 {
                rgb = rgb.map(|c| (u16::from(c) * 255 / u16::from(a)).min(255) as u8);
            }
            match (alpha, &kind) {
                (true, _) => out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]),
                (false, Kind::Gray) => out.push(rgb[0]),
                (false, _) => out.extend_from_slice(&rgb),
            }
        }
        let kind = match (alpha, kind) {
            (true, _) => Kind::Rgba,
            (false, kind) => kind,
        };
        Some(png::encode(width, height, 8, kind, &out))
    }
}

/// Fax rows with 0 for white, as libtiff hands them to the photometric
/// interpretation.
fn fax(bytes: &[u8], compression: u32, ifd: &Ifd, columns: usize, rows: usize) -> Option<Vec<u8>> {
    let t4 = ifd.get(T4_OPTIONS).unwrap_or(0);
    let settings = CcittSettings {
        columns: columns as u32,
        rows: rows as u32,
        end_of_block: true,
        end_of_line: false,
        // T4 fill bits sit before each EOL, which the decoder skips anyway.
        rows_are_byte_aligned: compression == 2,
        encoding: match compression {
            4 => EncodingMode::Group4,
            3 if t4 & 1 != 0 => EncodingMode::Group3_2D { k: u32::MAX },
            _ => EncodingMode::Group3_1D,
        },
        invert_black: false,
    };
    let decoded = bilevel::ccitt(bytes, settings)?;
    Some(decoded.data.iter().map(|b| !b).collect())
}

fn inflate(bytes: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let _ = flate2::read::ZlibDecoder::new(bytes)
        .take(len as u64)
        .read_to_end(&mut out);
    out
}

/// Horizontal differencing undone, sample by sample along each row.
fn undo_predictor(
    bytes: &mut [u8],
    row_bytes: usize,
    per_pixel: usize,
    bits: usize,
    big_endian: bool,
) {
    for row in bytes.chunks_exact_mut(row_bytes) {
        match bits {
            8 => {
                for index in per_pixel..row.len() {
                    row[index] = row[index].wrapping_add(row[index - per_pixel]);
                }
            }
            16 => {
                let read = |row: &[u8], i: usize| match big_endian {
                    true => u16::from_be_bytes([row[2 * i], row[2 * i + 1]]),
                    false => u16::from_le_bytes([row[2 * i], row[2 * i + 1]]),
                };
                for index in per_pixel..row.len() / 2 {
                    let value = read(row, index).wrapping_add(read(row, index - per_pixel));
                    let bytes = match big_endian {
                        true => value.to_be_bytes(),
                        false => value.to_le_bytes(),
                    };
                    row[2 * index..2 * index + 2].copy_from_slice(&bytes);
                }
            }
            _ => {}
        }
    }
}

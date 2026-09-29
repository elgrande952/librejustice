//! PNG writing over the deflate the crate already links, no encoder crate.

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;
use flate2::Crc;

pub enum Kind<'a> {
    Gray,
    Rgb,
    Rgba,
    /// Colours and, when some entry is not opaque, their alphas.
    Palette(&'a [[u8; 3]], &'a [u8]),
}

impl Kind<'_> {
    fn channels(&self) -> usize {
        match self {
            Kind::Gray | Kind::Palette(..) => 1,
            Kind::Rgb => 3,
            Kind::Rgba => 4,
        }
    }

    fn code(&self) -> u8 {
        match self {
            Kind::Gray => 0,
            Kind::Rgb => 2,
            Kind::Palette(..) => 3,
            Kind::Rgba => 6,
        }
    }
}

/// Samples packed big-endian within a byte, each row starting on a byte.
pub fn encode(width: u32, height: u32, depth: u8, kind: Kind<'_>, rows: &[u8]) -> Vec<u8> {
    let channels = kind.channels();
    let stride = (width as usize * channels * depth as usize).div_ceil(8);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[depth, kind.code(), 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    if let Kind::Palette(colors, alphas) = kind {
        chunk(&mut out, b"PLTE", colors.as_flattened());
        if alphas.iter().any(|&alpha| alpha != 255) {
            chunk(&mut out, b"tRNS", alphas);
        }
    }
    let paeth = depth == 8 && !matches!(kind, Kind::Palette(..));
    let mut encoder = ZlibEncoder::new(Vec::with_capacity(rows.len() / 4), Compression::fast());
    let mut line = vec![0u8; stride + 1];
    let zero = vec![0u8; stride];
    let mut previous: &[u8] = &zero;
    for row in rows.chunks_exact(stride).take(height as usize) {
        if paeth {
            line[0] = 4;
            let first = channels;
            for index in 0..first {
                line[index + 1] = row[index].wrapping_sub(predict(0, previous[index], 0));
            }
            for index in first..stride {
                let back = index - channels;
                let guess = predict(row[back], previous[index], previous[back]);
                line[index + 1] = row[index].wrapping_sub(guess);
            }
        } else {
            line[0] = 0;
            line[1..].copy_from_slice(row);
        }
        let _ = encoder.write_all(&line);
        previous = row;
    }
    let data = encoder.finish().unwrap_or_default();
    chunk(&mut out, b"IDAT", &data);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn predict(left: u8, up: u8, corner: u8) -> u8 {
    let (left16, up16, corner16) = (i16::from(left), i16::from(up), i16::from(corner));
    let to_left = (up16 - corner16).abs();
    let to_up = (left16 - corner16).abs();
    let to_corner = (left16 + up16 - 2 * corner16).abs();
    if to_left <= to_up && to_left <= to_corner {
        left
    } else if to_up <= to_corner {
        up
    } else {
        corner
    }
}

fn chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(data);
    let mut crc = Crc::new();
    crc.update(name);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

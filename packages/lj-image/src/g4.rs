//! CCITT Group 4 writing (T.6), the lossless code of a bilevel scan.

/// Terminating codes, run 0 to 63, as (length, code).
#[rustfmt::skip]
const WHITE_TERMINATING: [(u8, u16); 64] = [
    (8, 0x35), (6, 0x07), (4, 0x07), (4, 0x08), (4, 0x0B), (4, 0x0C), (4, 0x0E), (4, 0x0F),
    (5, 0x13), (5, 0x14), (5, 0x07), (5, 0x08), (6, 0x08), (6, 0x03), (6, 0x34), (6, 0x35),
    (6, 0x2A), (6, 0x2B), (7, 0x27), (7, 0x0C), (7, 0x08), (7, 0x17), (7, 0x03), (7, 0x04),
    (7, 0x28), (7, 0x2B), (7, 0x13), (7, 0x24), (7, 0x18), (8, 0x02), (8, 0x03), (8, 0x1A),
    (8, 0x1B), (8, 0x12), (8, 0x13), (8, 0x14), (8, 0x15), (8, 0x16), (8, 0x17), (8, 0x28),
    (8, 0x29), (8, 0x2A), (8, 0x2B), (8, 0x2C), (8, 0x2D), (8, 0x04), (8, 0x05), (8, 0x0A),
    (8, 0x0B), (8, 0x52), (8, 0x53), (8, 0x54), (8, 0x55), (8, 0x24), (8, 0x25), (8, 0x58),
    (8, 0x59), (8, 0x5A), (8, 0x5B), (8, 0x4A), (8, 0x4B), (8, 0x32), (8, 0x33), (8, 0x34),
];

#[rustfmt::skip]
const BLACK_TERMINATING: [(u8, u16); 64] = [
    (10, 0x37), (3, 0x02), (2, 0x03), (2, 0x02), (3, 0x03), (4, 0x03), (4, 0x02), (5, 0x03),
    (6, 0x05), (6, 0x04), (7, 0x04), (7, 0x05), (7, 0x07), (8, 0x04), (8, 0x07), (9, 0x18),
    (10, 0x17), (10, 0x18), (10, 0x08), (11, 0x67), (11, 0x68), (11, 0x6C), (11, 0x37), (11, 0x28),
    (11, 0x17), (11, 0x18), (12, 0xCA), (12, 0xCB), (12, 0xCC), (12, 0xCD), (12, 0x68), (12, 0x69),
    (12, 0x6A), (12, 0x6B), (12, 0xD2), (12, 0xD3), (12, 0xD4), (12, 0xD5), (12, 0xD6), (12, 0xD7),
    (12, 0x6C), (12, 0x6D), (12, 0xDA), (12, 0xDB), (12, 0x54), (12, 0x55), (12, 0x56), (12, 0x57),
    (12, 0x64), (12, 0x65), (12, 0x52), (12, 0x53), (12, 0x24), (12, 0x37), (12, 0x38), (12, 0x27),
    (12, 0x28), (12, 0x58), (12, 0x59), (12, 0x2B), (12, 0x2C), (12, 0x5A), (12, 0x66), (12, 0x67),
];

/// Make-up codes, run 64 to 1728 by 64.
#[rustfmt::skip]
const WHITE_MAKEUP: [(u8, u16); 27] = [
    (5, 0x1B), (5, 0x12), (6, 0x17), (7, 0x37), (8, 0x36), (8, 0x37), (8, 0x64), (8, 0x65),
    (8, 0x68), (8, 0x67), (9, 0xCC), (9, 0xCD), (9, 0xD2), (9, 0xD3), (9, 0xD4), (9, 0xD5),
    (9, 0xD6), (9, 0xD7), (9, 0xD8), (9, 0xD9), (9, 0xDA), (9, 0xDB), (9, 0x98), (9, 0x99),
    (9, 0x9A), (6, 0x18), (9, 0x9B),
];

#[rustfmt::skip]
const BLACK_MAKEUP: [(u8, u16); 27] = [
    (10, 0x0F), (12, 0xC8), (12, 0xC9), (12, 0x5B), (12, 0x33), (12, 0x34), (12, 0x35), (13, 0x6C),
    (13, 0x6D), (13, 0x4A), (13, 0x4B), (13, 0x4C), (13, 0x4D), (13, 0x72), (13, 0x73), (13, 0x74),
    (13, 0x75), (13, 0x76), (13, 0x77), (13, 0x52), (13, 0x53), (13, 0x54), (13, 0x55), (13, 0x5A),
    (13, 0x5B), (13, 0x64), (13, 0x65),
];

/// Make-up codes both colours share, run 1792 to 2560 by 64.
#[rustfmt::skip]
const COMMON_MAKEUP: [(u8, u16); 13] = [
    (11, 0x08), (11, 0x0C), (11, 0x0D), (12, 0x12), (12, 0x13), (12, 0x14), (12, 0x15), (12, 0x16),
    (12, 0x17), (12, 0x1C), (12, 0x1D), (12, 0x1E), (12, 0x1F),
];

/// Vertical mode codes, a1 - b1 from -3 to 3.
#[rustfmt::skip]
const VERTICAL: [(u8, u16); 7] = [
    (7, 0x02), (6, 0x02), (3, 0x02), (1, 0x01), (3, 0x03), (6, 0x03), (7, 0x03),
];

struct Bits {
    out: Vec<u8>,
    acc: u32,
    used: u32,
}

impl Bits {
    fn put(&mut self, (length, code): (u8, u16)) {
        self.acc = (self.acc << length) | u32::from(code);
        self.used += u32::from(length);
        while self.used >= 8 {
            self.used -= 8;
            self.out.push((self.acc >> self.used) as u8);
        }
        self.acc &= (1 << self.used) - 1;
    }

    fn run(&mut self, mut length: usize, black: bool) {
        let (terminating, makeup) = match black {
            false => (&WHITE_TERMINATING, &WHITE_MAKEUP),
            true => (&BLACK_TERMINATING, &BLACK_MAKEUP),
        };
        while length >= 2560 {
            self.put(COMMON_MAKEUP[12]);
            length -= 2560;
        }
        if length >= 1792 {
            self.put(COMMON_MAKEUP[length / 64 - 28]);
            length %= 64;
        } else if length >= 64 {
            self.put(makeup[length / 64 - 1]);
            length %= 64;
        }
        self.put(terminating[length]);
    }
}

/// Where the colour flips along a row, white first, then `width` twice.
fn changes(row: &[u8], width: usize, out: &mut Vec<usize>) {
    out.clear();
    let mut white = true;
    let mut x = 0;
    while x < width {
        let byte = row[x / 8];
        let other = if white { !byte } else { byte } << (x % 8);
        if other == 0 {
            x += 8 - x % 8;
            continue;
        }
        x += other.leading_zeros() as usize;
        if x < width {
            out.push(x);
            white = !white;
        }
    }
    out.extend([width, width]);
}

/// `rows` packed one bit per pixel, 1 for white, each row starting on a
/// byte, written as Group 4 (K < 0) ending on the end-of-block code.
pub fn encode(rows: &[u8], width: u32, height: u32) -> Vec<u8> {
    let width = width as usize;
    let stride = width.div_ceil(8);
    let mut bits = Bits {
        out: Vec::with_capacity(rows.len() / 8),
        acc: 0,
        used: 0,
    };
    let (mut reference, mut coding) = (vec![width, width], Vec::new());
    for row in rows.chunks_exact(stride).take(height as usize) {
        changes(row, width, &mut coding);
        let mut a0: isize = -1;
        let mut black = false;
        let (mut i, mut j) = (0, 0);
        loop {
            while coding[i] as isize <= a0 && coding[i] < width {
                i += 1;
            }
            let a1 = coding[i];
            // b1: the first change past a0 on the reference row that turns
            // to the colour opposite a0's; changes at even indexes turn black.
            while reference[j] as isize <= a0 && reference[j] < width {
                j += 1;
            }
            let b = match j % 2 == usize::from(black) || reference[j] == width {
                true => j,
                false => j + 1,
            };
            let b1 = reference[b];
            let b2 = reference[(b + 1).min(reference.len() - 1)];
            if b2 < a1 {
                bits.put((4, 0x01));
                a0 = b2 as isize;
            } else if a1.abs_diff(b1) <= 3 {
                bits.put(VERTICAL[(a1 as isize - b1 as isize + 3) as usize]);
                a0 = a1 as isize;
                black = !black;
            } else {
                let a2 = coding[(i + 1).min(coding.len() - 1)];
                bits.put((3, 0x01));
                bits.run(a1 - a0.max(0) as usize, black);
                bits.run(a2 - a1, !black);
                a0 = a2 as isize;
            }
            if a0 as usize >= width {
                break;
            }
        }
        std::mem::swap(&mut reference, &mut coding);
    }
    bits.put((12, 0x001));
    bits.put((12, 0x001));
    if bits.used > 0 {
        bits.put((8 - bits.used as u8, 0));
    }
    bits.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bilevel::{ccitt, CcittSettings, EncodingMode};

    fn round_trip(rows: &[u8], width: u32, height: u32) {
        let encoded = encode(rows, width, height);
        let settings = CcittSettings {
            columns: width,
            rows: height,
            end_of_block: true,
            end_of_line: false,
            rows_are_byte_aligned: false,
            encoding: EncodingMode::Group4,
            invert_black: false,
        };
        let decoded = ccitt(&encoded, settings).expect("decodes");
        assert_eq!(decoded.rows_written(), height as usize);
        let stride = (width as usize).div_ceil(8);
        let pad = (8 - width % 8) % 8;
        let mask = (0xFFu16 << pad) as u8;
        for (y, (a, b)) in rows
            .chunks(stride)
            .zip(decoded.data.chunks(stride))
            .enumerate()
        {
            assert_eq!(a[..stride - 1], b[..stride - 1], "row {y}");
            assert_eq!(a[stride - 1] & mask, b[stride - 1] & mask, "row {y}");
        }
    }

    #[test]
    fn every_row_decodes_back_to_the_same_bits() {
        let mut state = 0x2545F491u32;
        let mut random = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for (width, height) in [(1, 1), (7, 3), (64, 48), (203, 57), (3000, 9)] {
            let stride = (width as usize).div_ceil(8);
            let blank = vec![0xFFu8; stride * height as usize];
            let black = vec![0x00u8; stride * height as usize];
            let noise: Vec<u8> = (0..blank.len()).map(|_| random() as u8).collect();
            // Text-like rows: long white runs broken by short strokes, which
            // exercise pass and vertical modes against the row above.
            let mut strokes = blank.clone();
            for y in 0..height as usize {
                for x in 0..width as usize {
                    if (x / 3 + y / 2) % 7 == 0 || random() % 97 == 0 {
                        strokes[y * stride + x / 8] &= !(0x80 >> (x % 8));
                    }
                }
            }
            for rows in [blank, black, noise, strokes] {
                round_trip(&rows, width, height);
            }
        }
    }
}

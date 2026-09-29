//! LZW as PDF and TIFF write it.

/// Decode an LZW-encoded byte run (PDF § 7.4.4, TIFF 6.0 § 13): 9–12-bit
/// codes, 256 = CLEAR, 257 = EOD, the literal dictionary seeds indices
/// 0–255, and growth starts at 258. `early_change`, as TIFF writes and PDF
/// defaults to, switches to a wider code one entry before the dictionary is
/// fully populated at the current width. Refuses to write more than `max`
/// bytes.
pub fn decode(data: &[u8], early_change: bool, max: usize) -> Result<Vec<u8>, String> {
    const CLEAR: usize = 256;
    const EOD: usize = 257;
    const MAX_WIDTH: u32 = 12;
    const SIZE: usize = 1 << MAX_WIDTH;
    let width_threshold = |width: u32| {
        if early_change {
            (1usize << width) - 1
        } else {
            1usize << width
        }
    };

    // Each entry is its prefix entry plus one last byte, written out by
    // walking the prefixes back from the end.
    let mut prefix = vec![0u16; SIZE];
    let mut last = vec![0u8; SIZE];
    let mut first = vec![0u8; SIZE];
    let mut length = vec![0usize; SIZE];
    for byte in 0..256 {
        last[byte] = byte as u8;
        first[byte] = byte as u8;
        length[byte] = 1;
    }

    let mut reader = BitReader::new(data);
    let mut output: Vec<u8> = Vec::new();
    let mut code_width: u32 = 9;
    let mut next = EOD + 1;
    let mut previous: Option<usize> = None;
    while let Some(code) = reader.read_bits(code_width) {
        let code = code as usize;
        if code == EOD {
            break;
        }
        if code == CLEAR {
            next = EOD + 1;
            code_width = 9;
            previous = None;
            continue;
        }
        // The K+K[0] case: the code names the entry about to be added.
        let (lead, len) = match (code.cmp(&next), previous) {
            (std::cmp::Ordering::Less, _) => (first[code], length[code]),
            (std::cmp::Ordering::Equal, Some(prev)) => (first[prev], length[prev] + 1),
            (std::cmp::Ordering::Equal, None) => return Err("LZW code out of sequence".into()),
            (std::cmp::Ordering::Greater, _) => {
                return Err(format!("LZW code {code} outside dictionary"));
            }
        };
        if output.len() + len > max {
            return Err("decompressed stream exceeds maximum allowed size".to_string());
        }
        if let Some(prev) = previous {
            if next < SIZE {
                prefix[next] = prev as u16;
                last[next] = lead;
                first[next] = first[prev];
                length[next] = length[prev] + 1;
                next += 1;
            }
            // The decoder trails the encoder by one entry: with early change
            // it widens once 511 entries are in, as libtiff and pdf.js do.
            if next >= width_threshold(code_width) && code_width < MAX_WIDTH {
                code_width += 1;
            }
        }
        let start = output.len();
        output.resize(start + len, 0);
        let mut at = code;
        for byte in output[start..].iter_mut().rev() {
            *byte = last[at];
            at = usize::from(prefix[at]);
        }
        previous = Some(code);
    }
    Ok(output)
}

/// MSB-first bit reader used by the LZW decoder. The PDF spec § 7.4.4.2
/// states codes are packed "with the high-order bit of each code
/// appearing first", so bytes are consumed from the front of the stream
/// and codes are shifted out from the top of an accumulating buffer.
/// When the backing data runs out mid-code, the remaining bits are
/// zero-padded, matching the encoder contract in § 7.4.4.3.
struct BitReader<'a> {
    data: &'a [u8],
    byte_index: usize,
    bit_buffer: u32,
    bit_count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            byte_index: 0,
            bit_buffer: 0,
            bit_count: 0,
        }
    }

    fn read_bits(&mut self, width: u32) -> Option<u32> {
        while self.bit_count < width {
            if self.byte_index >= self.data.len() {
                if self.bit_count == 0 {
                    return None;
                }
                // Pad with zero bits to flush the final partial code.
                let pad = width - self.bit_count;
                self.bit_buffer <<= pad;
                let mask = (1u32 << width) - 1;
                let code = self.bit_buffer & mask;
                self.bit_count = 0;
                self.bit_buffer = 0;
                return Some(code);
            }
            self.bit_buffer = (self.bit_buffer << 8) | u32::from(self.data[self.byte_index]);
            self.byte_index += 1;
            self.bit_count += 8;
        }
        self.bit_count -= width;
        let mask = (1u32 << width) - 1;
        let code = (self.bit_buffer >> self.bit_count) & mask;
        self.bit_buffer &= (1u32 << self.bit_count) - 1;
        Some(code)
    }
}

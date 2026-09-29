//! PackBits, the run-length code of TIFF and QuickDraw.

/// Unpacks until `len` bytes are out or the data ends; 128 is a no-op.
pub fn decode(data: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut at = 0;
    while out.len() < len && at < data.len() {
        let control = data[at];
        at += 1;
        match control {
            0..=127 => {
                let end = (at + usize::from(control) + 1).min(data.len());
                out.extend_from_slice(&data[at..end]);
                at = end;
            }
            128 => {}
            _ => {
                if let Some(&byte) = data.get(at) {
                    out.extend(std::iter::repeat_n(byte, 257 - usize::from(control)));
                }
                at += 1;
            }
        }
    }
    out.truncate(len);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn unpacks_the_tiff_specification_example() {
        let packed = [
            0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A, 0xFD, 0xAA, 0x03, 0x80, 0x00, 0x2A, 0x22, 0xF7,
            0xAA,
        ];
        let unpacked = [
            0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0xAA, 0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0x22,
            0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA,
        ];
        assert_eq!(super::decode(&packed, 24), unpacked);
    }
}

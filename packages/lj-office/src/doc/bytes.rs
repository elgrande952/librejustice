//! Little-endian reads that answer `None` past the end instead of panicking:
//! every offset in a `.doc` comes from the file itself.

pub fn u8_at(b: &[u8], at: usize) -> Option<u8> {
    b.get(at).copied()
}

pub fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

pub fn i16_at(b: &[u8], at: usize) -> Option<i16> {
    u16_at(b, at).map(|v| v as i16)
}

pub fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    u32_at(b, at).map(|v| v as i32)
}

/// `len` bytes at `at`, or `None` when the range leaves `b`.
pub fn slice(b: &[u8], at: usize, len: usize) -> Option<&[u8]> {
    b.get(at..at.checked_add(len)?)
}

/// A PLC: `n + 1` u32 positions followed by `n` records of `cb` bytes, `n`
/// derived from the byte length as the format prescribes.
pub struct Plc<'a> {
    data: &'a [u8],
    cb: usize,
    pub n: usize,
}

impl<'a> Plc<'a> {
    pub fn new(data: &'a [u8], cb: usize) -> Self {
        let n = if data.len() < 4 {
            0
        } else {
            (data.len() - 4) / (4 + cb)
        };
        Self { data, cb, n }
    }

    pub fn pos(&self, i: usize) -> u32 {
        u32_at(self.data, i * 4).unwrap_or(0)
    }

    pub fn data(&self, i: usize) -> &'a [u8] {
        let at = (self.n + 1) * 4 + i * self.cb;
        &self.data[at..at + self.cb]
    }
}

/// UTF-16LE code units to a `String`, lone surrogates replaced.
pub fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
}

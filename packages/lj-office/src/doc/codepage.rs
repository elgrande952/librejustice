//! Single-byte code pages, as the 8-bit text of a `.doc` needs them.

use encoding_rs::Encoding;

/// A byte-to-UTF-16 table for one Windows code page.
#[derive(Clone, Copy)]
pub struct Codepage {
    table: [u16; 256],
}

impl Codepage {
    pub fn windows(number: u16) -> Self {
        let label = match number {
            874 => "windows-874",
            932 => "shift_jis",
            936 => "gbk",
            949 => "euc-kr",
            950 => "big5",
            1250..=1258 => return Self::from_encoding(windows_125x(number)),
            _ => "windows-1252",
        };
        Self::from_encoding(
            Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252),
        )
    }

    fn from_encoding(encoding: &'static Encoding) -> Self {
        let mut table = [0u16; 256];
        for (b, slot) in table.iter_mut().enumerate() {
            let byte = [b as u8];
            let (text, _) = encoding.decode_without_bom_handling(&byte);
            *slot = text.encode_utf16().next().unwrap_or(0xFFFD);
        }
        Self { table }
    }

    /// The code page Word 6 and 95 wrote 8-bit text in, from the document's
    /// language.
    pub fn for_lid(lid: u16) -> Self {
        Self::windows(Self::number_for_lid(lid))
    }

    pub fn number_for_lid(lid: u16) -> u16 {
        match lid & 0x3FF {
            0x02 | 0x19 | 0x22 | 0x23 | 0x2F | 0x3F | 0x40 | 0x43 | 0x44 | 0x50 => 1251,
            0x1A if matches!(lid, 0x0C1A | 0x1C1A) => 1251,
            0x05 | 0x0E | 0x15 | 0x18 | 0x1A | 0x1B | 0x1C | 0x24 => 1250,
            0x08 => 1253,
            0x1F | 0x2C => 1254,
            0x0D => 1255,
            0x01 | 0x20 | 0x29 => 1256,
            0x25..=0x27 => 1257,
            0x2A => 1258,
            0x1E => 874,
            0x11 => 932,
            0x04 if lid == 0x0404 || lid == 0x0C04 || lid == 0x1404 => 950,
            0x04 => 936,
            0x12 => 949,
            _ => 1252,
        }
    }

    pub fn decode(&self, b: u8) -> u16 {
        self.table[b as usize]
    }

    /// The byte that decodes to `unit`, if this code page has one.
    pub fn encode(&self, unit: u16) -> Option<u8> {
        self.table.iter().position(|u| *u == unit).map(|b| b as u8)
    }
}

fn windows_125x(number: u16) -> &'static Encoding {
    match number {
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        _ => encoding_rs::WINDOWS_1252,
    }
}

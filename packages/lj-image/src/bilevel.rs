//! One bit per pixel: the CCITT fax codes, and the rows any bilevel codec
//! writes into.

pub use hayro_ccitt::{DecodeSettings as CcittSettings, EncodingMode};

/// Rows of bits, 1 for white, each starting on a byte; the rows a codec
/// leaves out stay white.
pub struct Bilevel {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    stride: usize,
    row: usize,
    column: usize,
}

impl Bilevel {
    pub fn new(width: u32, height: u32) -> Bilevel {
        let stride = (width as usize).div_ceil(8);
        Bilevel {
            width,
            height,
            data: vec![0xFF; stride * height as usize],
            stride,
            row: 0,
            column: 0,
        }
    }

    /// How many rows the codec reached.
    pub fn rows_written(&self) -> usize {
        self.row
    }

    pub fn push(&mut self, white: bool) {
        if self.row < self.height as usize && self.column < self.width as usize {
            if !white {
                self.data[self.row * self.stride + self.column / 8] &= !(0x80 >> (self.column % 8));
            }
            self.column += 1;
        }
    }

    /// `count` whole bytes of one colour, from a column on a byte.
    pub fn push_bytes(&mut self, white: bool, count: u32) {
        if self.row >= self.height as usize {
            return;
        }
        let start = self.row * self.stride + self.column / 8;
        let end = (start + count as usize).min((self.row + 1) * self.stride);
        if !white {
            self.data[start..end].fill(0);
        }
        self.column = (self.column + 8 * count as usize).min(self.width as usize);
    }

    pub fn next_line(&mut self) {
        self.row += 1;
        self.column = 0;
    }
}

impl hayro_ccitt::Decoder for Bilevel {
    fn push_pixel(&mut self, white: bool) {
        self.push(white);
    }

    fn push_pixel_chunk(&mut self, white: bool, chunk_count: u32) {
        self.push_bytes(white, chunk_count);
    }

    fn next_line(&mut self) {
        Bilevel::next_line(self);
    }
}

/// The rows a fax stream decodes to, as far as it decodes; `None` when not
/// one row does.
pub fn ccitt(bytes: &[u8], settings: CcittSettings) -> Option<Bilevel> {
    let mut rows = Bilevel::new(settings.columns, settings.rows);
    let mut context = hayro_ccitt::DecoderContext::new(settings);
    let result = hayro_ccitt::decode(bytes, &mut rows, &mut context);
    (result.is_ok() || rows.row > 0).then_some(rows)
}

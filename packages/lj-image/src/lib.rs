//! Raster codecs the Word and PDF engines share: what a browser does not
//! read is decoded here and written as PNG.

pub mod bilevel;
pub mod g4;
pub mod jpeg;
pub mod lzw;
pub mod packbits;
pub mod png;
pub mod tiff;

/// Above this many pixels an image is refused rather than decoded: a 600 dpi
/// A4 scan has 35 million.
pub const MAX_PIXELS: u64 = 1 << 26;

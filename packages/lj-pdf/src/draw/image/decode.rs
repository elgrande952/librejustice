//! From an image stream to its samples: the general filters, then the codec.

use crate::objects::{
    PdfDictionary, PdfError, PdfFile, PdfObject, PdfResult, PdfStream, PdfValue, decode_stream,
};

use lj_image::MAX_PIXELS;
use lj_image::bilevel::{self, Bilevel, CcittSettings, EncodingMode};

/// A JPEG 2000 image above this many pixels is decoded at a lower resolution
/// level, as its codestream allows.
const REDUCED_PIXELS: u64 = 1 << 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Codec {
    Dct,
    Jpx,
    Jbig2,
    Ccitt,
}

/// The stream once the general filters are undone, and the codec left to run.
pub(crate) struct Encoded {
    pub bytes: Vec<u8>,
    pub codec: Option<(Codec, PdfDictionary)>,
}

/// Samples packed as PDF packs them, rows starting on a byte.
pub(crate) struct Samples {
    pub width: u32,
    pub height: u32,
    pub bits: u8,
    pub components: usize,
    pub data: Vec<u8>,
    /// One byte per pixel, from a JPEG 2000 image that carries its own.
    pub alpha: Option<Vec<u8>>,
}

impl Samples {
    /// A row's bits can pass 2^32 where its bytes do not.
    pub(crate) fn stride(&self) -> usize {
        (u64::from(self.width) * self.components as u64 * u64::from(self.bits)).div_ceil(8) as usize
    }
}

pub(crate) fn check_size(width: u32, height: u32) -> PdfResult<()> {
    if width == 0 || height == 0 {
        return Err(PdfError::Corrupt("image without pixels".into()));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(PdfError::Unsupported(format!(
            "image of {width}x{height} pixels is over the limit"
        )));
    }
    Ok(())
}

pub(crate) fn encoded(file: &PdfFile, dict: &PdfDictionary, data: &[u8]) -> PdfResult<Encoded> {
    let resolve = |value: &PdfValue| file.resolve(value).cloned().unwrap_or(PdfValue::Null);
    let filters: Vec<PdfValue> = match dict.get("Filter").map(resolve) {
        None | Some(PdfValue::Null) => Vec::new(),
        Some(PdfValue::Array(items)) => items.iter().map(resolve).collect(),
        Some(single) => vec![single],
    };
    let parms: Vec<PdfValue> = match dict.get("DecodeParms").map(resolve) {
        Some(PdfValue::Array(items)) => items.iter().map(resolve).collect(),
        Some(single) => vec![single],
        None => Vec::new(),
    };
    let codec = filters
        .last()
        .and_then(PdfValue::as_name)
        .and_then(|name| match name {
            "DCTDecode" | "DCT" => Some(Codec::Dct),
            "JPXDecode" => Some(Codec::Jpx),
            "JBIG2Decode" => Some(Codec::Jbig2),
            "CCITTFaxDecode" | "CCF" => Some(Codec::Ccitt),
            _ => None,
        });
    let general = filters.len() - usize::from(codec.is_some());
    let bytes = if general == 0 {
        data.to_vec()
    } else {
        let mut stream_dict = PdfDictionary::new();
        stream_dict.insert(
            "Filter".into(),
            PdfValue::Array(filters[..general].to_vec()),
        );
        let parms_of = (0..general)
            .map(|index| parms.get(index).cloned().unwrap_or(PdfValue::Null))
            .collect();
        stream_dict.insert("DecodeParms".into(), PdfValue::Array(parms_of));
        decode_stream(&PdfStream {
            dict: stream_dict,
            data: data.to_vec(),
        })?
    };
    let codec = codec.map(|codec| {
        // `/Columns 20 0 R`: a value behind a reference is a value all the same;
        // a stream behind one, as `JBIG2Globals`, stays a reference.
        let parms = match parms.get(general) {
            Some(PdfValue::Dictionary(parms)) => parms
                .iter()
                .map(|(key, value)| {
                    let value = file
                        .resolve(value)
                        .cloned()
                        .unwrap_or_else(|_| value.clone());
                    (key.clone(), value)
                })
                .collect(),
            _ => PdfDictionary::new(),
        };
        (codec, parms)
    });
    // Readers skip what a writer left before a JPEG's start of image.
    let start = match &codec {
        Some((Codec::Dct, _)) => bytes.windows(2).position(|pair| pair == [0xFF, 0xD8]),
        _ => None,
    };
    let bytes = match start {
        Some(start) if start > 0 => bytes[start..].to_vec(),
        _ => bytes,
    };
    Ok(Encoded { bytes, codec })
}

/// Raw samples, cut or padded with zeros to the size the image declares, as
/// readers do with a stream that ends early.
pub(crate) fn raw(
    mut bytes: Vec<u8>,
    width: u32,
    height: u32,
    bits: u8,
    components: usize,
) -> PdfResult<Samples> {
    // The largest image: MAX_PIXELS pixels of eight samples.
    if u64::from(width) * components as u64 * u64::from(height) > MAX_PIXELS * 8 {
        return Err(PdfError::Unsupported(format!(
            "image of {width}x{height} pixels of {components} components is over the limit"
        )));
    }
    let mut samples = Samples {
        width,
        height,
        bits,
        components,
        data: Vec::new(),
        alpha: None,
    };
    bytes.resize(samples.stride() * height as usize, 0);
    samples.data = bytes;
    Ok(samples)
}

pub(crate) struct JpegHeader {
    pub components: u8,
    pub adobe: bool,
}

/// Walks the markers up to the frame header.
pub(crate) fn jpeg_header(bytes: &[u8]) -> Option<JpegHeader> {
    let mut position = 2;
    let mut adobe = false;
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    loop {
        while *bytes.get(position)? != 0xFF {
            position += 1;
        }
        while *bytes.get(position)? == 0xFF {
            position += 1;
        }
        let marker = *bytes.get(position)?;
        position += 1;
        if matches!(marker, 0xD0..=0xD7 | 0x01) {
            continue;
        }
        let length = usize::from(u16::from_be_bytes([
            *bytes.get(position)?,
            *bytes.get(position + 1)?,
        ]));
        let segment = bytes.get(position + 2..position + length)?;
        match marker {
            0xEE if segment.starts_with(b"Adobe") => adobe = true,
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                return Some(JpegHeader {
                    components: *segment.get(5)?,
                    adobe,
                });
            }
            0xDA | 0xD9 => return None,
            _ => {}
        }
        position += length;
    }
}

/// A 4 component JPEG comes out as the codec stores it: the Decode array is
/// what inverts an Adobe one.
pub(crate) fn dct(bytes: &[u8], parms: &PdfDictionary) -> PdfResult<Samples> {
    let header = jpeg_header(bytes);
    let mut decoder = jpeg_decoder::Decoder::new(bytes);
    decoder
        .read_info()
        .map_err(|error| PdfError::Corrupt(format!("JPEG image: {error}")))?;
    let info = decoder
        .info()
        .ok_or_else(|| PdfError::Corrupt("JPEG image without a frame".into()))?;
    check_size(u32::from(info.width), u32::from(info.height))?;
    let adobe = header.as_ref().is_some_and(|header| header.adobe);
    if !adobe && parms.get("ColorTransform").and_then(PdfValue::as_integer) == Some(0) {
        decoder.set_color_transform(match info.pixel_format {
            jpeg_decoder::PixelFormat::CMYK32 => jpeg_decoder::ColorTransform::CMYK,
            _ => jpeg_decoder::ColorTransform::RGB,
        });
    }
    let mut data = decoder
        .decode()
        .map_err(|error| PdfError::Corrupt(format!("JPEG image: {error}")))?;
    let (components, bits) = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => (1, 8),
        jpeg_decoder::PixelFormat::L16 => (1, 16),
        jpeg_decoder::PixelFormat::RGB24 => (3, 8),
        jpeg_decoder::PixelFormat::CMYK32 => {
            for byte in &mut data {
                *byte = 255 - *byte;
            }
            (4, 8)
        }
    };
    raw(
        data,
        u32::from(info.width),
        u32::from(info.height),
        bits,
        components,
    )
}

/// Eight bit samples in the image's own colour space, or its palette indices
/// when the PDF names an Indexed space.
pub(crate) fn jpx(bytes: &[u8], indexed: bool, keep_alpha: bool) -> PdfResult<Samples> {
    let corrupt = |error| PdfError::Corrupt(format!("JPEG 2000 image: {error}"));
    let mut settings = hayro_jpeg2000::DecodeSettings {
        resolve_palette_indices: !indexed,
        ..Default::default()
    };
    let mut image = hayro_jpeg2000::Image::new(bytes, &settings).map_err(corrupt)?;
    let pixels = u64::from(image.width()) * u64::from(image.height());
    if pixels > REDUCED_PIXELS {
        let scale = (pixels as f64 / REDUCED_PIXELS as f64).sqrt();
        settings.target_resolution = Some((
            (f64::from(image.width()) / scale) as u32,
            (f64::from(image.height()) / scale) as u32,
        ));
        image = hayro_jpeg2000::Image::new(bytes, &settings).map_err(corrupt)?;
    }
    let (width, height) = (image.width(), image.height());
    check_size(width, height)?;
    let components = usize::from(image.color_space().num_channels());
    let has_alpha = image.has_alpha();
    let depth = image.original_bit_depth();
    let mut decoded = image.decode().map_err(corrupt)?;
    // The codec scales every sample to eight bits, palette indices included.
    if indexed && depth < 8 {
        let max = (1u16 << depth) - 1;
        for sample in &mut decoded {
            *sample = ((u16::from(*sample) * max + 127) / 255) as u8;
        }
    }
    let (data, alpha) = if has_alpha {
        let step = components + 1;
        let data = decoded
            .chunks_exact(step)
            .flat_map(|pixel| pixel[..components].iter().copied())
            .collect();
        let alpha = keep_alpha.then(|| {
            decoded
                .chunks_exact(step)
                .map(|pixel| pixel[components])
                .collect()
        });
        (data, alpha)
    } else {
        (decoded, None)
    };
    let mut samples = raw(data, width, height, 8, components)?;
    samples.alpha = alpha;
    Ok(samples)
}

/// One bit per pixel, 0 for black as in DeviceGray.
pub(crate) fn jbig2(file: &PdfFile, bytes: &[u8], parms: &PdfDictionary) -> PdfResult<Samples> {
    let globals = match parms.get("JBIG2Globals") {
        Some(PdfValue::Reference(object_ref)) => match file.get_object(*object_ref)? {
            PdfObject::Stream(stream) => Some(decode_stream(stream)?),
            PdfObject::Value(_) => None,
        },
        _ => None,
    };
    // Some writers embed a whole JBIG2 file, header included.
    let image = match bytes.starts_with(b"\x97JB2\r\n\x1a\n") {
        true => hayro_jbig2::Image::new(bytes),
        false => hayro_jbig2::Image::new_embedded(bytes, globals.as_deref()),
    }
    .map_err(|error| PdfError::Corrupt(format!("JBIG2 image: {error}")))?;
    let (width, height) = (image.width(), image.height());
    check_size(width, height)?;
    let mut rows = Jbig2Rows(Bilevel::new(width, height));
    image
        .decode(&mut rows)
        .map_err(|error| PdfError::Corrupt(format!("JBIG2 image: {error}")))?;
    Ok(bilevel_samples(rows.0))
}

/// One bit per pixel, 0 for black unless BlackIs1 says otherwise.
pub(crate) fn ccitt(bytes: &[u8], parms: &PdfDictionary, height: u32) -> PdfResult<Samples> {
    let integer = |key: &str| parms.get(key).and_then(PdfValue::as_integer);
    let flag = |key: &str, default: bool| {
        parms
            .get(key)
            .and_then(PdfValue::as_bool)
            .unwrap_or(default)
    };
    let k = integer("K").unwrap_or(0);
    let columns = u32::try_from(integer("Columns").unwrap_or(1728))
        .map_err(|_| PdfError::Corrupt("CCITT image with negative columns".into()))?;
    let rows = integer("Rows")
        .and_then(|rows| u32::try_from(rows).ok())
        .filter(|&rows| rows > 0)
        .unwrap_or(height);
    check_size(columns, rows)?;
    let settings = CcittSettings {
        columns,
        rows,
        end_of_block: flag("EndOfBlock", true),
        end_of_line: flag("EndOfLine", false),
        rows_are_byte_aligned: flag("EncodedByteAlign", false),
        encoding: match k {
            ..0 => EncodingMode::Group4,
            0 => EncodingMode::Group3_1D,
            k => EncodingMode::Group3_2D { k: k as u32 },
        },
        invert_black: flag("BlackIs1", false),
    };
    bilevel::ccitt(bytes, settings)
        .map(bilevel_samples)
        .ok_or_else(|| PdfError::Corrupt("CCITT image: no row decodes".into()))
}

fn bilevel_samples(rows: Bilevel) -> Samples {
    Samples {
        width: rows.width,
        height: rows.height,
        bits: 1,
        components: 1,
        data: rows.data,
        alpha: None,
    }
}

struct Jbig2Rows(Bilevel);

impl hayro_jbig2::Decoder for Jbig2Rows {
    fn push_pixel(&mut self, black: bool) {
        self.0.push(!black);
    }

    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        self.0.push_bytes(!black, chunk_count);
    }

    fn next_line(&mut self) {
        self.0.next_line();
    }
}

/// Every sample of every component, row after row, as integers.
pub(crate) fn unpack(samples: &Samples) -> impl Iterator<Item = u16> + '_ {
    let per_row = samples.width as usize * samples.components;
    let bits = samples.bits as usize;
    samples
        .data
        .chunks_exact(samples.stride())
        .flat_map(move |row| {
            (0..per_row).map(move |index| match bits {
                8 => u16::from(row[index]),
                16 => u16::from_be_bytes([row[2 * index], row[2 * index + 1]]),
                _ => {
                    let bit = index * bits;
                    let shift = 8 - bits - bit % 8;
                    u16::from((row[bit / 8] >> shift) & ((1u8 << bits) - 1))
                }
            })
        })
}

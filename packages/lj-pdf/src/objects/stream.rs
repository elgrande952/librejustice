use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::objects::error::{PdfError, PdfResult};
use crate::objects::types::{PdfStream, PdfValue};

/// Compress `data` with FlateDecode (zlib / deflate) at the default
/// compression level. Used by the writer when re-emitting rewritten content
/// streams so the saved PDF does not bloat with plaintext content bytes.
pub fn flate_encode(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|error| PdfError::Corrupt(format!("flate encode failed: {error}")))?;
    encoder
        .finish()
        .map_err(|error| PdfError::Corrupt(format!("flate encode finalize failed: {error}")))
}

pub fn decode_stream(stream: &PdfStream) -> PdfResult<Vec<u8>> {
    let filter_names = normalize_filter_list(stream.dict.get("Filter"))?;
    let parms_of = |index: usize| match stream.dict.get("DecodeParms") {
        Some(PdfValue::Array(items)) => items.get(index),
        single => single,
    };
    let mut decoded = stream.data.clone();
    for (index, filter_name) in filter_names.iter().enumerate() {
        let parms = parms_of(index);
        decoded = match filter_name.as_str() {
            "LZWDecode" | "LZW" => {
                apply_predictor(&lzw_decode(&decoded, lzw_early_change(parms)?)?, parms)?
            }
            "FlateDecode" | "Fl" => apply_predictor(&inflate(&decoded)?, parms)?,
            "BrotliDecode" | "Br" => apply_predictor(&brotli_decode(&decoded)?, parms)?,
            _ => apply_filter(filter_name, &decoded)?,
        };
    }
    Ok(decoded)
}

/// Return the /Filter entry as an ordered list of filter names, whether
/// the source dictionary uses the single-name shorthand or the array
/// form. Empty list means no filters applied (raw data).
fn normalize_filter_list(value: Option<&PdfValue>) -> PdfResult<Vec<String>> {
    match value {
        None => Ok(Vec::new()),
        Some(PdfValue::Null) => Ok(Vec::new()),
        Some(PdfValue::Name(name)) => Ok(vec![name.clone()]),
        Some(PdfValue::Array(items)) => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    PdfValue::Name(name) => names.push(name.clone()),
                    _ => {
                        return Err(PdfError::Corrupt(
                            "stream /Filter array contains a non-name entry".to_string(),
                        ));
                    }
                }
            }
            Ok(names)
        }
        Some(_) => Err(PdfError::Corrupt(
            "stream /Filter is neither a name nor an array of names".to_string(),
        )),
    }
}

fn apply_filter(filter: &str, data: &[u8]) -> PdfResult<Vec<u8>> {
    match filter {
        "FlateDecode" | "Fl" => inflate(data),
        "ASCII85Decode" | "A85" => ascii85_decode(data),
        "ASCIIHexDecode" | "AHx" => ascii_hex_decode(data),
        "LZWDecode" | "LZW" => lzw_decode(data, true),
        "RunLengthDecode" | "RL" => run_length_decode(data),
        other => Err(PdfError::Unsupported(format!(
            "stream filter /{other} is not supported"
        ))),
    }
}

fn lzw_decode(data: &[u8], early_change: bool) -> PdfResult<Vec<u8>> {
    lj_image::lzw::decode(data, early_change, MAX_DECOMPRESSED_SIZE as usize)
        .map_err(PdfError::Corrupt)
}

/// Read the `DecodeParms /EarlyChange` flag for an LZW stream. PDF
/// defaults to `1` when the entry is missing; `0` disables the
/// one-code-early width switch that TIFF-flavoured LZW implementations
/// use. Any other value is rejected as corrupt so we never silently
/// misalign on an unknown flag.
fn lzw_early_change(decode_parms: Option<&PdfValue>) -> PdfResult<bool> {
    let Some(value) = decode_parms else {
        return Ok(true);
    };
    let PdfValue::Dictionary(dict) = value else {
        return Ok(true);
    };
    match dict.get("EarlyChange").and_then(PdfValue::as_integer) {
        None => Ok(true),
        Some(1) => Ok(true),
        Some(0) => Ok(false),
        Some(other) => Err(PdfError::Corrupt(format!(
            "unsupported LZW EarlyChange value {other}"
        ))),
    }
}

/// Maximum decompressed stream size (256 MiB). Prevents decompression bombs from
/// exhausting memory in WASM or native contexts.
const MAX_DECOMPRESSED_SIZE: u64 = 256 * 1024 * 1024;

/// Inflates as far as the data allows: a stream cut short or damaged past
/// some point keeps what came before, and one without its zlib header is
/// read as raw deflate, as readers do.
fn inflate(data: &[u8]) -> PdfResult<Vec<u8>> {
    let zlib = inflate_with(data, true);
    let output = match zlib {
        Ok(output) if !output.is_empty() => output,
        zlib => match inflate_with(data, false) {
            Ok(raw) if !raw.is_empty() => raw,
            _ => zlib.map_err(|error| {
                PdfError::Corrupt(format!("failed to decode flate stream: {error}"))
            })?,
        },
    };
    if output.len() as u64 > MAX_DECOMPRESSED_SIZE {
        return Err(PdfError::Corrupt(
            "decompressed stream exceeds maximum allowed size".to_string(),
        ));
    }
    Ok(output)
}

/// Decode an ASCII85-encoded byte run (PDF § 7.4.3). Whitespace is
/// ignored, `z` expands to four zero bytes, and `~>` terminates the
/// stream; a short final group is padded with `u` and the decoded
/// tail is truncated accordingly.
fn ascii85_decode(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut output = Vec::with_capacity(data.len());
    let mut group = [0u8; 5];
    let mut group_len = 0usize;

    for &byte in data {
        if byte == b'~' {
            break; // `~>` EOD marker; the `>` is allowed to follow or be absent.
        }
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C) {
            continue;
        }
        if byte == b'z' {
            if group_len != 0 {
                return Err(PdfError::Corrupt(
                    "ASCII85 'z' shortcut inside a partial group".to_string(),
                ));
            }
            output.extend_from_slice(&[0u8; 4]);
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            break;
        }
        group[group_len] = byte - b'!';
        group_len += 1;
        if group_len == 5 {
            let value = (group[0] as u64) * 85u64.pow(4)
                + (group[1] as u64) * 85u64.pow(3)
                + (group[2] as u64) * 85u64.pow(2)
                + (group[3] as u64) * 85
                + (group[4] as u64);
            if value > u32::MAX as u64 {
                return Err(PdfError::Corrupt(
                    "ASCII85 group value exceeds 32 bits".to_string(),
                ));
            }
            output.extend_from_slice(&(value as u32).to_be_bytes());
            group_len = 0;
        }
    }

    if group_len > 0 {
        if group_len == 1 {
            return Err(PdfError::Corrupt(
                "ASCII85 final group contains a single byte".to_string(),
            ));
        }
        // Pad with the max digit so truncating yields the right tail.
        for entry in group.iter_mut().skip(group_len) {
            *entry = 84;
        }
        let value = (group[0] as u64) * 85u64.pow(4)
            + (group[1] as u64) * 85u64.pow(3)
            + (group[2] as u64) * 85u64.pow(2)
            + (group[3] as u64) * 85
            + (group[4] as u64);
        let bytes = (value as u32).to_be_bytes();
        output.extend_from_slice(&bytes[..group_len - 1]);
    }

    Ok(output)
}

/// Decode an ASCIIHex-encoded byte run (PDF § 7.4.2). Whitespace is
/// ignored, `>` terminates the stream, and a trailing odd nibble is
/// treated as if followed by `0`.
fn brotli_decode(data: &[u8]) -> PdfResult<Vec<u8>> {
    use std::io::Read;
    let mut decoder =
        brotli_decompressor::Decompressor::new(data, 4096).take(MAX_DECOMPRESSED_SIZE);
    let mut output = Vec::new();
    match decoder.read_to_end(&mut output) {
        Err(error) if output.is_empty() => Err(PdfError::Corrupt(format!(
            "failed to decode brotli stream: {error}"
        ))),
        _ => Ok(output),
    }
}

/// Inflates up to the first error, keeping every byte decoded before it.
fn inflate_with(data: &[u8], zlib_header: bool) -> Result<Vec<u8>, flate2::DecompressError> {
    let mut inflater = flate2::Decompress::new(zlib_header);
    let mut output = Vec::new();
    loop {
        output.reserve(64 * 1024);
        let before = (inflater.total_in(), inflater.total_out());
        let input = &data[(inflater.total_in() as usize).min(data.len())..];
        match inflater.decompress_vec(input, &mut output, flate2::FlushDecompress::None) {
            Ok(flate2::Status::StreamEnd) => return Ok(output),
            Ok(_) if before == (inflater.total_in(), inflater.total_out()) => return Ok(output),
            Ok(_) if output.len() as u64 > MAX_DECOMPRESSED_SIZE => return Ok(output),
            Ok(_) => {}
            Err(_) if !output.is_empty() => return Ok(output),
            Err(error) => return Err(error),
        }
    }
}

fn ascii_hex_decode(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut output = Vec::with_capacity(data.len() / 2 + 1);
    let mut high: Option<u8> = None;
    for &byte in data {
        if byte == b'>' {
            break;
        }
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C) {
            continue;
        }
        let nibble = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => break,
        };
        match high.take() {
            None => high = Some(nibble),
            Some(h) => output.push((h << 4) | nibble),
        }
    }
    if let Some(h) = high {
        output.push(h << 4);
    }
    Ok(output)
}

/// Decode a RunLengthDecode byte run (PDF § 7.4.5). Each control byte `L`
/// either introduces a literal run (`0..=127` → copy `L+1` bytes), a
/// repeated byte (`129..=255` → repeat the next byte `257-L` times), or
/// the end-of-data marker (`128`). A stream that ends before the EOD
/// marker is accepted, some producers omit it, but truncated literal
/// or repeat runs are treated as corruption.
fn run_length_decode(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut output: Vec<u8> = Vec::with_capacity(data.len());
    let mut index = 0usize;
    while index < data.len() {
        let length_byte = data[index];
        index += 1;
        if length_byte == 128 {
            return Ok(output);
        }
        if length_byte < 128 {
            let run_len = usize::from(length_byte) + 1;
            let end = index
                .checked_add(run_len)
                .ok_or_else(|| PdfError::Corrupt("RunLengthDecode index overflow".to_string()))?;
            if end > data.len() {
                return Err(PdfError::Corrupt(
                    "RunLengthDecode literal run runs past end of stream".to_string(),
                ));
            }
            output.extend_from_slice(&data[index..end]);
            index = end;
        } else {
            let repeat = 257usize - usize::from(length_byte);
            if index >= data.len() {
                return Err(PdfError::Corrupt(
                    "RunLengthDecode repeat run is missing its payload byte".to_string(),
                ));
            }
            let byte = data[index];
            index += 1;
            output.extend(std::iter::repeat_n(byte, repeat));
        }
        if output.len() as u64 > MAX_DECOMPRESSED_SIZE {
            return Err(PdfError::Corrupt(
                "decompressed stream exceeds maximum allowed size".to_string(),
            ));
        }
    }
    Ok(output)
}

fn apply_predictor(data: &[u8], decode_parms: Option<&PdfValue>) -> PdfResult<Vec<u8>> {
    let Some(PdfValue::Dictionary(parms)) = decode_parms else {
        return Ok(data.to_vec());
    };

    let predictor = parms
        .get("Predictor")
        .and_then(PdfValue::as_integer)
        .unwrap_or(1);
    match predictor {
        1 => Ok(data.to_vec()),
        2 => tiff_predictor_decode(data, parms),
        10..=15 => png_predictor_decode(data, parms),
        other => Err(PdfError::Unsupported(format!(
            "predictor {other} is not supported"
        ))),
    }
}

/// Columns, Colors and BitsPerComponent of a predictor, within MuPDF's
/// bounds: at most 32 colours, and a row of fewer than 2^31 bits.
fn predictor_parms(
    parms: &crate::objects::types::PdfDictionary,
) -> PdfResult<(usize, usize, usize)> {
    let get = |key: &str, default: i64| {
        parms
            .get(key)
            .and_then(PdfValue::as_integer)
            .unwrap_or(default)
    };
    let (columns, colors, bits) = (
        get("Columns", 1),
        get("Colors", 1),
        get("BitsPerComponent", 8),
    );
    if !(1..=32).contains(&colors) || !(1..=16).contains(&bits) || columns < 1 {
        return Err(PdfError::Corrupt(format!(
            "predictor with {columns} columns, {colors} colours of {bits} bits"
        )));
    }
    if columns >= i64::from(i32::MAX) / (colors * bits) {
        return Err(PdfError::Corrupt(format!(
            "predictor with {columns} columns"
        )));
    }
    Ok((columns as usize, colors as usize, bits as usize))
}

fn tiff_predictor_decode(
    data: &[u8],
    parms: &crate::objects::types::PdfDictionary,
) -> PdfResult<Vec<u8>> {
    let (columns, colors, bits_per_component) = predictor_parms(parms)?;

    if bits_per_component != 8 {
        return Err(PdfError::Unsupported(format!(
            "TIFF predictor with BitsPerComponent {bits_per_component} is not supported"
        )));
    }
    let row_stride = columns * colors;
    if !data.len().is_multiple_of(row_stride) {
        return Err(PdfError::Corrupt(format!(
            "TIFF predictor row length mismatch: data={} stride={row_stride}",
            data.len()
        )));
    }
    let mut output = Vec::with_capacity(data.len());
    for row in data.chunks_exact(row_stride) {
        for (component_index, byte) in row.iter().enumerate() {
            if component_index < colors {
                // First pixel in a row is stored as-is per component.
                output.push(*byte);
            } else {
                let previous = output[output.len() - colors];
                output.push(previous.wrapping_add(*byte));
            }
        }
    }
    Ok(output)
}

fn png_predictor_decode(
    data: &[u8],
    parms: &crate::objects::types::PdfDictionary,
) -> PdfResult<Vec<u8>> {
    let (columns, colors, bits_per_component) = predictor_parms(parms)?;

    if !matches!(bits_per_component, 1 | 2 | 4 | 8 | 16) {
        return Err(PdfError::Unsupported(format!(
            "PNG predictor with BitsPerComponent {bits_per_component} is not supported"
        )));
    }
    let bytes_per_pixel = (colors * bits_per_component).div_ceil(8);
    let row_data_len = (columns * colors * bits_per_component).div_ceil(8);
    let row_stride = row_data_len + 1; // leading filter byte

    if !data.len().is_multiple_of(row_stride) {
        return Err(PdfError::Corrupt(format!(
            "PNG predictor row length mismatch: data={} stride={row_stride}",
            data.len()
        )));
    }
    let row_count = data.len() / row_stride;
    let mut output = Vec::with_capacity(row_count * row_data_len);
    let zeros = vec![0u8; row_data_len];
    let bpp = bytes_per_pixel;
    // Each row is unfiltered in place in `output`, against the row before it
    // there; the first `bpp` bytes of a row have no left neighbour.
    for (index, line) in data.chunks_exact(row_stride).enumerate() {
        let start = output.len();
        output.extend_from_slice(&line[1..]);
        let (done, row) = output.split_at_mut(start);
        let prev = match index {
            0 => &zeros[..],
            _ => &done[start - row_data_len..],
        };
        match line[0] {
            0 => {} // None
            1 => {
                // Sub
                for i in bpp..row_data_len {
                    row[i] = row[i].wrapping_add(row[i - bpp]);
                }
            }
            2 => {
                // Up
                for (value, up) in row.iter_mut().zip(prev) {
                    *value = value.wrapping_add(*up);
                }
            }
            3 => {
                // Average
                for i in 0..bpp {
                    row[i] = row[i].wrapping_add(prev[i] / 2);
                }
                for i in bpp..row_data_len {
                    let avg = ((u16::from(row[i - bpp]) + u16::from(prev[i])) / 2) as u8;
                    row[i] = row[i].wrapping_add(avg);
                }
            }
            4 => {
                // Paeth, whose predictor with no left neighbour is the byte above.
                for i in 0..bpp {
                    row[i] = row[i].wrapping_add(prev[i]);
                }
                for i in bpp..row_data_len {
                    row[i] = row[i].wrapping_add(paeth(row[i - bpp], prev[i], prev[i - bpp]));
                }
            }
            other => {
                return Err(PdfError::Corrupt(format!(
                    "unknown PNG row filter type {other}"
                )));
            }
        }
    }

    Ok(output)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a16, b16, c16) = (i16::from(a), i16::from(b), i16::from(c));
    let pa = (b16 - c16).abs();
    let pb = (a16 - c16).abs();
    let pc = (a16 + b16 - 2 * c16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::types::{PdfDictionary, PdfStream, PdfValue};
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    fn make_stream(dict: PdfDictionary, data: Vec<u8>) -> PdfStream {
        PdfStream { dict, data }
    }

    #[test]
    fn passthrough_when_no_filter() {
        let dict = PdfDictionary::new();
        let stream = make_stream(dict, vec![1, 2, 3]);
        assert_eq!(decode_stream(&stream).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn inflates_flate_decode() {
        let raw = b"hello world";
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(raw).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("FlateDecode".into()));
        let stream = make_stream(dict, compressed);
        assert_eq!(decode_stream(&stream).unwrap(), raw.to_vec());
    }

    #[test]
    fn applies_png_up_predictor() {
        // Original 2 rows of 4 bytes each.
        let original: [u8; 8] = [10, 20, 30, 40, 15, 22, 33, 44];

        // Encode with filter type 2 (Up) on row 2, type 0 on row 1.
        let mut encoded = Vec::new();
        encoded.push(0); // row 0: None
        encoded.extend_from_slice(&original[0..4]);
        encoded.push(2); // row 1: Up
        let diff: Vec<u8> = original[4..8]
            .iter()
            .zip(original[0..4].iter())
            .map(|(v, up)| v.wrapping_sub(*up))
            .collect();
        encoded.extend_from_slice(&diff);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&encoded).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("FlateDecode".into()));
        let mut parms = PdfDictionary::new();
        parms.insert("Predictor".to_string(), PdfValue::Integer(12));
        parms.insert("Columns".to_string(), PdfValue::Integer(4));
        dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));

        let stream = make_stream(dict, compressed);
        let decoded = decode_stream(&stream).expect("decode");
        assert_eq!(decoded, original.to_vec());
    }

    #[test]
    fn applies_tiff_predictor() {
        // Original 2 rows of 4 bytes each, 1 color, 8 bits per component.
        let original: [u8; 8] = [10, 20, 30, 40, 15, 22, 33, 44];

        // TIFF predictor encodes each row independently: first byte as-is,
        // subsequent bytes as (current - previous). No filter byte prefix.
        let mut encoded = Vec::new();
        for row in original.chunks(4) {
            encoded.push(row[0]);
            for index in 1..row.len() {
                encoded.push(row[index].wrapping_sub(row[index - 1]));
            }
        }

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&encoded).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("FlateDecode".into()));
        let mut parms = PdfDictionary::new();
        parms.insert("Predictor".to_string(), PdfValue::Integer(2));
        parms.insert("Columns".to_string(), PdfValue::Integer(4));
        dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));

        let stream = make_stream(dict, compressed);
        let decoded = decode_stream(&stream).expect("decode");
        assert_eq!(decoded, original.to_vec());
    }

    #[test]
    fn decodes_ascii85_full_group() {
        // Full 4-byte group "Man " → ASCII85 "9jqo^".
        let encoded = b"9jqo^~>".to_vec();
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("ASCII85Decode".into()));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"Man ".to_vec());
    }

    #[test]
    fn decodes_ascii85_z_shortcut() {
        let encoded = b"z~>".to_vec();
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("ASCII85Decode".into()));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), vec![0, 0, 0, 0]);
    }

    #[test]
    fn decodes_filter_chain_ascii85_then_flate() {
        // Encode plaintext with FlateDecode first, then ASCII85 wrap. The
        // order the filter list uses is the DECODE order, so reading the
        // stream applies ASCII85 first and FlateDecode second, the same
        // order we use to produce the bytes in reverse.
        let plaintext = b"PdfStreamFilterChainTest".to_vec();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&plaintext).unwrap();
        let flate_bytes = encoder.finish().unwrap();

        // ASCII85 encode the FlateDecode payload.
        let mut ascii85 = String::new();
        for chunk in flate_bytes.chunks(4) {
            let mut buf = [0u8; 4];
            buf[..chunk.len()].copy_from_slice(chunk);
            let value = u32::from_be_bytes(buf);
            if chunk.len() == 4 && value == 0 {
                ascii85.push('z');
                continue;
            }
            let mut digits = [0u8; 5];
            let mut v = value as u64;
            for i in (0..5).rev() {
                digits[i] = (v % 85) as u8 + b'!';
                v /= 85;
            }
            let take = chunk.len() + 1;
            for &digit in &digits[..take] {
                ascii85.push(digit as char);
            }
        }
        ascii85.push_str("~>");

        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Array(vec![
                PdfValue::Name("ASCII85Decode".into()),
                PdfValue::Name("FlateDecode".into()),
            ]),
        );
        let stream = make_stream(dict, ascii85.into_bytes());
        assert_eq!(decode_stream(&stream).unwrap(), plaintext);
    }

    #[test]
    fn truncated_flate_keeps_what_came_before_and_raw_deflate_reads() {
        use flate2::{Compression, write::DeflateEncoder};
        use std::io::Write;
        let text: Vec<u8> = (0..200_000u32).map(|i| b'a' + (i * 7 % 26) as u8).collect();
        let mut flate = PdfDictionary::new();
        flate.insert("Filter".to_string(), PdfValue::Name("FlateDecode".into()));
        let whole = super::flate_encode(&text).unwrap();
        assert_eq!(
            decode_stream(&make_stream(flate.clone(), whole.clone())).unwrap(),
            text
        );
        let cut = decode_stream(&make_stream(
            flate.clone(),
            whole[..whole.len() / 2].to_vec(),
        ))
        .unwrap();
        assert!(!cut.is_empty() && text.starts_with(&cut));
        let mut raw = DeflateEncoder::new(Vec::new(), Compression::default());
        raw.write_all(b"raw deflate").unwrap();
        let raw = raw.finish().unwrap();
        assert_eq!(
            decode_stream(&make_stream(flate.clone(), raw)).unwrap(),
            b"raw deflate"
        );
        let stored_then_broken = [
            0x78, 0x01, 0x00, 0x05, 0x00, 0xFA, 0xFF, b'H', b'e', b'l', b'l', b'o', 0x01, 0x05,
            0x00, 0x00, 0x00,
        ];
        assert_eq!(
            decode_stream(&make_stream(flate, stored_then_broken.to_vec())).unwrap(),
            b"Hello"
        );
    }

    #[test]
    fn brotli_stream_decodes() {
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("BrotliDecode".into()));
        let hello = [0x0B, 0x02, 0x80, b'H', b'e', b'l', b'l', b'o', 0x03];
        assert_eq!(
            decode_stream(&make_stream(dict, hello.to_vec())).unwrap(),
            b"Hello"
        );
    }

    #[test]
    fn ascii_filters_stop_at_a_byte_outside_their_alphabet() {
        let mut hex = PdfDictionary::new();
        hex.insert(
            "Filter".to_string(),
            PdfValue::Name("ASCIIHexDecode".into()),
        );
        assert_eq!(
            decode_stream(&make_stream(hex, b"4869 zz 41>".to_vec())).unwrap(),
            b"Hi"
        );
        let mut a85 = PdfDictionary::new();
        a85.insert("Filter".to_string(), PdfValue::Name("ASCII85Decode".into()));
        assert_eq!(
            decode_stream(&make_stream(a85, b"9jqo^\xB1~>".to_vec())).unwrap(),
            b"Man "
        );
    }

    #[test]
    fn decodes_ascii_hex() {
        let encoded = b"48656C6C6F>".to_vec();
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("ASCIIHexDecode".into()),
        );
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"Hello".to_vec());
    }

    #[test]
    fn rejects_unsupported_predictor() {
        let mut dict = PdfDictionary::new();
        let mut parms = PdfDictionary::new();
        parms.insert("Predictor".to_string(), PdfValue::Integer(3));
        dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));
        dict.insert("Filter".to_string(), PdfValue::Name("FlateDecode".into()));
        let stream = make_stream(dict, super::flate_encode(&[0, 0, 0, 0]).unwrap());
        match decode_stream(&stream) {
            Err(PdfError::Unsupported(msg)) => {
                assert!(msg.contains("predictor"), "got: {msg}")
            }
            other => panic!("expected Unsupported, got: {other:?}"),
        }
    }

    /// Encode `input` with the TIFF-compatible LZW variant used by PDF
    /// (9–12-bit codes, 256 = CLEAR, 257 = EOD). `early_change` mirrors
    /// `DecodeParms /EarlyChange`: `true` switches width one code earlier.
    fn encode_lzw(input: &[u8], early_change: bool) -> Vec<u8> {
        use std::collections::HashMap;

        let mut out: Vec<u8> = Vec::new();
        let mut bit_buffer: u64 = 0;
        let mut bit_count: u32 = 0;
        let flush_code = |code: u32,
                          width: u32,
                          bit_buffer: &mut u64,
                          bit_count: &mut u32,
                          out: &mut Vec<u8>| {
            *bit_buffer = (*bit_buffer << width) | u64::from(code);
            *bit_count += width;
            while *bit_count >= 8 {
                *bit_count -= 8;
                out.push(((*bit_buffer >> *bit_count) & 0xFF) as u8);
                *bit_buffer &= (1u64 << *bit_count) - 1;
            }
        };

        // Start every stream with CLEAR.
        flush_code(256, 9, &mut bit_buffer, &mut bit_count, &mut out);

        let mut dict: HashMap<Vec<u8>, u32> = HashMap::new();
        for b in 0u32..256 {
            dict.insert(vec![b as u8], b);
        }
        let mut next_code: u32 = 258;
        let mut code_width: u32 = 9;

        let mut buffer: Vec<u8> = Vec::new();
        for &byte in input {
            let mut extended = buffer.clone();
            extended.push(byte);
            if dict.contains_key(&extended) {
                buffer = extended;
            } else {
                let code = dict[&buffer];
                flush_code(code, code_width, &mut bit_buffer, &mut bit_count, &mut out);
                dict.insert(extended, next_code);
                next_code += 1;
                let threshold = if early_change {
                    1u32 << code_width
                } else {
                    (1u32 << code_width) + 1
                };
                if next_code >= threshold && code_width < 12 {
                    code_width += 1;
                }
                buffer = vec![byte];
            }
        }
        if !buffer.is_empty() {
            let code = dict[&buffer];
            flush_code(code, code_width, &mut bit_buffer, &mut bit_count, &mut out);
        }
        flush_code(257, code_width, &mut bit_buffer, &mut bit_count, &mut out);
        if bit_count > 0 {
            out.push(((bit_buffer << (8 - bit_count)) & 0xFF) as u8);
        }
        out
    }

    #[test]
    fn decodes_lzw_spec_example() {
        // PDF 1.7 spec § 7.4.4.3, Annex A.3: "-----A---B" encodes to the
        // 8 nine-bit codes 256, 45, 258, 258, 65, 259, 66, 257, which pack
        // MSB-first into these nine bytes.
        let data = vec![0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let stream = make_stream(dict, data);
        assert_eq!(decode_stream(&stream).unwrap(), b"-----A---B".to_vec());
    }

    #[test]
    fn decodes_lzw_roundtrip_default_early_change() {
        let plaintext = b"the quick brown fox jumps over the lazy dog".to_vec();
        let encoded = encode_lzw(&plaintext, true);
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), plaintext);
    }

    #[test]
    fn decodes_lzw_roundtrip_early_change_zero() {
        let plaintext = b"the quick brown fox jumps over the lazy dog".to_vec();
        let encoded = encode_lzw(&plaintext, false);
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let mut parms = PdfDictionary::new();
        parms.insert("EarlyChange".to_string(), PdfValue::Integer(0));
        dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), plaintext);
    }

    #[test]
    fn decodes_lzw_with_tiff_predictor() {
        // Original 2 rows × 4 bytes, 1 colour, 8 bits per component.
        // TIFF predictor leaves the first byte per row as-is and stores
        // the rest as delta from the previous byte. The LZW filter sits
        // on top: it compresses the predictor-encoded bytes.
        let original: [u8; 8] = [10, 20, 30, 40, 15, 22, 33, 44];
        let mut predictor_encoded = Vec::new();
        for row in original.chunks(4) {
            predictor_encoded.push(row[0]);
            for index in 1..row.len() {
                predictor_encoded.push(row[index].wrapping_sub(row[index - 1]));
            }
        }
        let lzw_bytes = encode_lzw(&predictor_encoded, true);
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let mut parms = PdfDictionary::new();
        parms.insert("Predictor".to_string(), PdfValue::Integer(2));
        parms.insert("Columns".to_string(), PdfValue::Integer(4));
        dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));
        let stream = make_stream(dict, lzw_bytes);
        assert_eq!(decode_stream(&stream).unwrap(), original.to_vec());
    }

    #[test]
    fn decodes_lzw_exercises_code_width_transitions() {
        // Build an input long enough to force the dictionary past 511
        // entries so the decoder exercises the 9→10 and 10→11 bit width
        // transitions. ~1200 unique trigrams from a pangram-ish repeat
        // suffices.
        let mut plaintext = Vec::new();
        for i in 0u16..1200 {
            plaintext.push(b'a' + (i % 26) as u8);
            plaintext.push(b'A' + (i % 26) as u8);
            plaintext.push(b'0' + (i % 10) as u8);
        }
        let encoded = encode_lzw(&plaintext, true);
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), plaintext);
    }

    #[test]
    fn decodes_run_length_literal_runs() {
        // Length byte 2 means "copy next 3 bytes literally". EOD = 128.
        let encoded = vec![2, b'A', b'B', b'C', 128];
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("RunLengthDecode".into()),
        );
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"ABC".to_vec());
    }

    #[test]
    fn decodes_run_length_repeat_runs() {
        // Length byte 0xFF (255) means "repeat next byte (257-255)=2 times".
        let encoded = vec![0xFF, b'Z', 128];
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("RL".into()));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"ZZ".to_vec());
    }

    #[test]
    fn decodes_run_length_mixed_runs_without_eod() {
        // "ABBBCD" packed as literal A, repeat B x3, literal CD. No trailing
        // EOD byte, some producers omit it and we treat that as end of
        // stream rather than corruption.
        let encoded = vec![0, b'A', 0xFE, b'B', 1, b'C', b'D'];
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("RunLengthDecode".into()),
        );
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"ABBBCD".to_vec());
    }

    #[test]
    fn decodes_filter_chain_run_length_then_flate() {
        // Flate-compress a plaintext, then wrap the Flate bytes as a pure
        // literal RunLengthDecode stream. Decoding has to run RL first and
        // then Flate on the recovered payload, guarding the dispatch
        // order inside `decode_stream`.
        let plaintext = b"RunLengthInsideAFilterChain".to_vec();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&plaintext).unwrap();
        let flate_bytes = encoder.finish().unwrap();

        // Encode `flate_bytes` as RunLengthDecode using only literal runs
        // of up to 128 bytes followed by the 128 EOD marker.
        let mut rl_bytes = Vec::new();
        let mut offset = 0usize;
        while offset < flate_bytes.len() {
            let run_len = (flate_bytes.len() - offset).min(128);
            rl_bytes.push((run_len - 1) as u8);
            rl_bytes.extend_from_slice(&flate_bytes[offset..offset + run_len]);
            offset += run_len;
        }
        rl_bytes.push(128);

        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Array(vec![
                PdfValue::Name("RunLengthDecode".into()),
                PdfValue::Name("FlateDecode".into()),
            ]),
        );
        let stream = make_stream(dict, rl_bytes);
        assert_eq!(decode_stream(&stream).unwrap(), plaintext);
    }

    #[test]
    fn rejects_run_length_truncated_literal_run() {
        // Length byte 3 claims 4 bytes of literal but only 2 follow.
        let encoded = vec![3, b'A', b'B'];
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("RunLengthDecode".into()),
        );
        let stream = make_stream(dict, encoded);
        let err = decode_stream(&stream).unwrap_err();
        assert!(matches!(err, PdfError::Corrupt(_)), "got: {err:?}");
    }

    #[test]
    fn rejects_run_length_truncated_repeat_run() {
        // Length byte 200 implies a repeat with a payload byte, but the
        // payload is missing (stream ends immediately after the length).
        let encoded = vec![200];
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("RunLengthDecode".into()),
        );
        let stream = make_stream(dict, encoded);
        let err = decode_stream(&stream).unwrap_err();
        assert!(matches!(err, PdfError::Corrupt(_)), "got: {err:?}");
    }

    #[test]
    fn rejects_lzw_out_of_range_code() {
        // Single 9-bit code 0x1FF (= 511) after a CLEAR is outside the
        // still-256-entry dictionary and not equal to `next_code` yet,
        // so the decoder must refuse rather than silently emit.
        let mut out: Vec<u8> = Vec::new();
        let mut bit_buffer: u64 = 0;
        let mut bit_count: u32 = 0;
        let mut push = |code: u32, width: u32| {
            bit_buffer = (bit_buffer << width) | u64::from(code);
            bit_count += width;
            while bit_count >= 8 {
                bit_count -= 8;
                out.push(((bit_buffer >> bit_count) & 0xFF) as u8);
                bit_buffer &= (1u64 << bit_count) - 1;
            }
        };
        push(256, 9); // CLEAR
        push(511, 9); // invalid
        if bit_count > 0 {
            out.push(((bit_buffer << (8 - bit_count)) & 0xFF) as u8);
        }
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let stream = make_stream(dict, out);
        let err = decode_stream(&stream).unwrap_err();
        assert!(matches!(err, PdfError::Corrupt(_)), "got: {err:?}");
    }
}

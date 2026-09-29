use crate::objects::{PdfDictionary, PdfFile, PdfStream, PdfValue};

use super::StandardMetrics;

const MULTILINE: i64 = 1 << 12;
const COMBO: i64 = 1 << 17;

/// What a viewer draws for a field or a free text the file leaves without an
/// appearance: its value in Helvetica, at the size its `/DA` asks.
pub(super) fn drawn(
    file: &PdfFile,
    annotation: &PdfDictionary,
    width: f64,
    height: f64,
) -> Option<PdfStream> {
    let (lines, stacked) = match annotation.get("Subtype").and_then(PdfValue::as_name)? {
        "FreeText" => (
            split_lines(&text_of(file, annotation.get("Contents")?)?),
            true,
        ),
        "Widget" => {
            let flags = inherited(file, annotation, "Ff")
                .and_then(PdfValue::as_integer)
                .unwrap_or(0);
            match inherited(file, annotation, "FT").and_then(PdfValue::as_name)? {
                "Tx" => {
                    let value = text_of(file, inherited(file, annotation, "V")?)?;
                    match flags & MULTILINE != 0 {
                        true => (split_lines(&value), true),
                        false => (vec![value.replace(['\r', '\n'], " ")], false),
                    }
                }
                "Ch" if flags & COMBO != 0 => {
                    let value = match file.resolve(inherited(file, annotation, "V")?).ok()? {
                        PdfValue::Array(values) => text_of(file, values.first()?)?,
                        value => text_of(file, value)?,
                    };
                    (vec![value], false)
                }
                "Ch" => {
                    let options = inherited(file, annotation, "Opt")
                        .and_then(|v| file.resolve(v).ok())
                        .and_then(PdfValue::as_array)?;
                    let lines = options
                        .iter()
                        .filter_map(|option| match file.resolve(option).ok()? {
                            PdfValue::Array(pair) => text_of(file, pair.get(1)?),
                            value => text_of(file, value),
                        })
                        .collect();
                    (lines, true)
                }
                _ => return None,
            }
        }
        _ => return None,
    };
    if lines.iter().all(|line| line.trim().is_empty()) {
        return None;
    }
    let da = inherited(file, annotation, "DA")
        .and_then(|v| text_of(file, v))
        .or_else(|| {
            let root = file.trailer.get("Root")?;
            let form = file.resolve_dict(root).ok()?.get("AcroForm")?;
            text_of(file, file.resolve_dict(form).ok()?.get("DA")?)
        });
    let size = da
        .as_deref()
        .and_then(font_size)
        .filter(|size| *size > 0.0)
        .unwrap_or_else(|| (height - 4.0).clamp(4.0, 12.0));
    let alignment = inherited(file, annotation, "Q")
        .and_then(PdfValue::as_integer)
        .unwrap_or(0);
    let mut content = String::new();
    for (index, line) in lines.iter().enumerate() {
        let (font, bytes) = encode(line);
        let advance: f64 = match font {
            "LjW" => bytes
                .iter()
                .map(|byte| StandardMetrics::Helvetica.width(*byte, ""))
                .sum::<f64>(),
            _ => 556.0 * (bytes.len() / 2) as f64,
        } * size
            / 1000.0;
        let x = match alignment {
            1 => (width - advance) / 2.0,
            2 => width - 2.0 - advance,
            _ => 2.0,
        };
        let y = match stacked {
            true => height - 2.0 - size * (0.718 + 1.15 * index as f64),
            false => (height - 0.925 * size) / 2.0 + 0.207 * size,
        };
        let hex: String = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
        content.push_str(&format!(
            "BT /{font} {size} Tf {x:.3} {y:.3} Td <{hex}> Tj ET\n"
        ));
    }
    Some(PdfStream {
        dict: PdfDictionary::from([
            (
                "BBox".to_string(),
                PdfValue::Array(
                    [0.0, 0.0, width, height]
                        .into_iter()
                        .map(PdfValue::Number)
                        .collect(),
                ),
            ),
            ("Resources".to_string(), fonts()),
        ]),
        data: content.into_bytes(),
    })
}

fn fonts() -> PdfValue {
    let name = |n: &str| PdfValue::Name(n.to_string());
    let dict = |entries: Vec<(&str, PdfValue)>| {
        PdfValue::Dictionary(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    };
    let latin = dict(vec![
        ("Type", name("Font")),
        ("Subtype", name("Type1")),
        ("BaseFont", name("Helvetica")),
        ("Encoding", name("WinAnsiEncoding")),
    ]);
    let unicode = dict(vec![
        ("Type", name("Font")),
        ("Subtype", name("Type0")),
        ("BaseFont", name("Helvetica")),
        ("Encoding", name("Identity-H")),
        ("ToUnicode", name("Identity-H")),
        (
            "DescendantFonts",
            PdfValue::Array(vec![dict(vec![
                ("Type", name("Font")),
                ("Subtype", name("CIDFontType2")),
                ("DW", PdfValue::Number(556.0)),
            ])]),
        ),
    ]);
    dict(vec![("Font", dict(vec![("LjW", latin), ("LjU", unicode)]))])
}

/// The line in WinAnsi when every character has a byte there, else in
/// UTF-16 code units.
fn encode(line: &str) -> (&'static str, Vec<u8>) {
    let winansi: Option<Vec<u8>> = line
        .chars()
        .map(|character| {
            (0x20..=0xFFu8).find(|byte| super::winansi_byte_to_char(*byte) == Some(character))
        })
        .collect();
    match winansi {
        Some(bytes) => ("LjW", bytes),
        None => (
            "LjU",
            line.chars()
                .map(|c| if (c as u32) < 0x10000 { c } else { '\u{FFFD}' })
                .flat_map(|c| (c as u16).to_be_bytes())
                .collect(),
        ),
    }
}

fn font_size(da: &str) -> Option<f64> {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let at = tokens.iter().rposition(|token| *token == "Tf")?;
    tokens.get(at.checked_sub(1)?)?.parse().ok()
}

fn split_lines(text: &str) -> Vec<String> {
    text.split("\r\n")
        .flat_map(|line| line.split(['\r', '\n']))
        .map(str::to_string)
        .collect()
}

fn inherited<'a>(file: &'a PdfFile, dict: &'a PdfDictionary, key: &str) -> Option<&'a PdfValue> {
    let mut current = dict;
    for _ in 0..32 {
        if let Some(value) = current.get(key) {
            return Some(value);
        }
        current = file.resolve_dict(current.get("Parent")?).ok()?;
    }
    None
}

fn text_of(file: &PdfFile, value: &PdfValue) -> Option<String> {
    match file.resolve(value).ok()? {
        PdfValue::String(string) => Some(text_string(&string.0)),
        _ => None,
    }
}

/// A PDF text string: UTF-16BE or UTF-8 behind their byte order mark, else
/// PDFDocEncoding.
pub(crate) fn text_string(bytes: &[u8]) -> String {
    if let Some(units) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = units
            .chunks(2)
            .map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(utf8) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(utf8).into_owned();
    }
    bytes.iter().map(|byte| pdf_doc_char(*byte)).collect()
}

fn pdf_doc_char(byte: u8) -> char {
    const LOW: [u16; 8] = [
        0x02D8, 0x02C7, 0x02C6, 0x02D9, 0x02DD, 0x02DB, 0x02DA, 0x02DC,
    ];
    const HIGH: [u16; 33] = [
        0x2022, 0x2020, 0x2021, 0x2026, 0x2014, 0x2013, 0x0192, 0x2044, 0x2039, 0x203A, 0x2212,
        0x2030, 0x201E, 0x201C, 0x201D, 0x2018, 0x2019, 0x201A, 0x2122, 0xFB01, 0xFB02, 0x0141,
        0x0152, 0x0160, 0x0178, 0x017D, 0x0131, 0x0142, 0x0153, 0x0161, 0x017E, 0xFFFD, 0x20AC,
    ];
    let unit = match byte {
        0x18..=0x1F => LOW[usize::from(byte - 0x18)],
        0x80..=0xA0 => HIGH[usize::from(byte - 0x80)],
        _ => u16::from(byte),
    };
    char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}')
}

#[cfg(test)]
mod tests {
    use super::{font_size, text_string};

    #[test]
    fn text_strings_read_utf16_and_pdfdocencoding() {
        assert_eq!(
            text_string(&[0xFE, 0xFF, 0x00, 0x5A, 0x01, 0x42]),
            "Z\u{142}"
        );
        assert_eq!(
            text_string(b"caf\xE9 \x84 \x92"),
            "caf\u{e9} \u{2014} \u{2122}"
        );
        assert_eq!(font_size("/Helv 0 Tf 0 g"), Some(0.0));
        assert_eq!(font_size("0.2 g /F1 9.5 Tf"), Some(9.5));
    }
}

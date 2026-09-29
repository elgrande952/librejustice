use crate::graphics::{Matrix, Point, Rect};
use crate::objects::{
    PageInfo, PdfDictionary, PdfError, PdfFile, PdfResult, PdfString, PdfValue, decode_stream,
    document::get_stream,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub operator: String,
    pub operands: Vec<PdfValue>,
}

#[derive(Debug, Clone)]
pub struct ContentStream {
    pub decoded: Vec<u8>,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone)]
pub struct ParsedPageContent {
    pub bytes: Vec<u8>,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone)]
pub struct OperandString {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaintOperator {
    Stroke,
    Fill,
    FillEvenOdd,
    StrokeFill,
    StrokeFillEvenOdd,
    CloseStroke,
    CloseFill,
    CloseFillEvenOdd,
    NoPaint,
}

impl PaintOperator {
    pub fn from_operator(operator: &str) -> Option<Self> {
        match operator {
            "S" => Some(Self::Stroke),
            "s" => Some(Self::CloseStroke),
            "f" | "F" => Some(Self::Fill),
            "f*" => Some(Self::FillEvenOdd),
            "B" => Some(Self::StrokeFill),
            "B*" => Some(Self::StrokeFillEvenOdd),
            "b" => Some(Self::CloseFill),
            "b*" => Some(Self::CloseFillEvenOdd),
            "n" => Some(Self::NoPaint),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum PathSegment {
    MoveTo(Point),
    LineTo(Point),
    CurveTo(Point, Point, Point),
    Rect(Rect),
    ClosePath,
}

#[derive(Debug, Clone, Copy)]
pub struct TextState {
    pub text_matrix: Matrix,
    pub line_matrix: Matrix,
    pub font_size: f64,
    pub character_spacing: f64,
    pub word_spacing: f64,
    pub text_rise: f64,
    pub horizontal_scaling: f64,
    pub leading: f64,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            text_matrix: Matrix::identity(),
            line_matrix: Matrix::identity(),
            font_size: 12.0,
            character_spacing: 0.0,
            word_spacing: 0.0,
            text_rise: 0.0,
            horizontal_scaling: 100.0,
            leading: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GraphicsState {
    pub ctm: Matrix,
    pub stroke_width: f64,
    pub text_state: TextState,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Matrix::identity(),
            stroke_width: 1.0,
            text_state: TextState::default(),
        }
    }
}

pub fn parse_page_contents(file: &PdfFile, page: &PageInfo) -> PdfResult<ParsedPageContent> {
    let bytes = page_content_bytes(file, page);
    let stream = parse_content_stream(&bytes)?;
    Ok(ParsedPageContent {
        bytes,
        operations: stream.operations,
    })
}

/// The page's content streams decoded and joined, a newline between two.
pub(crate) fn page_content_bytes(file: &PdfFile, page: &PageInfo) -> Vec<u8> {
    let mut bytes = Vec::new();
    for content_ref in &page.content_refs {
        let Ok(decoded) = get_stream(file, *content_ref).and_then(decode_stream) else {
            continue;
        };
        bytes.extend_from_slice(&decoded);
        if !decoded.ends_with(b"\n") {
            bytes.push(b'\n');
        }
    }
    bytes
}

/// Each inline image is a `BI` operation whose operands are its dictionary,
/// keys as written, and its still encoded data as a string.
pub fn parse_content_stream(bytes: &[u8]) -> PdfResult<ContentStream> {
    let mut parser = ContentParser::new(bytes);
    let mut operations = Vec::new();
    let mut operands = Vec::new();
    while !parser.eof() {
        parser.skip_ws_and_comments();
        if parser.eof() {
            break;
        }
        if let Some(value) = parser.try_parse_operand()? {
            operands.push(value);
            continue;
        }
        let Some(operator) = parser.parse_operator() else {
            parser.position += 1;
            continue;
        };
        if operator == "BI" {
            let (dict, data) = parser.inline_image()?;
            operands.clear();
            operations.push(Operation {
                operator,
                operands: vec![
                    PdfValue::Dictionary(dict),
                    PdfValue::String(PdfString(data)),
                ],
            });
            continue;
        }
        operations.push(Operation {
            operator,
            operands: std::mem::take(&mut operands),
        });
    }
    Ok(ContentStream {
        decoded: bytes.to_vec(),
        operations,
    })
}

struct ContentParser<'a> {
    bytes: &'a [u8],
    position: usize,
    /// Arrays and dictionaries open around the operand being parsed.
    depth: usize,
}

/// Operands nested deeper are refused: each level is a frame of a recursive
/// parse, on the 1 MiB stack of a wasm module.
const MAX_NESTING: usize = 256;

impl<'a> ContentParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            depth: 0,
        }
    }

    fn nested(
        &mut self,
        parse: fn(&mut Self) -> PdfResult<PdfValue>,
    ) -> PdfResult<Option<PdfValue>> {
        if self.depth == MAX_NESTING {
            return Err(PdfError::Parse(format!(
                "operands nest deeper than {MAX_NESTING}"
            )));
        }
        self.depth += 1;
        let value = parse(self);
        self.depth -= 1;
        value.map(Some)
    }

    fn eof(&self) -> bool {
        self.position >= self.bytes.len()
    }

    fn current(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_ws_and_comments(&mut self) {
        while let Some(byte) = self.current() {
            match byte {
                b' ' | b'\t' | b'\r' | b'\n' | 0x0C | 0x00 => self.position += 1,
                b'%' => {
                    while let Some(next) = self.current() {
                        self.position += 1;
                        if next == b'\n' || next == b'\r' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn try_parse_operand(&mut self) -> PdfResult<Option<PdfValue>> {
        self.skip_ws_and_comments();
        match self.current() {
            Some(b'/') => self.parse_name().map(Some),
            Some(b'(') => self.parse_literal_string().map(Some),
            Some(b'[') => self.nested(Self::parse_array),
            Some(b'<') if self.bytes.get(self.position + 1) == Some(&b'<') => {
                self.nested(Self::parse_dictionary)
            }
            Some(b'<') => self.parse_hex_string().map(Some),
            Some(b't') if self.peek_keyword("true") => {
                self.position += 4;
                Ok(Some(PdfValue::Bool(true)))
            }
            Some(b'f') if self.peek_keyword("false") => {
                self.position += 5;
                Ok(Some(PdfValue::Bool(false)))
            }
            Some(b'n') if self.peek_keyword("null") => {
                self.position += 4;
                Ok(Some(PdfValue::Null))
            }
            Some(byte) if byte == b'+' || byte == b'-' || byte.is_ascii_digit() || byte == b'.' => {
                self.parse_number().map(Some)
            }
            _ => Ok(None),
        }
    }

    fn parse_operator(&mut self) -> Option<String> {
        self.skip_ws_and_comments();
        let start = self.position;
        while let Some(byte) = self.current() {
            if is_whitespace(byte) || is_delimiter(byte) {
                break;
            }
            self.position += 1;
        }
        (self.position > start)
            .then(|| String::from_utf8_lossy(&self.bytes[start..self.position]).to_string())
    }

    fn parse_name(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let mut raw = Vec::new();
        while let Some(byte) = self.current() {
            if is_whitespace(byte) || is_delimiter(byte) {
                break;
            }
            if byte == b'#' {
                let high = self.bytes.get(self.position + 1).copied().unwrap_or(b'0');
                let low = self.bytes.get(self.position + 2).copied().unwrap_or(b'0');
                if let Ok(decoded) =
                    u8::from_str_radix(&format!("{}{}", high as char, low as char), 16)
                {
                    raw.push(decoded);
                    self.position += 3;
                } else {
                    raw.push(byte);
                    self.position += 1;
                }
            } else {
                raw.push(byte);
                self.position += 1;
            }
        }
        Ok(PdfValue::Name(String::from_utf8_lossy(&raw).to_string()))
    }

    fn parse_literal_string(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let mut output = Vec::new();
        let mut depth = 1usize;
        while let Some(byte) = self.current() {
            self.position += 1;
            match byte {
                b'\\' => {
                    let Some(escaped) = self.current() else {
                        break;
                    };
                    self.position += 1;
                    match escaped {
                        b'n' => output.push(b'\n'),
                        b'r' => output.push(b'\r'),
                        b't' => output.push(b'\t'),
                        b'b' => output.push(0x08),
                        b'f' => output.push(0x0C),
                        b'(' | b')' | b'\\' => output.push(escaped),
                        b'\n' => {}
                        b'\r' => {
                            if self.current() == Some(b'\n') {
                                self.position += 1;
                            }
                        }
                        b'0'..=b'7' => {
                            let mut octal = vec![escaped];
                            for _ in 0..2 {
                                match self.current() {
                                    Some(next @ b'0'..=b'7') => {
                                        octal.push(next);
                                        self.position += 1;
                                    }
                                    _ => break,
                                }
                            }
                            // PDF spec: octal value is taken modulo 256
                            let value =
                                u16::from_str_radix(std::str::from_utf8(&octal).unwrap_or("0"), 8)
                                    .unwrap_or(0);
                            output.push((value % 256) as u8);
                        }
                        other => output.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    output.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(PdfValue::String(PdfString(output)));
                    }
                    output.push(byte);
                }
                _ => output.push(byte),
            }
        }
        Ok(PdfValue::String(PdfString(output)))
    }

    fn parse_hex_string(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let start = self.position;
        while !self.eof() && self.current() != Some(b'>') {
            self.position += 1;
        }
        let mut digits: Vec<u8> = self.bytes[start..self.position]
            .iter()
            .filter_map(|byte| (*byte as char).to_digit(16).map(|d| d as u8))
            .collect();
        self.position += 1;
        if !digits.len().is_multiple_of(2) {
            digits.push(0);
        }
        let bytes = digits
            .chunks(2)
            .map(|pair| pair[0] << 4 | pair[1])
            .collect();
        Ok(PdfValue::String(PdfString(bytes)))
    }

    fn parse_array(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let mut values = Vec::new();
        loop {
            self.skip_ws_and_comments();
            match self.current() {
                Some(b']') => {
                    self.position += 1;
                    break;
                }
                Some(_) => match self.try_parse_operand()? {
                    Some(value) => values.push(value),
                    None => self.skip_token(),
                },
                None => break,
            }
        }
        Ok(PdfValue::Array(values))
    }

    fn parse_number(&mut self) -> PdfResult<PdfValue> {
        let start = self.position;
        if matches!(self.current(), Some(b'+' | b'-')) {
            self.position += 1;
        }
        while let Some(byte) = self.current() {
            if !(byte.is_ascii_digit() || byte == b'.') {
                break;
            }
            self.position += 1;
        }
        let token = String::from_utf8_lossy(&self.bytes[start..self.position]).to_string();
        // A malformed number reads as its longest valid prefix, as viewers do.
        let prefix = match token.match_indices('.').nth(1) {
            Some((second_dot, _)) => &token[..second_dot],
            None => &token,
        };
        Ok(match prefix.contains('.') {
            true => PdfValue::Number(
                prefix
                    .parse()
                    .unwrap_or_else(|_| prefix.trim_end_matches('.').parse::<f64>().unwrap_or(0.0)),
            ),
            false => prefix
                .parse()
                .map(PdfValue::Integer)
                .unwrap_or_else(|_| PdfValue::Number(prefix.parse().unwrap_or(0.0))),
        })
    }

    fn skip_token(&mut self) {
        let start = self.position;
        while let Some(byte) = self.current() {
            if is_whitespace(byte) || is_delimiter(byte) {
                break;
            }
            self.position += 1;
        }
        if self.position == start {
            self.position += 1;
        }
    }

    fn parse_dictionary(&mut self) -> PdfResult<PdfValue> {
        self.position += 2; // skip <<
        let mut dictionary = PdfDictionary::new();
        loop {
            self.skip_ws_and_comments();
            if self.current() == Some(b'>') && self.bytes.get(self.position + 1) == Some(&b'>') {
                self.position += 2;
                break;
            }
            if self.eof() {
                break;
            }
            if self.current() != Some(b'/') {
                self.skip_token();
                continue;
            }
            let PdfValue::Name(key) = self.parse_name()? else {
                continue;
            };
            match self.try_parse_operand()? {
                Some(value) => {
                    dictionary.insert(key, value);
                }
                None => self.skip_token(),
            }
        }
        Ok(PdfValue::Dictionary(dictionary))
    }

    /// After `BI`, reads the key/value pairs, `ID`, the data and `EI`. The
    /// data runs to its computed length when the image is unfiltered and its
    /// colour space a device one, and otherwise to the first `EI` that stands
    /// alone and is followed by what reads as content, or failing that to the
    /// first that stands alone.
    fn inline_image(&mut self) -> PdfResult<(PdfDictionary, Vec<u8>)> {
        let mut dict = PdfDictionary::new();
        loop {
            self.skip_ws_and_comments();
            if self.eof() {
                return Ok((dict, Vec::new()));
            }
            if self.current() == Some(b'I')
                && self.bytes.get(self.position + 1) == Some(&b'D')
                && self
                    .bytes
                    .get(self.position + 2)
                    .is_none_or(|&byte| is_whitespace(byte))
            {
                self.position += 3;
                break;
            }
            if self.current() == Some(b'/') {
                let PdfValue::Name(key) = self.parse_name()? else {
                    continue;
                };
                if let Some(value) = self.try_parse_operand()? {
                    dict.insert(key, value);
                }
            } else {
                self.position += 1;
            }
        }
        let start = self.position.min(self.bytes.len());
        if let Some(length) = inline_image_length(&dict) {
            let end = start.saturating_add(length);
            if end <= self.bytes.len() {
                let mut after = end;
                while self
                    .bytes
                    .get(after)
                    .is_some_and(|&byte| is_whitespace(byte))
                {
                    after += 1;
                }
                if self.bytes.get(after..after + 2) == Some(b"EI")
                    && self
                        .bytes
                        .get(after + 2)
                        .is_none_or(|&byte| is_whitespace(byte) || is_delimiter(byte))
                {
                    self.position = after + 2;
                    return Ok((dict, self.bytes[start..end].to_vec()));
                }
            }
        }
        let mut position = start;
        let mut end = None;
        while position + 1 < self.bytes.len() {
            if self.bytes[position] == b'E'
                && self.bytes[position + 1] == b'I'
                && position > start
                && is_whitespace(self.bytes[position - 1])
                && self
                    .bytes
                    .get(position + 2)
                    .is_none_or(|&byte| is_whitespace(byte) || is_delimiter(byte))
            {
                if reads_as_content(&self.bytes[position + 2..]) {
                    end = Some(position);
                    break;
                }
                end.get_or_insert(position);
            }
            position += 1;
        }
        let Some(position) = end else {
            self.position = self.bytes.len();
            return Ok((dict, self.bytes[start..].to_vec()));
        };
        self.position = position + 2;
        Ok((dict, self.bytes[start..position - 1].to_vec()))
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        self.bytes
            .get(self.position..self.position + keyword.len())
            .map(|slice| slice == keyword.as_bytes())
            .unwrap_or(false)
    }
}

/// The byte length of an unfiltered inline image in a device colour space.
fn inline_image_length(dict: &PdfDictionary) -> Option<usize> {
    let get = |short: &str, long: &str| dict.get(short).or_else(|| dict.get(long));
    if get("F", "Filter")
        .is_some_and(|filter| !matches!(filter, PdfValue::Array(items) if items.is_empty()))
    {
        return None;
    }
    let width = usize::try_from(get("W", "Width")?.as_integer()?).ok()?;
    let height = usize::try_from(get("H", "Height")?.as_integer()?).ok()?;
    let mask = get("IM", "ImageMask").and_then(PdfValue::as_bool) == Some(true);
    let (bits, components) = if mask {
        (1, 1)
    } else {
        let bits = usize::try_from(get("BPC", "BitsPerComponent")?.as_integer()?).ok()?;
        let components = match get("CS", "ColorSpace")? {
            PdfValue::Name(name) => match name.as_str() {
                "G" | "DeviceGray" | "I" | "Indexed" => 1,
                "RGB" | "DeviceRGB" => 3,
                "CMYK" | "DeviceCMYK" => 4,
                _ => return None,
            },
            PdfValue::Array(items) => match items.first()?.as_name()? {
                "I" | "Indexed" => 1,
                _ => return None,
            },
            _ => return None,
        };
        (bits, components)
    };
    width
        .checked_mul(bits)?
        .checked_mul(components)?
        .div_ceil(8)
        .checked_mul(height)
}

/// Whether what follows a candidate `EI` looks like operators and operands
/// rather than more binary image data.
fn reads_as_content(rest: &[u8]) -> bool {
    rest.iter()
        .take(16)
        .all(|&byte| is_whitespace(byte) || (0x20..0x7f).contains(&byte))
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0C | 0x00)
}

fn is_delimiter(byte: u8) -> bool {
    matches!(byte, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'/' | b'%')
}

#[cfg(test)]
mod tests {
    use super::parse_content_stream;
    use crate::objects::{PdfString, PdfValue, serialize_value};

    #[test]
    fn malformed_tokens_read_as_viewers_read_them() {
        let stream = b"BT . 5 Td [(a) Tw <6G2> (b)] TJ ) 1.2.3 - Td (x) Tj 7";
        let parsed = parse_content_stream(stream).unwrap();
        let read: Vec<_> = parsed
            .operations
            .iter()
            .map(|op| (op.operator.as_str(), op.operands.clone()))
            .collect();
        let string = |s: &[u8]| PdfValue::String(PdfString(s.to_vec()));
        assert_eq!(
            read,
            [
                ("BT", vec![]),
                ("Td", vec![PdfValue::Number(0.0), PdfValue::Integer(5)]),
                (
                    "TJ",
                    vec![PdfValue::Array(vec![
                        string(b"a"),
                        string(&[0x62]),
                        string(b"b")
                    ])]
                ),
                ("Td", vec![PdfValue::Number(1.2), PdfValue::Number(0.0)]),
                ("Tj", vec![string(b"x")]),
            ]
        );
    }

    #[test]
    fn parses_dictionary_operand_in_content_stream() {
        let stream = b"/Span <</MCID 0>> BDC\nBT\nET\nEMC\n";
        let parsed = parse_content_stream(stream).expect("should parse dictionary operand");
        let bdc = &parsed.operations[0];
        assert_eq!(bdc.operator, "BDC");
        assert_eq!(bdc.operands.len(), 2);
        assert!(matches!(&bdc.operands[1], PdfValue::Dictionary(_)));
    }

    #[test]
    fn keeps_inline_images() {
        let stream = b"q BI /W 2 /H 2 /CS /G /BPC 8 ID EI\n\xff\nEI Q BI /W 1 /H 1 /F /AHx /CS /RGB /BPC 8 ID\nff00ff> EI\nQ";
        let parsed = parse_content_stream(stream).unwrap();
        let operators: Vec<&str> = parsed
            .operations
            .iter()
            .map(|op| op.operator.as_str())
            .collect();
        assert_eq!(operators, ["q", "BI", "Q", "BI", "Q"]);
        let data = |index: usize| match &parsed.operations[index].operands[1] {
            PdfValue::String(data) => data.0.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(data(1), b"EI\n\xff");
        assert_eq!(data(3), b"ff00ff>");
        let PdfValue::Dictionary(dict) = &parsed.operations[3].operands[0] else {
            panic!();
        };
        assert_eq!(dict.get("F"), Some(&PdfValue::Name("AHx".into())));
    }

    #[test]
    fn ends_inline_image_at_lone_ei_before_binary_text() {
        let stream = b"BI /W 1 /H 1 /F /AHx /CS /G /BPC 8 ID\nff> EI (\xe9t\xe9) Tj";
        let parsed = parse_content_stream(stream).unwrap();
        let operators: Vec<&str> = parsed
            .operations
            .iter()
            .map(|op| op.operator.as_str())
            .collect();
        assert_eq!(operators, ["BI", "Tj"]);
    }

    #[test]
    fn content_parser_round_trips_binary_literal_strings() {
        let encoded = serialize_value(&PdfValue::String(PdfString(vec![
            0x00, 0x01, 0x00, 0x02, 0x00, 0x20, 0x00, 0x43, 0xff,
        ])));
        let stream = format!("{encoded} Tj\n");
        let parsed = parse_content_stream(stream.as_bytes()).expect("content parsing should work");
        let operand = parsed
            .operations
            .first()
            .and_then(|operation| operation.operands.first())
            .expect("string operand should be present");
        assert_eq!(
            operand,
            &PdfValue::String(PdfString(vec![
                0x00, 0x01, 0x00, 0x02, 0x00, 0x20, 0x00, 0x43, 0xff,
            ]))
        );
    }
}

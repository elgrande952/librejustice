use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use hayro_cmap::{BfString, CMapName, CidFamily, load_embedded};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Code {
    pub value: u32,
    pub len: u8,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CMap {
    codespace: Vec<(u8, [u8; 4], [u8; 4])>,
    cid_ranges: Vec<(u8, u32, u32, u32)>,
    unicode: BTreeMap<Code, String>,
    pub vertical: bool,
    pub parent: Option<String>,
    adobe: Option<Arc<hayro_cmap::CMap>>,
    unicode_parent: Option<Arc<hayro_cmap::CMap>>,
}

static PARSED: Mutex<BTreeMap<String, Arc<hayro_cmap::CMap>>> = Mutex::new(BTreeMap::new());
static PROVIDED: Mutex<BTreeMap<String, &'static [u8]>> = Mutex::new(BTreeMap::new());
static MISSING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Hand over an Adobe CMap by name, for a build that does not embed them. Its
/// bytes live as long as the process, as embedded ones do.
pub fn provide_cmap(name: &str, bytes: Vec<u8>) {
    PROVIDED
        .lock()
        .unwrap()
        .entry(name.to_string())
        .or_insert_with(|| Vec::leak(bytes));
}

/// The Adobe CMaps a reading asked for and found neither embedded nor handed
/// over, since the last call.
pub fn missing_cmaps() -> Vec<String> {
    std::mem::take(&mut *MISSING.lock().unwrap())
        .into_iter()
        .collect()
}

fn cmap_data(name: CMapName<'_>) -> Option<&'static [u8]> {
    if let Some(data) = load_embedded(name) {
        return Some(data);
    }
    let key = String::from_utf8_lossy(name.to_bytes()).into_owned();
    if let Some(data) = PROVIDED.lock().unwrap().get(&key) {
        return Some(data);
    }
    if !matches!(name, CMapName::Custom(_)) {
        MISSING.lock().unwrap().insert(key);
    }
    None
}

/// An Adobe CMap, parsed once per process. One not found is looked up again,
/// so it is named missing until it is handed over.
fn adobe(name: &str) -> Option<Arc<hayro_cmap::CMap>> {
    if let Some(cmap) = PARSED.lock().unwrap().get(name) {
        return Some(cmap.clone());
    }
    let cmap = cmap_data(CMapName::from_bytes(name.as_bytes()))
        .and_then(|data| hayro_cmap::CMap::parse(data, cmap_data))
        .map(Arc::new)?;
    PARSED
        .lock()
        .unwrap()
        .insert(name.to_string(), cmap.clone());
    Some(cmap)
}

/// The text of a CID in an Adobe character collection, by its UCS2 table.
pub(crate) fn collection_text(registry: &str, ordering: &str, cid: u32) -> Option<String> {
    let family = CidFamily::from_registry_ordering(registry.as_bytes(), ordering.as_bytes());
    let table = family.ucs2_cmap()?;
    match adobe(std::str::from_utf8(table.to_bytes()).ok()?)?.lookup_bf_string(cid)? {
        BfString::Char(character) => Some(character.to_string()),
        BfString::String(text) => Some(text),
    }
}

impl CMap {
    pub fn identity(vertical: bool) -> Self {
        let mut cmap = Self {
            vertical,
            ..Self::default()
        };
        cmap.add_codespace(&[0x00, 0x00], &[0xFF, 0xFF]);
        cmap.cid_ranges.push((2, 0, 0xFFFF, 0));
        cmap
    }

    /// An Adobe predefined CMap: its code space guessed from its family, its
    /// CIDs read from the embedded set.
    pub fn predefined(name: &str) -> Option<Self> {
        let vertical = name.ends_with("-V");
        if name.starts_with("Identity-") {
            return Some(Self::identity(vertical));
        }
        let mut cmap = Self {
            vertical,
            adobe: adobe(name),
            ..Self::default()
        };
        let one = |cmap: &mut Self, low: u8, high: u8| cmap.add_codespace(&[low], &[high]);
        let two = |cmap: &mut Self, l: [u8; 2], h: [u8; 2]| cmap.add_codespace(&l, &h);
        if name.contains("UCS2") {
            two(&mut cmap, [0x00, 0x00], [0xFF, 0xFF]);
        } else if name.contains("UTF16") {
            two(&mut cmap, [0x00, 0x00], [0xD7, 0xFF]);
            two(&mut cmap, [0xE0, 0x00], [0xFF, 0xFF]);
            cmap.add_codespace(&[0xD8, 0x00, 0xDC, 0x00], &[0xDB, 0xFF, 0xDF, 0xFF]);
        } else if name.contains("UTF32") {
            cmap.add_codespace(&[0, 0, 0, 0], &[0x00, 0x10, 0xFF, 0xFF]);
        } else if name.contains("RKSJ") {
            one(&mut cmap, 0x00, 0x80);
            one(&mut cmap, 0xA0, 0xDF);
            two(&mut cmap, [0x81, 0x40], [0x9F, 0xFC]);
            two(&mut cmap, [0xE0, 0x40], [0xFC, 0xFC]);
        } else if name.contains("EUC") {
            one(&mut cmap, 0x00, 0x80);
            two(&mut cmap, [0x8E, 0xA0], [0x8E, 0xDF]);
            two(&mut cmap, [0xA1, 0xA1], [0xFE, 0xFE]);
        } else if name.contains("GBK") || name.contains("GB-") || name.starts_with("GBpc") {
            one(&mut cmap, 0x00, 0x80);
            two(&mut cmap, [0x81, 0x40], [0xFE, 0xFE]);
        } else if name.contains("B5") {
            one(&mut cmap, 0x00, 0x80);
            two(&mut cmap, [0xA1, 0x40], [0xFE, 0xFE]);
        } else if name.contains("UHC") || name.contains("KSC") {
            one(&mut cmap, 0x00, 0x80);
            two(&mut cmap, [0x81, 0x41], [0xFE, 0xFE]);
        } else if cmap.adobe.is_some() {
            two(&mut cmap, [0x00, 0x00], [0xFF, 0xFF]);
        } else {
            return None;
        }
        Some(cmap)
    }

    pub fn unicode_codes(name: &str) -> bool {
        name.starts_with("Uni") && ["UCS2", "UTF16", "UTF32"].iter().any(|u| name.contains(u))
    }

    fn add_codespace(&mut self, low: &[u8], high: &[u8]) {
        let len = low.len().min(high.len()).clamp(1, 4);
        let mut l = [0u8; 4];
        let mut h = [0u8; 4];
        l[..len].copy_from_slice(&low[..len]);
        h[..len].copy_from_slice(&high[..len]);
        self.codespace.push((len as u8, l, h));
    }

    pub fn inherit(&mut self, parent: &CMap) {
        if self.codespace.is_empty() {
            self.codespace = parent.codespace.clone();
        }
        self.cid_ranges
            .splice(0..0, parent.cid_ranges.iter().copied());
        for (code, text) in &parent.unicode {
            self.unicode.entry(*code).or_insert_with(|| text.clone());
        }
        if self.adobe.is_none() {
            self.adobe = parent.adobe.clone();
        }
    }

    /// The shortest prefix at `at` that falls in a code space range, else as
    /// many bytes as the narrowest range takes.
    pub fn next_code(&self, bytes: &[u8], at: usize) -> Code {
        let rest = &bytes[at..];
        for len in 1..=4usize.min(rest.len()) {
            let matches = self.codespace.iter().any(|(l, low, high)| {
                *l as usize == len && (0..len).all(|i| low[i] <= rest[i] && rest[i] <= high[i])
            });
            if matches {
                return code_of(&rest[..len]);
            }
        }
        let len = self
            .codespace
            .iter()
            .map(|(l, ..)| *l as usize)
            .min()
            .unwrap_or(1)
            .min(rest.len())
            .max(1);
        code_of(&rest[..len])
    }

    pub fn cid(&self, code: Code) -> Option<u32> {
        self.cid_ranges
            .iter()
            .rev()
            .find(|(len, low, high, _)| {
                *len == code.len && *low <= code.value && code.value <= *high
            })
            .map(|(_, low, _, first)| first.saturating_add(code.value - low))
            .or_else(|| self.adobe.as_ref()?.lookup_cid_code(code.value, code.len))
    }

    /// Producers write one-byte codes on two in the ToUnicode map, so the
    /// same value on another width stands in. A map built on an Adobe table
    /// by `usecmap` reads what it does not redefine there.
    pub fn unicode(&self, code: Code) -> Option<Cow<'_, str>> {
        let own = self.unicode.get(&code).or_else(|| {
            let (low, high) = (
                Code {
                    value: code.value,
                    len: 0,
                },
                Code {
                    value: code.value,
                    len: 4,
                },
            );
            self.unicode.range(low..=high).next().map(|(_, text)| text)
        });
        if let Some(text) = own {
            return Some(Cow::Borrowed(text));
        }
        match self.unicode_parent.as_ref()?.lookup_bf_string(code.value)? {
            BfString::Char(character) => Some(Cow::Owned(character.to_string())),
            BfString::String(text) => Some(Cow::Owned(text)),
        }
    }

    /// Parses a ToUnicode stream, the Adobe table it names by `usecmap`
    /// included.
    pub fn parse_to_unicode(data: &[u8]) -> Self {
        let mut cmap = Self::parse(data);
        cmap.unicode_parent = cmap.parent.as_deref().and_then(adobe);
        cmap
    }

    pub fn parse(data: &[u8]) -> Self {
        let tokens = tokenize(data);
        let mut cmap = Self::default();
        let mut index = 0;
        while index < tokens.len() {
            match &tokens[index] {
                Token::Keyword(k) if k == "begincodespacerange" => {
                    index += 1;
                    while let (Some(Token::Hex(low)), Some(Token::Hex(high))) =
                        (tokens.get(index), tokens.get(index + 1))
                    {
                        cmap.add_codespace(low, high);
                        index += 2;
                    }
                }
                Token::Keyword(k) if k == "begincidrange" => {
                    index += 1;
                    while let (
                        Some(Token::Hex(low)),
                        Some(Token::Hex(high)),
                        Some(Token::Number(cid)),
                    ) = (
                        tokens.get(index),
                        tokens.get(index + 1),
                        tokens.get(index + 2),
                    ) {
                        let (len, low, high) = (code_of(low).len, be(low), be(high));
                        if low <= high && *cid >= 0.0 {
                            cmap.cid_ranges.push((len, low, high, *cid as u32));
                        }
                        index += 3;
                    }
                }
                Token::Keyword(k) if k == "begincidchar" => {
                    index += 1;
                    while let (Some(Token::Hex(code)), Some(Token::Number(cid))) =
                        (tokens.get(index), tokens.get(index + 1))
                    {
                        let code = code_of(code);
                        if *cid >= 0.0 {
                            cmap.cid_ranges
                                .push((code.len, code.value, code.value, *cid as u32));
                        }
                        index += 2;
                    }
                }
                Token::Keyword(k) if k == "beginbfchar" => {
                    index += 1;
                    while let (Some(Token::Hex(code)), Some(destination)) =
                        (tokens.get(index), tokens.get(index + 1))
                    {
                        let Some(text) = destination_text(destination) else {
                            break;
                        };
                        cmap.unicode.insert(code_of(code), text);
                        index += 2;
                    }
                }
                Token::Keyword(k) if k == "beginbfrange" => {
                    index += 1;
                    while let (Some(Token::Hex(low)), Some(Token::Hex(high)), Some(destination)) = (
                        tokens.get(index),
                        tokens.get(index + 1),
                        tokens.get(index + 2),
                    ) {
                        cmap.add_bf_range(low, high, destination);
                        index += 3;
                    }
                }
                Token::Name(name) if name == "WMode" => {
                    if let Some(Token::Number(mode)) = tokens.get(index + 1) {
                        cmap.vertical = *mode == 1.0;
                    }
                    index += 1;
                }
                Token::Keyword(k) if k == "usecmap" => {
                    if let Some(Token::Name(name)) =
                        index.checked_sub(1).and_then(|i| tokens.get(i))
                    {
                        cmap.parent = Some(name.clone());
                    }
                    index += 1;
                }
                _ => index += 1,
            }
        }
        cmap
    }

    /// An encoding CMap written with bf operators maps codes to CIDs, as
    /// MuPDF reads it; its explicit CID mappings stay first.
    pub fn bf_as_cids(&mut self) {
        let mut ranges: Vec<(u8, u32, u32, u32)> = std::mem::take(&mut self.unicode)
            .into_iter()
            .filter_map(|(code, text)| {
                let mut units = text.encode_utf16();
                match (units.next(), units.next()) {
                    (Some(unit), None) => Some((code.len, code.value, code.value, unit as u32)),
                    _ => None,
                }
            })
            .collect();
        ranges.append(&mut self.cid_ranges);
        self.cid_ranges = ranges;
    }

    fn add_bf_range(&mut self, low: &[u8], high: &[u8], destination: &Token) {
        let len = code_of(low).len;
        let (first, last) = (be(low), be(high));
        if first > last || last - first > 0xFFFF {
            return;
        }
        match destination {
            Token::Array(items) => {
                for (offset, item) in items.iter().enumerate() {
                    let value = first + offset as u32;
                    if value > last {
                        break;
                    }
                    if let Some(text) = destination_text(item) {
                        self.unicode.insert(Code { value, len }, text);
                    }
                }
            }
            Token::Hex(start) if start.len() >= 2 => {
                let mut units = utf16_units(start);
                for value in first..=last {
                    self.unicode
                        .insert(Code { value, len }, String::from_utf16_lossy(&units));
                    if let Some(last_unit) = units.last_mut() {
                        *last_unit = last_unit.wrapping_add(1);
                    }
                }
            }
            Token::Hex(start) => {
                let mut byte = start.first().copied().unwrap_or(0);
                for value in first..=last {
                    self.unicode
                        .insert(Code { value, len }, char::from(byte).to_string());
                    byte = byte.wrapping_add(1);
                }
            }
            _ => {}
        }
    }
}

fn be(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0u32, |acc, b| (acc << 8) | u32::from(*b))
}

fn code_of(bytes: &[u8]) -> Code {
    Code {
        value: be(bytes),
        len: bytes.len().clamp(1, 4) as u8,
    }
}

fn utf16_units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks(2)
        .map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
        .collect()
}

fn destination_text(token: &Token) -> Option<String> {
    match token {
        Token::Hex(bytes) if bytes.len() == 1 => Some(char::from(bytes[0]).to_string()),
        Token::Hex(bytes) => Some(String::from_utf16_lossy(&utf16_units(bytes))),
        Token::Name(name) => super::glyph_name_to_text(name),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Hex(Vec<u8>),
    Number(f64),
    Name(String),
    Keyword(String),
    Array(Vec<Token>),
    Other,
}

fn tokenize(data: &[u8]) -> Vec<Token> {
    let mut position = 0;
    let mut stack: Vec<Vec<Token>> = vec![Vec::new()];
    while position < data.len() {
        let byte = data[position];
        let token = match byte {
            b'%' => {
                while position < data.len() && !matches!(data[position], b'\n' | b'\r') {
                    position += 1;
                }
                continue;
            }
            b'<' | b'>' if data.get(position + 1) == Some(&byte) => {
                position += 2;
                Token::Other
            }
            b'<' => {
                let start = position + 1;
                let end = data[start..]
                    .iter()
                    .position(|b| *b == b'>')
                    .map_or(data.len(), |e| start + e);
                position = end + 1;
                Token::Hex(hex(&data[start..end]))
            }
            b'(' => {
                let (bytes, end) = literal(data, position + 1);
                position = end;
                Token::Hex(bytes)
            }
            b'[' => {
                position += 1;
                stack.push(Vec::new());
                continue;
            }
            b']' => {
                position += 1;
                if stack.len() == 1 {
                    continue;
                }
                Token::Array(stack.pop().unwrap_or_default())
            }
            b'/' => {
                let start = position + 1;
                position = start;
                while position < data.len() && !is_break(data[position]) {
                    position += 1;
                }
                Token::Name(String::from_utf8_lossy(&data[start..position]).into_owned())
            }
            _ if is_break(byte) => {
                position += 1;
                continue;
            }
            _ => {
                let start = position;
                while position < data.len() && !is_break(data[position]) {
                    position += 1;
                }
                let word = String::from_utf8_lossy(&data[start..position]).into_owned();
                match word.parse::<f64>() {
                    Ok(number) => Token::Number(number),
                    Err(_) => Token::Keyword(word),
                }
            }
        };
        if let Some(top) = stack.last_mut() {
            top.push(token);
        }
    }
    while stack.len() > 1 {
        let items = stack.pop().unwrap_or_default();
        if let Some(top) = stack.last_mut() {
            top.push(Token::Array(items));
        }
    }
    stack.pop().unwrap_or_default()
}

fn literal(data: &[u8], mut position: usize) -> (Vec<u8>, usize) {
    let mut depth = 0usize;
    let mut bytes = Vec::new();
    while position < data.len() {
        match data[position] {
            b'\\' => {
                position += 1;
                if let Some(escaped) = data.get(position) {
                    bytes.push(*escaped);
                }
            }
            b'(' => {
                depth += 1;
                bytes.push(b'(');
            }
            b')' if depth == 0 => return (bytes, position + 1),
            b')' => {
                depth -= 1;
                bytes.push(b')');
            }
            other => bytes.push(other),
        }
        position += 1;
    }
    (bytes, position)
}

fn is_break(byte: u8) -> bool {
    matches!(
        byte,
        b' ' | b'\t'
            | b'\n'
            | b'\r'
            | 0x0C
            | 0x00
            | b'/'
            | b'['
            | b']'
            | b'<'
            | b'>'
            | b'('
            | b')'
            | b'{'
            | b'}'
            | b'%'
    )
}

fn hex(data: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = data
        .iter()
        .filter_map(|b| (*b as char).to_digit(16).map(|d| d as u8))
        .collect();
    digits
        .chunks(2)
        .map(|pair| (pair[0] << 4) | pair.get(1).copied().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{CMap, Code};

    fn code(value: u32, len: u8) -> Code {
        Code { value, len }
    }

    #[test]
    fn codes_split_by_code_space_of_mixed_widths() {
        let cmap = CMap::parse(b"2 begincodespacerange <00> <80> <8140> <9FFC> endcodespacerange");
        let bytes = [0x41, 0x81, 0x40, 0x42];
        let codes = [
            cmap.next_code(&bytes, 0),
            cmap.next_code(&bytes, 1),
            cmap.next_code(&bytes, 3),
        ];
        assert_eq!(codes, [code(0x41, 1), code(0x8140, 2), code(0x42, 1)]);
    }

    #[test]
    fn a_byte_outside_every_range_takes_the_narrowest_width() {
        let cmap = CMap::parse(b"1 begincodespacerange <0000> <FFFF> endcodespacerange");
        assert_eq!(cmap.next_code(&[0x12, 0x34], 0), code(0x1234, 2));
        assert_eq!(cmap.next_code(&[0x12], 0), code(0x12, 1));
    }

    #[test]
    fn bfrange_increments_the_last_unit_and_arrays_list_each_code() {
        let cmap = CMap::parse(
            b"beginbfrange <0010> <0012> <0041> <0020> <0021> [<00660069> <D835DC00>] endbfrange",
        );
        assert_eq!(cmap.unicode(code(0x12, 2)).as_deref(), Some("C"));
        assert_eq!(cmap.unicode(code(0x20, 2)).as_deref(), Some("fi"));
        assert_eq!(cmap.unicode(code(0x21, 2)).as_deref(), Some("\u{1D400}"));
    }

    #[test]
    fn bfrange_on_a_surrogate_pair_steps_the_low_half() {
        let cmap = CMap::parse(b"beginbfrange <01> <02> <D835DC00> endbfrange");
        assert_eq!(cmap.unicode(code(0x02, 1)).as_deref(), Some("\u{1D401}"));
    }

    #[test]
    fn a_one_byte_code_finds_its_text_written_on_two() {
        let cmap = CMap::parse(b"beginbfchar <0041> <0042> endbfchar");
        assert_eq!(cmap.unicode(code(0x41, 1)).as_deref(), Some("B"));
    }

    #[test]
    fn cid_ranges_vertical_mode_and_parent_are_read() {
        let cmap = CMap::parse(
            b"/WMode 1 def /Base-H usecmap begincidrange <20> <7E> 1 endcidrange begincidchar <8140> 633 endcidchar",
        );
        assert!(cmap.vertical);
        assert_eq!(cmap.parent.as_deref(), Some("Base-H"));
        assert_eq!(cmap.cid(code(0x41, 1)), Some(34));
        assert_eq!(cmap.cid(code(0x8140, 2)), Some(633));
        assert_eq!(cmap.cid(code(0x41, 2)), None);
    }

    #[test]
    fn predefined_shift_jis_splits_single_and_double_bytes() {
        let cmap = CMap::predefined("90ms-RKSJ-H").unwrap();
        let bytes = [0x82, 0xA0, 0xB1, 0x41];
        let codes = [
            cmap.next_code(&bytes, 0),
            cmap.next_code(&bytes, 2),
            cmap.next_code(&bytes, 3),
        ];
        assert_eq!(codes, [code(0x82A0, 2), code(0xB1, 1), code(0x41, 1)]);
        assert!(CMap::predefined("Identity-V").unwrap().vertical);
        assert!(CMap::predefined("Unknown-Registry-H").is_none());
    }

    #[test]
    fn predefined_cmap_reads_cids_and_the_collection_names_them() {
        let cmap = CMap::predefined("90ms-RKSJ-H").unwrap();
        let cid = cmap.cid(code(0x82A0, 2)).unwrap();
        assert_eq!(
            super::collection_text("Adobe", "Japan1", cid).as_deref(),
            Some("\u{3042}")
        );
        assert_eq!(super::collection_text("Adobe", "Identity", cid), None);
    }

    #[test]
    fn a_to_unicode_built_on_an_adobe_table_reads_what_it_leaves_to_it() {
        let cmap = CMap::parse_to_unicode(
            b"/Adobe-Korea1-UCS2 usecmap 1 beginbfchar <46FE> <0033> endbfchar",
        );
        assert_eq!(cmap.unicode(code(0x46FE, 2)).as_deref(), Some("3"));
        assert_eq!(cmap.unicode(code(0x0A4B, 2)).as_deref(), Some("\u{C778}"));
    }
}

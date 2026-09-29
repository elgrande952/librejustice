//! A tolerant reading of HTML as browsers and Word write it: tags, character
//! data and markup cut into segments that cover the whole source, then nested
//! into elements the way a browser closes what the markup leaves open.

use std::ops::Range;

use crate::MAX_DEPTH;

pub(crate) struct Attribute {
    pub name: String,
    pub value: String,
    /// Where the raw value sits in the source, quotes excluded.
    pub span: Option<Range<usize>>,
}

pub(crate) enum Token {
    /// Character data, as the number of the text node it is.
    Text(u32),
    Open {
        name: String,
        attributes: Vec<Attribute>,
        closed: bool,
    },
    Close(String),
    /// A comment, a doctype, a conditional marker or raw text: nothing to read.
    Markup,
}

pub(crate) struct Segment {
    pub span: Range<usize>,
    pub token: Token,
}

pub(crate) struct Tokens {
    pub segments: Vec<Segment>,
    /// Each text node's span in the source and its text, entities decoded.
    pub nodes: Vec<(Range<usize>, String)>,
    /// The content of every `<style>` element, in order.
    pub styles: String,
}

fn find_ignore_case(s: &str, needle: &str, from: usize) -> Option<usize> {
    let n = needle.as_bytes();
    s.as_bytes()
        .get(from..)?
        .windows(n.len())
        .position(|w| w.eq_ignore_ascii_case(n))
        .map(|at| from + at)
}

fn is_name_char(c: u8) -> bool {
    !c.is_ascii_whitespace() && !matches!(c, b'/' | b'>' | b'=' | b'<')
}

/// Where a `<` at `at` opens markup rather than standing for itself.
fn opens_markup(s: &[u8], at: usize) -> bool {
    matches!(s.get(at + 1), Some(c) if c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?'))
}

pub(crate) fn tokenize(source: &str) -> Tokens {
    let bytes = source.as_bytes();
    let mut out = Tokens {
        segments: Vec::new(),
        nodes: Vec::new(),
        styles: String::new(),
    };
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'<' || !opens_markup(bytes, at) {
            let mut end = at + 1;
            while end < bytes.len() && !(bytes[end] == b'<' && opens_markup(bytes, end)) {
                end += 1;
            }
            text(&mut out, source, at..end);
            at = end;
            continue;
        }
        let rest = &source[at..];
        let end_of = |close: &str, from: usize| {
            source[from..]
                .find(close)
                .map_or(bytes.len(), |i| from + i + close.len())
        };
        if rest.starts_with("<!--[if") {
            let end = end_of("]>", at);
            markup(&mut out, at..end);
            at = end;
        } else if rest.starts_with("<!--") {
            let end = end_of("-->", at + 4);
            markup(&mut out, at..end);
            at = end;
        } else if rest.starts_with("<![CDATA[") {
            let end = end_of("]]>", at);
            markup(&mut out, at..end);
            at = end;
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            let end = end_of(">", at);
            markup(&mut out, at..end);
            at = end;
        } else if rest.starts_with("</") {
            let mut name_end = at + 2;
            while name_end < bytes.len() && is_name_char(bytes[name_end]) {
                name_end += 1;
            }
            let name = source[at + 2..name_end].to_ascii_lowercase();
            let end = end_of(">", name_end);
            out.segments.push(Segment {
                span: at..end,
                token: Token::Close(name),
            });
            at = end;
        } else {
            at = open_tag(&mut out, source, at);
        }
    }
    out
}

fn markup(out: &mut Tokens, span: Range<usize>) {
    if !span.is_empty() {
        out.segments.push(Segment {
            span,
            token: Token::Markup,
        });
    }
}

fn text(out: &mut Tokens, source: &str, span: Range<usize>) {
    let node = out.nodes.len() as u32;
    out.nodes
        .push((span.clone(), decode(&source[span.clone()])));
    out.segments.push(Segment {
        span,
        token: Token::Text(node),
    });
}

/// Read the tag opening at `at`, and the raw text of a `<script>` or a
/// `<style>` after it; return where reading resumes.
fn open_tag(out: &mut Tokens, source: &str, at: usize) -> usize {
    let bytes = source.as_bytes();
    let mut i = at + 1;
    while i < bytes.len() && is_name_char(bytes[i]) {
        i += 1;
    }
    let name = source[at + 1..i].to_ascii_lowercase();
    let mut attributes = Vec::new();
    let mut closed = false;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        match bytes.get(i) {
            None => break,
            Some(b'>') => {
                i += 1;
                break;
            }
            Some(b'/') => {
                closed = bytes.get(i + 1) == Some(&b'>');
                i += 1;
                continue;
            }
            _ => {}
        }
        let start = i;
        while i < bytes.len() && is_name_char(bytes[i]) {
            i += 1;
        }
        if i == start {
            i += 1;
            continue;
        }
        let attribute = source[start..i].to_ascii_lowercase();
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if bytes.get(j) != Some(&b'=') {
            attributes.push(Attribute {
                name: attribute,
                value: String::new(),
                span: None,
            });
            continue;
        }
        j += 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        let span = match bytes.get(j) {
            Some(&q @ (b'"' | b'\'')) => {
                let end = source[j + 1..]
                    .find(q as char)
                    .map_or(bytes.len(), |e| j + 1 + e);
                i = (end + 1).min(bytes.len());
                j + 1..end
            }
            _ => {
                let mut end = j;
                while end < bytes.len() && !bytes[end].is_ascii_whitespace() && bytes[end] != b'>' {
                    end += 1;
                }
                i = end;
                j..end
            }
        };
        attributes.push(Attribute {
            name: attribute,
            value: decode(&source[span.clone()]),
            span: Some(span),
        });
    }
    let raw = matches!(
        name.as_str(),
        "script" | "style" | "xmp" | "textarea" | "title"
    );
    out.segments.push(Segment {
        span: at..i,
        token: Token::Open {
            name: name.clone(),
            attributes,
            closed,
        },
    });
    if !raw || closed {
        return i;
    }
    let content_end = find_ignore_case(source, &format!("</{name}"), i).unwrap_or(bytes.len());
    match name.as_str() {
        "title" | "textarea" => {
            if content_end > i {
                text(out, source, i..content_end);
            }
        }
        _ => {
            if name == "style" {
                out.styles.push_str(&source[i..content_end]);
                out.styles.push('\n');
            }
            markup(out, i..content_end);
        }
    }
    content_end
}

/// The entities of HTML 4 from U+00A0 to U+00FF, in order.
const LATIN_1: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf",
    "laquo", "not", "shy", "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro",
    "para", "middot", "cedil", "sup1", "ordm", "raquo", "frac14", "frac12", "frac34", "iquest",
    "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig", "Ccedil", "Egrave", "Eacute",
    "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve", "Oacute",
    "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute",
    "THORN", "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil",
    "egrave", "eacute", "ecirc", "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde",
    "ograve", "oacute", "ocirc", "otilde", "ouml", "divide", "oslash", "ugrave", "uacute", "ucirc",
    "uuml", "yacute", "thorn", "yuml",
];

const OTHERS: &[(&str, u32)] = &[
    ("quot", 0x22),
    ("amp", 0x26),
    ("apos", 0x27),
    ("lt", 0x3C),
    ("gt", 0x3E),
    ("OElig", 0x152),
    ("oelig", 0x153),
    ("Scaron", 0x160),
    ("scaron", 0x161),
    ("Yuml", 0x178),
    ("fnof", 0x192),
    ("circ", 0x2C6),
    ("tilde", 0x2DC),
    ("ensp", 0x2002),
    ("emsp", 0x2003),
    ("thinsp", 0x2009),
    ("zwnj", 0x200C),
    ("zwj", 0x200D),
    ("lrm", 0x200E),
    ("rlm", 0x200F),
    ("ndash", 0x2013),
    ("mdash", 0x2014),
    ("lsquo", 0x2018),
    ("rsquo", 0x2019),
    ("sbquo", 0x201A),
    ("ldquo", 0x201C),
    ("rdquo", 0x201D),
    ("bdquo", 0x201E),
    ("dagger", 0x2020),
    ("Dagger", 0x2021),
    ("bull", 0x2022),
    ("hellip", 0x2026),
    ("permil", 0x2030),
    ("prime", 0x2032),
    ("Prime", 0x2033),
    ("lsaquo", 0x2039),
    ("rsaquo", 0x203A),
    ("euro", 0x20AC),
    ("trade", 0x2122),
    ("larr", 0x2190),
    ("rarr", 0x2192),
    ("harr", 0x2194),
    ("minus", 0x2212),
];

fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let value = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        // A reference in the C1 range names the windows-1252 character, as browsers read it.
        if (0x80..0xA0).contains(&value) {
            let byte = [value as u8];
            let (decoded, _) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(&byte);
            return decoded.chars().next();
        }
        return char::from_u32(value);
    }
    if let Some(i) = LATIN_1.iter().position(|n| *n == name) {
        return char::from_u32(0xA0 + i as u32);
    }
    OTHERS
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, c)| char::from_u32(*c))
}

/// `raw` with its character references replaced by what they name; one that
/// names nothing stays as written.
pub(crate) fn decode(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest[1..]
            .find(';')
            .filter(|&end| end > 0 && end <= 32)
            .and_then(|end| entity(&rest[1..1 + end]).map(|c| (c, end + 2)));
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `text` written as character data.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{A0}' => out.push_str("&nbsp;"),
            c => out.push(c),
        }
    }
    out
}

pub(crate) enum Node {
    Element(Element),
    Text(u32),
}

pub(crate) struct Element {
    pub name: String,
    /// Index of the segment of its opening tag.
    pub segment: usize,
    pub children: Vec<Node>,
}

const VOID: &[&str] = &[
    "area", "base", "basefont", "br", "col", "embed", "frame", "hr", "img", "input", "isindex",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// Opening one of these closes an open paragraph.
const CLOSES_PARAGRAPH: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "dd",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];

/// The elements `tokens` nest into, the whole document as the children of
/// one root.
pub(crate) fn tree(tokens: &Tokens) -> Vec<Node> {
    let mut stack: Vec<Element> = vec![Element {
        name: String::new(),
        segment: usize::MAX,
        children: Vec::new(),
    }];
    fn pop(stack: &mut Vec<Element>) {
        let element = stack.pop().expect("the root is never closed");
        stack
            .last_mut()
            .expect("the root is never closed")
            .children
            .push(Node::Element(element));
    }
    /// Close the innermost `names` open above the nearest `bounds`.
    fn close(stack: &mut Vec<Element>, names: &[&str], bounds: &[&str]) {
        let found = stack
            .iter()
            .enumerate()
            .skip(1)
            .rev()
            .take_while(|(_, e)| !bounds.contains(&e.name.as_str()))
            .find(|(_, e)| names.contains(&e.name.as_str()))
            .map(|(i, _)| i);
        if let Some(i) = found {
            while stack.len() > i {
                pop(stack);
            }
        }
    }
    const TABLE: &[&str] = &["table", "td", "th", "caption"];
    for (index, segment) in tokens.segments.iter().enumerate() {
        match &segment.token {
            Token::Text(node) => stack
                .last_mut()
                .expect("the root is never closed")
                .children
                .push(Node::Text(*node)),
            Token::Markup => {}
            Token::Open { name, closed, .. } => {
                let name = name.as_str();
                if CLOSES_PARAGRAPH.contains(&name) {
                    close(&mut stack, &["p"], TABLE);
                }
                match name {
                    "li" => close(&mut stack, &["li"], &["ul", "ol", "table", "td", "th"]),
                    "dt" | "dd" => close(&mut stack, &["dt", "dd"], &["dl", "table", "td", "th"]),
                    "tr" => close(&mut stack, &["tr"], &["table", "tbody", "thead", "tfoot"]),
                    "td" | "th" => close(&mut stack, &["td", "th"], &["tr", "table"]),
                    "tbody" | "thead" | "tfoot" => {
                        close(&mut stack, &["tbody", "thead", "tfoot"], &["table"])
                    }
                    _ => {}
                }
                let element = Element {
                    name: name.to_string(),
                    segment: index,
                    children: Vec::new(),
                };
                if *closed || VOID.contains(&name) || stack.len() >= MAX_DEPTH {
                    stack
                        .last_mut()
                        .expect("the root is never closed")
                        .children
                        .push(Node::Element(element));
                } else {
                    stack.push(element);
                }
            }
            Token::Close(name) => {
                let bounds: &[&str] = match name.as_str() {
                    "td" | "th" | "tr" | "tbody" | "thead" | "tfoot" | "table" | "caption" => {
                        &["table"]
                    }
                    _ => TABLE,
                };
                let bounds: Vec<&str> = bounds.iter().copied().filter(|b| b != name).collect();
                close(&mut stack, &[name.as_str()], &bounds);
            }
        }
    }
    while stack.len() > 1 {
        pop(&mut stack);
    }
    stack.pop().expect("the root").children
}

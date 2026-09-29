//! One XML part as an element tree whose leaves are the byte spans an edit can
//! rewrite: character data, the ODF space, tab and line-break elements, and
//! the attribute values that carry free text.

use std::collections::BTreeMap;
use std::ops::Range;

use anyhow::{anyhow, bail, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::NodeEdit;

/// The namespaces a reader tells apart, whatever prefix a producer bound them to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ns {
    Office,
    Style,
    Text,
    Table,
    Draw,
    Fo,
    Xlink,
    Dc,
    Meta,
    Number,
    Svg,
    Chart,
    Form,
    Script,
    Config,
    Manifest,
    Loext,
    Officeooo,
    Xhtml,
    Presentation,
    Xml,
    /// Word's form fields, as Writer writes them.
    Field,
    /// The statements of a paragraph signature, in an RDF metadata file.
    ParagraphSignature,
    Other,
}

fn ns_of(uri: &[u8]) -> Ns {
    const ODF: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:";
    if let Some(rest) = uri.strip_prefix(ODF) {
        let name = rest.split(|b| *b == b':').next().unwrap_or_default();
        return match name {
            b"office" => Ns::Office,
            b"style" => Ns::Style,
            b"text" => Ns::Text,
            b"table" => Ns::Table,
            b"drawing" => Ns::Draw,
            b"xsl-fo-compatible" => Ns::Fo,
            b"meta" => Ns::Meta,
            b"datastyle" => Ns::Number,
            b"svg-compatible" => Ns::Svg,
            b"chart" => Ns::Chart,
            b"form" => Ns::Form,
            b"script" => Ns::Script,
            b"config" => Ns::Config,
            b"manifest" => Ns::Manifest,
            b"presentation" => Ns::Presentation,
            _ => Ns::Other,
        };
    }
    match uri {
        // The drafts before ODF 1.0 named the W3C namespaces, which Writer still reads.
        b"http://www.w3.org/1999/XSL/Format" => Ns::Fo,
        b"http://www.w3.org/2000/svg" => Ns::Svg,
        b"http://www.w3.org/1999/xlink" => Ns::Xlink,
        b"http://purl.org/dc/elements/1.1/" => Ns::Dc,
        b"urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0" => Ns::Loext,
        b"http://openoffice.org/2009/office" => Ns::Officeooo,
        b"http://www.w3.org/1999/xhtml" => Ns::Xhtml,
        b"http://www.w3.org/XML/1998/namespace" => Ns::Xml,
        b"urn:openoffice:names:experimental:ooo-ms-interop:xmlns:field:1.0" => Ns::Field,
        _ if uri.starts_with(b"urn:bails:loext:paragraph:signature:") => Ns::ParagraphSignature,
        _ => Ns::Other,
    }
}

/// What a leaf span is, which decides how it is written back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Character data, escaped in the part.
    Chars,
    /// One entity or character reference.
    Ref,
    /// A CDATA section.
    CData,
    /// `text:s`, as many spaces as it counts.
    Spaces,
    /// `text:tab`.
    Tab,
    /// `text:line-break`.
    LineBreak,
    /// The value of an attribute that carries free text.
    Attr,
    Comment,
}

pub struct Node {
    pub span: Range<u32>,
    pub text: String,
    pub kind: Kind,
}

pub struct Attr {
    pub ns: Ns,
    pub local: String,
    pub value: String,
    /// The leaf that holds this value when it is free text.
    pub node: Option<u32>,
}

pub enum Child {
    Element(u32),
    Node(u32),
}

pub struct Element {
    pub ns: Ns,
    pub local: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Child>,
    /// From the `<` of the start tag to the `>` of the end tag.
    pub span: Range<u32>,
}

impl Element {
    pub fn is(&self, ns: Ns, local: &str) -> bool {
        self.ns == ns && self.local == local
    }

    pub fn attr(&self, ns: Ns, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.ns == ns && a.local == local)
            .map(|a| a.value.as_str())
    }
}

pub struct Xml {
    pub nodes: Vec<Node>,
    pub elements: Vec<Element>,
    pub root: Option<u32>,
}

/// Attributes whose value is written by a person rather than chosen from a
/// vocabulary. Names and the references to them are rewritten alike, so a
/// bookmark and the field that points at it stay paired.
fn free_text(element: (Ns, &str), ns: Ns, local: &str) -> bool {
    if matches!(
        ns,
        Ns::Config | Ns::Manifest | Ns::Fo | Ns::Number | Ns::Xml | Ns::Svg
    ) {
        return false;
    }
    if local.starts_with("font-") || element == (Ns::Style, "font-face") {
        return false;
    }
    match local {
        "href" => !matches!(
            element,
            (Ns::Draw, "image")
                | (Ns::Draw, "object")
                | (Ns::Draw, "object-ole")
                | (Ns::Draw, "plugin")
                | (Ns::Draw, "applet")
                | (Ns::Draw, "fill-image")
                | (Ns::Style, "background-image")
                | (Ns::Text, "list-level-style-image")
                | (Ns::Loext, "graphic")
        ),
        "value" => matches!(element, (Ns::Text, "label") | (_, "param")),
        "name" | "title" | "description" | "comment" | "creator" | "author" | "label" | "key1"
        | "key2" | "formula" | "condition" | "initials" | "content" | "alias" | "tag" => true,
        _ if element == (Ns::Text, "bibliography-mark") => local != "bibliography-type",
        _ => {
            local.ends_with("-name")
                || local.starts_with("string-value")
                || local.ends_with("-phonetic")
        }
    }
}

fn is_ws(text: &str) -> bool {
    text.bytes()
        .all(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
}

struct Scope {
    bindings: Vec<(Vec<u8>, Ns)>,
    default: Ns,
}

struct Builder<'a> {
    input: &'a [u8],
    nodes: Vec<Node>,
    elements: Vec<Element>,
    stack: Vec<u32>,
    scopes: Vec<Scope>,
    root: Option<u32>,
    /// A `text:s`, `text:tab` or `text:line-break` written with an end tag, whose
    /// leaf must cover the end tag too.
    open_leaf: Option<(u32, usize)>,
}

impl<'a> Builder<'a> {
    fn resolve(&self, prefix: Option<&[u8]>) -> Ns {
        match prefix {
            None => self.scopes.last().map(|s| s.default).unwrap_or(Ns::Other),
            Some(b"xml") => Ns::Xml,
            Some(p) => self
                .scopes
                .iter()
                .rev()
                .find_map(|s| s.bindings.iter().find(|(q, _)| q == p).map(|(_, n)| *n))
                .unwrap_or(Ns::Other),
        }
    }

    fn offset_of(&self, slice: &[u8]) -> Result<usize> {
        let base = self.input.as_ptr() as usize;
        let at = slice.as_ptr() as usize;
        if at < base || at + slice.len() > base + self.input.len() {
            return Err(anyhow!("an attribute value does not point into the part"));
        }
        Ok(at - base)
    }

    fn push_node(&mut self, span: Range<usize>, text: String, kind: Kind) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            span: span.start as u32..span.end as u32,
            text,
            kind,
        });
        if let Some(parent) = self.stack.last() {
            self.elements[*parent as usize]
                .children
                .push(Child::Node(id));
        }
        id
    }

    fn open(&mut self, e: &BytesStart<'a>, start: usize, end: usize, empty: bool) -> Result<()> {
        let mut scope = Scope {
            bindings: Vec::new(),
            default: self.scopes.last().map(|s| s.default).unwrap_or(Ns::Other),
        };
        let mut raw = Vec::new();
        for attr in e.attributes().with_checks(false) {
            let attr = attr.map_err(|e| anyhow!("reading an attribute: {e}"))?;
            let key = attr.key.as_ref();
            if key == b"xmlns" {
                scope.default = ns_of(&attr.value);
            } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
                scope.bindings.push((prefix.to_vec(), ns_of(&attr.value)));
            } else {
                let value = match &attr.value {
                    std::borrow::Cow::Borrowed(v) => *v,
                    std::borrow::Cow::Owned(_) => {
                        return Err(anyhow!("an attribute value was copied out of the part"))
                    }
                };
                raw.push((key.to_vec(), value));
            }
        }
        self.scopes.push(scope);

        let qname = e.name();
        let (prefix, local) = split(qname.as_ref());
        let ns = self.resolve(prefix);
        let local = String::from_utf8_lossy(local).into_owned();

        let leaf = match (ns, local.as_str()) {
            (Ns::Text, "s") => Some(Kind::Spaces),
            (Ns::Text, "tab") => Some(Kind::Tab),
            (Ns::Text, "line-break") => Some(Kind::LineBreak),
            _ => None,
        };

        let mut attrs = Vec::with_capacity(raw.len());
        for (key, value) in raw {
            let (p, l) = split(&key);
            let a_ns = match p {
                Some(p) => self.resolve(Some(p)),
                None => Ns::Other,
            };
            let a_local = String::from_utf8_lossy(l).into_owned();
            let text = unescape(value)?;
            let node =
                if leaf.is_none() && !text.is_empty() && free_text((ns, &local), a_ns, &a_local) {
                    let at = self.offset_of(value)?;
                    let id = self.nodes.len() as u32;
                    self.nodes.push(Node {
                        span: at as u32..(at + value.len()) as u32,
                        text: text.clone(),
                        kind: Kind::Attr,
                    });
                    Some(id)
                } else {
                    None
                };
            attrs.push(Attr {
                ns: a_ns,
                local: a_local,
                value: text,
                node,
            });
        }

        if let Some(kind) = leaf {
            let text = match kind {
                Kind::Spaces => {
                    let count = attrs
                        .iter()
                        .find(|a| a.ns == Ns::Text && a.local == "c")
                        .and_then(|a| a.value.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                        .clamp(1, 10_000);
                    " ".repeat(count)
                }
                Kind::Tab => "\t".to_string(),
                _ => "\n".to_string(),
            };
            let id = self.push_node(start..end, text, kind);
            if empty {
                self.scopes.pop();
            } else {
                self.open_leaf = Some((id, self.stack.len()));
                // Its end tag closes it; nothing is pushed on the element stack,
                // so the scope is popped there.
            }
            return Ok(());
        }

        let id = self.elements.len() as u32;
        self.elements.push(Element {
            ns,
            local,
            attrs,
            children: Vec::new(),
            span: start as u32..end as u32,
        });
        match self.stack.last() {
            Some(parent) => self.elements[*parent as usize]
                .children
                .push(Child::Element(id)),
            None if self.root.is_none() => self.root = Some(id),
            None => return Err(anyhow!("a second root element")),
        }
        if empty {
            self.scopes.pop();
        } else {
            if self.stack.len() == crate::MAX_DEPTH {
                bail!("the part nests elements deeper than {}", crate::MAX_DEPTH);
            }
            self.stack.push(id);
        }
        Ok(())
    }

    fn close(&mut self, end: usize) -> Result<()> {
        if let Some((leaf, depth)) = self.open_leaf {
            if depth == self.stack.len() {
                self.nodes[leaf as usize].span.end = end as u32;
                self.open_leaf = None;
                self.scopes.pop();
                return Ok(());
            }
        }
        let id = self
            .stack
            .pop()
            .ok_or_else(|| anyhow!("an end tag without its start"))?;
        self.elements[id as usize].span.end = end as u32;
        self.scopes.pop();
        Ok(())
    }
}

fn split(qname: &[u8]) -> (Option<&[u8]>, &[u8]) {
    match qname.iter().position(|b| *b == b':') {
        Some(i) => (Some(&qname[..i]), &qname[i + 1..]),
        None => (None, qname),
    }
}

fn unescape(raw: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(raw).map_err(|e| anyhow!("a part is not UTF-8: {e}"))?;
    Ok(quick_xml::escape::unescape(text)
        .map_err(|e| anyhow!("unescaping: {e}"))?
        .into_owned())
}

fn resolve_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

impl Xml {
    pub fn parse(input: &[u8]) -> Result<Self> {
        // The reader skips a byte-order mark without counting it in its positions.
        let bom = if input.starts_with(b"\xEF\xBB\xBF") {
            3
        } else {
            0
        };
        let mut reader = Reader::from_reader(&input[bom..]);
        reader.config_mut().trim_text(false);
        reader.config_mut().check_end_names = false;
        let mut b = Builder {
            input,
            nodes: Vec::new(),
            elements: Vec::new(),
            stack: Vec::new(),
            scopes: Vec::new(),
            root: None,
            open_leaf: None,
        };
        loop {
            let start = bom + reader.buffer_position() as usize;
            let event = reader
                .read_event()
                .map_err(|e| anyhow!("reading XML at byte {start}: {e}"))?;
            let end = bom + reader.buffer_position() as usize;
            match event {
                Event::Eof => break,
                Event::Start(e) => b.open(&e, start, end, false)?,
                Event::Empty(e) => b.open(&e, start, end, true)?,
                Event::End(_) => b.close(end)?,
                Event::Text(e) if !b.stack.is_empty() => {
                    let text = e
                        .decode()
                        .map_err(|e| anyhow!("decoding a text node: {e}"))?
                        .into_owned();
                    if b.open_leaf.is_none() {
                        b.push_node(start..end, text, Kind::Chars);
                    }
                }
                Event::CData(e) if !b.stack.is_empty() => {
                    let text = String::from_utf8_lossy(&e).into_owned();
                    b.push_node(start..end, text, Kind::CData);
                }
                Event::GeneralRef(e) if !b.stack.is_empty() => {
                    let name = e.decode().map_err(|e| anyhow!("decoding an entity: {e}"))?;
                    let resolved = match e.resolve_char_ref() {
                        Ok(Some(c)) => Some(c),
                        _ => resolve_entity(&name),
                    };
                    let text = resolved.map(String::from).unwrap_or_default();
                    if b.open_leaf.is_none() {
                        b.push_node(start..end, text, Kind::Ref);
                    }
                }
                Event::Comment(e) => {
                    let text = String::from_utf8_lossy(&e).into_owned();
                    if !is_ws(&text) {
                        let id = b.nodes.len() as u32;
                        b.nodes.push(Node {
                            span: (start + 4) as u32..(end - 3) as u32,
                            text,
                            kind: Kind::Comment,
                        });
                        let _ = id;
                    }
                }
                _ => {}
            }
        }
        if !b.stack.is_empty() {
            return Err(anyhow!("the part ends inside an element"));
        }
        Ok(Xml {
            nodes: b.nodes,
            elements: b.elements,
            root: b.root,
        })
    }

    pub fn element(&self, id: u32) -> &Element {
        &self.elements[id as usize]
    }

    pub fn root(&self) -> Option<&Element> {
        self.root.map(|r| self.element(r))
    }

    /// The child elements of `e`, in order.
    pub fn children<'s>(&'s self, e: &'s Element) -> impl Iterator<Item = &'s Element> + 's {
        e.children.iter().filter_map(move |c| match c {
            Child::Element(id) => Some(self.element(*id)),
            Child::Node(_) => None,
        })
    }

    pub fn child<'s>(&'s self, e: &'s Element, ns: Ns, local: &str) -> Option<&'s Element> {
        self.children(e).find(|c| c.is(ns, local))
    }

    /// Every character of character data under `e`, spaces and tabs included.
    pub fn text_of(&self, e: &Element) -> String {
        let mut out = String::new();
        self.collect_text(e, &mut out);
        out
    }

    fn collect_text(&self, e: &Element, out: &mut String) {
        for c in &e.children {
            match c {
                Child::Node(n) => out.push_str(&self.nodes[*n as usize].text),
                Child::Element(id) => self.collect_text(self.element(*id), out),
            }
        }
    }

    /// Rewrite `input`: every leaf in `edits` gets its edited text, and every
    /// range in `cuts` is replaced by its bytes. Leaves inside a cut are
    /// ignored; everything else survives byte for byte.
    pub(crate) fn rewrite(
        &self,
        input: &[u8],
        edits: &BTreeMap<u32, NodeEdit>,
        cuts: &[(Range<u32>, Vec<u8>)],
    ) -> Vec<u8> {
        let mut splices: Vec<(Range<u32>, Vec<u8>)> = Vec::new();
        for (node, edit) in edits {
            let node = &self.nodes[*node as usize];
            let bytes = match node.kind {
                Kind::Chars | Kind::CData | Kind::Attr | Kind::Ref => {
                    quick_xml::escape::escape(edit.apply(&node.text))
                        .into_owned()
                        .into_bytes()
                }
                Kind::Comment => edit
                    .apply(&node.text)
                    .replace("--", "- ")
                    .trim_end_matches('-')
                    .as_bytes()
                    .to_vec(),
                Kind::Tab | Kind::LineBreak | Kind::Spaces => self.markup(input, node, edit),
            };
            splices.push((node.span.clone(), bytes));
        }
        splices.extend(cuts.iter().cloned());
        splices.sort_by_key(|(r, _)| (r.start, std::cmp::Reverse(r.end)));
        let mut out = Vec::with_capacity(input.len());
        let mut at = 0u32;
        for (range, bytes) in splices {
            if range.start < at {
                continue;
            }
            out.extend_from_slice(&input[at as usize..range.start as usize]);
            out.extend_from_slice(&bytes);
            at = range.end;
        }
        out.extend_from_slice(&input[at as usize..]);
        out
    }

    /// A tab, line-break or space element after `edit`: the element for the
    /// characters kept, the text put before one escaped in front of it.
    fn markup(&self, input: &[u8], node: &Node, edit: &NodeEdit) -> Vec<u8> {
        let tag = &input[node.span.start as usize..node.span.end as usize];
        let element = |count: usize| match node.kind {
            Kind::Spaces => {
                let name_end = tag
                    .iter()
                    .position(|b| b.is_ascii_whitespace() || *b == b'/' || *b == b'>')
                    .unwrap_or(tag.len());
                let qname = String::from_utf8_lossy(&tag[1..name_end]).into_owned();
                let prefix = qname.split(':').next().unwrap_or("text");
                format!("<{qname} {prefix}:c=\"{count}\"/>").into_bytes()
            }
            _ => tag.to_vec(),
        };
        let mut out = Vec::new();
        let mut kept = 0;
        for i in 0..node.text.chars().count() as u32 {
            if let Some(with) = edit.inserted.get(&i) {
                if kept > 0 {
                    out.extend(element(kept));
                    kept = 0;
                }
                out.extend_from_slice(quick_xml::escape::escape(with.as_str()).as_bytes());
            }
            if !edit.removed.contains(&i) {
                kept += 1;
            }
        }
        if kept > 0 {
            out.extend(element(kept));
        }
        out
    }
}

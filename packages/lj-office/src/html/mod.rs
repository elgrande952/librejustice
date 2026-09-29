//! A web page Word saved or opens, often under a `.doc` name: its character
//! data read as a document, and rewritten in place, the markup around it
//! left byte for byte.

mod css;
mod to_model;
mod token;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;

use anyhow::{bail, Result};
use encoding_rs::Encoding;

use crate::model::Document;
use crate::{record, views_showing, Anchor, NodeEdit, Replacement, Views, EVERY_VIEW};
use token::{Node, Token, Tokens};

/// Bytes that open as a web page: markup whose first element is `<html>` or
/// that holds an `<html>` or a `<body>` near its start.
pub fn is_html(bytes: &[u8]) -> bool {
    let (encoding, bom) = encoding(bytes);
    let head = &bytes[bom..bytes.len().min(bom + 2048)];
    let (text, _) = encoding.decode_without_bom_handling(head);
    let text = text.trim_start().to_ascii_lowercase();
    text.starts_with("<!doctype html")
        || text.starts_with("<html")
        || (text.starts_with('<') && (text.contains("<html") || text.contains("<body")))
}

/// The encoding of `bytes` and the length of their byte order mark: the mark,
/// else the charset a `<meta>` declares, else UTF-8 when they read as such,
/// else windows-1252.
fn encoding(bytes: &[u8]) -> (&'static Encoding, usize) {
    if let Some((encoding, bom)) = Encoding::for_bom(bytes) {
        return (encoding, bom);
    }
    let head = &bytes[..bytes.len().min(4096)];
    let lower = head.to_ascii_lowercase();
    let declared = memchr::memmem::find_iter(&lower, b"charset")
        .find_map(|at| {
            let rest = &head[at + 7..];
            let rest = rest
                .trim_ascii_start()
                .strip_prefix(b"=")?
                .trim_ascii_start();
            let rest = rest
                .strip_prefix(b"\"")
                .or_else(|| rest.strip_prefix(b"'"))
                .unwrap_or(rest);
            let end = rest
                .iter()
                .position(|c| {
                    matches!(c, b'"' | b'\'' | b';' | b'>' | b'/') || c.is_ascii_whitespace()
                })
                .unwrap_or(rest.len());
            Encoding::for_label(&rest[..end])
        })
        .filter(|e| *e != encoding_rs::UTF_16LE && *e != encoding_rs::UTF_16BE);
    let encoding = declared.unwrap_or(match std::str::from_utf8(bytes).is_ok() {
        true => encoding_rs::UTF_8,
        false => encoding_rs::WINDOWS_1252,
    });
    (encoding, 0)
}

pub struct HtmlPackage {
    encoding: &'static Encoding,
    bom: Vec<u8>,
    source: String,
    tokens: Tokens,
    tree: Vec<Node>,
    edits: BTreeMap<u32, NodeEdit>,
    clear_authors: bool,
}

/// A text node the page does not show that says what the document is: the
/// title, a document property.
struct Metadata {
    node: u32,
    /// It names who wrote or holds the document.
    author: bool,
}

impl HtmlPackage {
    pub fn open(bytes: &[u8]) -> Result<Self> {
        if !is_html(bytes) {
            bail!("not a web page");
        }
        let (encoding, bom) = encoding(bytes);
        let (source, _) = encoding.decode_without_bom_handling(&bytes[bom..]);
        let source = source.into_owned();
        let tokens = token::tokenize(&source);
        let tree = token::tree(&tokens);
        Ok(Self {
            encoding,
            bom: bytes[..bom].to_vec(),
            source,
            tokens,
            tree,
            edits: BTreeMap::new(),
            clear_authors: false,
        })
    }

    pub fn document(&self) -> Document {
        to_model::document(self)
    }

    fn attributes(&self, element: &token::Element) -> &[token::Attribute] {
        match &self.tokens.segments[element.segment].token {
            Token::Open { attributes, .. } => attributes,
            _ => &[],
        }
    }

    fn text(&self, node: u32) -> &str {
        &self.tokens.nodes[node as usize].1
    }

    fn metadata(&self) -> Vec<Metadata> {
        fn walk(nodes: &[Node], parent: &str, properties: bool, out: &mut Vec<Metadata>) {
            for node in nodes {
                match node {
                    Node::Text(node) if parent == "title" => out.push(Metadata {
                        node: *node,
                        author: true,
                    }),
                    Node::Text(node) if properties => out.push(Metadata {
                        node: *node,
                        author: crate::ooxml::is_author_property(
                            parent.rsplit(':').next().unwrap_or(parent),
                        ),
                    }),
                    Node::Text(_) => {}
                    Node::Element(e) => {
                        let inside = properties
                            || matches!(
                                e.name.as_str(),
                                "o:documentproperties" | "o:customdocumentproperties"
                            );
                        walk(&e.children, &e.name, inside, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.tree, "", false, &mut out);
        out
    }

    /// The characters of the page, then of its title and properties, each
    /// closed by a newline holding its last anchor.
    pub fn characters(&self) -> Vec<(char, Anchor)> {
        let mut out = self.document().characters();
        for meta in self.metadata() {
            let text = self.text(meta.node);
            if text.trim().is_empty() {
                continue;
            }
            out.extend(text.chars().enumerate().map(|(i, c)| {
                (
                    c,
                    Anchor::XmlText {
                        part: 0,
                        node: meta.node,
                        offset: i as u32,
                    },
                )
            }));
            let last = out[out.len() - 1].1;
            out.push(('\n', last));
        }
        out
    }

    /// For each character of [`Self::characters`], the views that show it:
    /// a web page tracks no change, and hides only what its style hides.
    pub fn views(&self) -> Vec<Views> {
        let document = self.document();
        let hidden = document.hidden();
        self.characters()
            .iter()
            .map(|(_, a)| match hidden.contains(a) {
                true => views_showing(false, false, true),
                false => EVERY_VIEW,
            })
            .collect()
    }

    /// A web page writes no field code nor style name among its text.
    pub fn markup(&self) -> HashSet<Anchor> {
        HashSet::new()
    }

    /// Record `replacements`, which [`Self::save`] applies. Returns how many
    /// characters go.
    pub fn replace(&mut self, replacements: &[Replacement]) -> Result<usize> {
        let nodes = self.tokens.nodes.len();
        let mut parts = [std::mem::take(&mut self.edits)];
        let removed = record(replacements, &mut parts, |_| nodes)?;
        let [edits] = parts;
        self.edits = edits;
        Ok(removed)
    }

    /// Empty the title, the properties naming an author and the `<meta>`
    /// naming one, when [`Self::save`] runs.
    pub fn clear_authors(&mut self) {
        self.clear_authors = true;
    }

    /// The text nodes [`Self::characters`] reads.
    fn fed(&self) -> BTreeSet<u32> {
        self.characters()
            .into_iter()
            .filter_map(|(_, a)| match a {
                Anchor::XmlText { node, .. } => Some(node),
                _ => None,
            })
            .collect()
    }

    /// The spans of the source no reader of the text walks: tags, comments,
    /// styles, scripts, and character data the page does not show.
    fn markup_spans(&self) -> Vec<Range<usize>> {
        let fed = self.fed();
        self.tokens
            .segments
            .iter()
            .filter(|s| !matches!(s.token, Token::Text(n) if fed.contains(&n)))
            .map(|s| s.span.clone())
            .collect()
    }

    /// The markup of the page as written, span by span.
    pub fn markup_text(&self) -> Vec<&str> {
        self.markup_spans()
            .into_iter()
            .map(|span| &self.source[span])
            .collect()
    }

    /// Hand each span of markup to `rewrite`, as UTF-8, which must keep its
    /// length: the spans of the text nodes stay where they were read.
    pub fn rewrite_markup(&mut self, rewrite: &mut dyn FnMut(&mut Vec<u8>)) -> Result<()> {
        let mut bytes = std::mem::take(&mut self.source).into_bytes();
        for span in self.markup_spans() {
            let mut data = bytes[span.clone()].to_vec();
            rewrite(&mut data);
            if data.len() != span.len() {
                bail!("a rewrite of the markup changed its length");
            }
            bytes[span].copy_from_slice(&data);
        }
        self.source = String::from_utf8(bytes)?;
        Ok(())
    }

    /// The page with its replacements applied and, when asked, its authors
    /// emptied, in the encoding it was read in.
    pub fn save(self) -> Result<Vec<u8>> {
        let mut scrubbed: BTreeSet<u32> = BTreeSet::new();
        let mut blanked: Vec<Range<usize>> = Vec::new();
        if self.clear_authors {
            scrubbed.extend(
                self.metadata()
                    .into_iter()
                    .filter(|m| m.author)
                    .map(|m| m.node),
            );
            for segment in &self.tokens.segments {
                let Token::Open {
                    name, attributes, ..
                } = &segment.token
                else {
                    continue;
                };
                let names_author = attributes
                    .iter()
                    .any(|a| a.name == "name" && crate::ooxml::is_author_property(&a.value));
                if name == "meta" && names_author {
                    blanked.extend(
                        attributes
                            .iter()
                            .filter(|a| a.name == "content")
                            .filter_map(|a| a.span.clone()),
                    );
                }
            }
        }
        let mut out = String::with_capacity(self.source.len());
        for segment in &self.tokens.segments {
            let span = segment.span.clone();
            match segment.token {
                Token::Text(node) if scrubbed.contains(&node) => {}
                Token::Text(node) if self.edits.contains_key(&node) => {
                    out.push_str(&token::escape(&self.edits[&node].apply(self.text(node))));
                }
                _ => {
                    let mut at = span.start;
                    for blank in blanked
                        .iter()
                        .filter(|b| b.start >= span.start && b.end <= span.end)
                    {
                        out.push_str(&self.source[at..blank.start]);
                        at = blank.end;
                    }
                    out.push_str(&self.source[at..span.end]);
                }
            }
        }
        let mut bytes = self.bom;
        match self.encoding {
            e if e == encoding_rs::UTF_16LE => {
                bytes.extend(out.encode_utf16().flat_map(u16::to_le_bytes))
            }
            e if e == encoding_rs::UTF_16BE => {
                bytes.extend(out.encode_utf16().flat_map(u16::to_be_bytes))
            }
            e => bytes.extend_from_slice(&e.encode(&out).0),
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests;

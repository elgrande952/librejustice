//! An element tree over one package part, built on the splice's own walk so
//! every text node carries the number its anchor needs.

use crate::ooxml::{walk, Step};
use anyhow::{anyhow, bail, Result};
use quick_xml::events::BytesStart;
use quick_xml::XmlVersion;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ns {
    W,
    R,
    Mc,
    Wp,
    A,
    Pic,
    C,
    V,
    M,
    Wps,
    Wpg,
    Wpc,
    /// A SmartArt diagram's references to its parts.
    Dgm,
    /// The shapes Word saved for a SmartArt diagram.
    Dsp,
    O,
    W10,
    Rel,
    Other,
    /// An attribute written without a prefix.
    None,
}

fn namespace(uri: &str) -> Ns {
    match uri {
        "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
        | "http://purl.oclc.org/ooxml/wordprocessingml/main" => Ns::W,
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
        | "http://purl.oclc.org/ooxml/officeDocument/relationships" => Ns::R,
        "http://schemas.openxmlformats.org/markup-compatibility/2006" => Ns::Mc,
        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
        | "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing" => Ns::Wp,
        "http://schemas.openxmlformats.org/drawingml/2006/main"
        | "http://purl.oclc.org/ooxml/drawingml/main" => Ns::A,
        "http://schemas.openxmlformats.org/drawingml/2006/picture"
        | "http://purl.oclc.org/ooxml/drawingml/picture" => Ns::Pic,
        "http://schemas.openxmlformats.org/drawingml/2006/chart"
        | "http://purl.oclc.org/ooxml/drawingml/chart" => Ns::C,
        "urn:schemas-microsoft-com:vml" => Ns::V,
        "http://schemas.openxmlformats.org/officeDocument/2006/math"
        | "http://purl.oclc.org/ooxml/officeDocument/math" => Ns::M,
        "http://schemas.microsoft.com/office/word/2010/wordprocessingShape" => Ns::Wps,
        "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" => Ns::Wpg,
        "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas" => Ns::Wpc,
        "http://schemas.openxmlformats.org/drawingml/2006/diagram" => Ns::Dgm,
        "http://schemas.microsoft.com/office/drawing/2008/diagram" => Ns::Dsp,
        "urn:schemas-microsoft-com:office:office" => Ns::O,
        "urn:schemas-microsoft-com:office:word" => Ns::W10,
        "http://schemas.openxmlformats.org/package/2006/relationships" => Ns::Rel,
        _ => Ns::Other,
    }
}

/// Extensions of the WordprocessingML vocabulary whose content this reader
/// takes as it is, so an alternate-content branch requiring only them is read
/// rather than its fallback.
const UNDERSTOOD: &[&str] = &[
    "http://schemas.microsoft.com/office/word/2010/wordml",
    "http://schemas.microsoft.com/office/word/2012/wordml",
    "http://schemas.microsoft.com/office/word/2015/wordml/symex",
    "http://schemas.microsoft.com/office/word/2016/wordml/cid",
    "http://schemas.microsoft.com/office/word/2018/wordml",
    "http://schemas.microsoft.com/office/word/2018/wordml/cex",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing",
];

pub(crate) struct Element {
    pub ns: Ns,
    pub name: Box<str>,
    attrs: Vec<(Ns, Box<str>, Box<str>)>,
    pub children: Vec<Node>,
}

pub(crate) enum Node {
    Element(Element),
    /// A text node and its number in the part.
    Text(u32, Box<str>),
}

impl Element {
    pub fn is(&self, ns: Ns, name: &str) -> bool {
        self.ns == ns && &*self.name == name
    }

    pub fn attr(&self, ns: Ns, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, k, _)| *n == ns && &**k == name)
            .map(|(_, _, v)| &**v)
    }

    /// A `w:` attribute, the one most elements of the vocabulary carry.
    pub fn w(&self, name: &str) -> Option<&str> {
        self.attr(Ns::W, name)
    }

    pub fn val(&self) -> Option<&str> {
        self.w("val")
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(..) => None,
        })
    }

    pub fn child(&self, ns: Ns, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.is(ns, name))
    }

    /// A `w:` child element.
    pub fn wc(&self, name: &str) -> Option<&Element> {
        self.child(Ns::W, name)
    }
}

struct Frame {
    element: Element,
    /// Prefix bindings the element declares.
    bindings: Vec<(Box<str>, Box<str>)>,
}

fn resolve<'a>(stack: &'a [Frame], prefix: &str) -> Option<&'a str> {
    stack
        .iter()
        .rev()
        .flat_map(|f| f.bindings.iter().rev())
        .find(|(p, _)| &**p == prefix)
        .map(|(_, uri)| &**uri)
}

fn split(qname: &[u8]) -> (&[u8], &[u8]) {
    match qname.iter().position(|b| *b == b':') {
        Some(i) => (&qname[..i], &qname[i + 1..]),
        None => (&[], qname),
    }
}

fn open(stack: &[Frame], start: &BytesStart<'_>) -> Result<Frame> {
    let mut bindings = Vec::new();
    let mut raw = Vec::new();
    for attr in start.attributes().with_checks(false) {
        let attr = attr?;
        let value = attr.normalized_value(XmlVersion::Implicit1_0)?;
        let key = attr.key.as_ref();
        if key == b"xmlns" {
            bindings.push((Box::from(""), Box::from(&*value)));
        } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
            bindings.push((
                Box::from(String::from_utf8_lossy(prefix).as_ref()),
                Box::from(&*value),
            ));
        } else {
            raw.push((key.to_vec(), Box::<str>::from(&*value)));
        }
    }
    let frame_uri = |prefix: &[u8], stack: &[Frame], own: &[(Box<str>, Box<str>)]| {
        let prefix = String::from_utf8_lossy(prefix);
        own.iter()
            .rev()
            .find(|(p, _)| **p == *prefix)
            .map(|(_, u)| namespace(u))
            .or_else(|| resolve(stack, &prefix).map(namespace))
            .unwrap_or(Ns::Other)
    };
    let (prefix, local) = split(start.name().into_inner());
    let ns = frame_uri(prefix, stack, &bindings);
    let attrs = raw
        .into_iter()
        .map(|(key, value)| {
            let (prefix, local) = split(&key);
            let ns = if prefix.is_empty() {
                Ns::None
            } else {
                frame_uri(prefix, stack, &bindings)
            };
            (
                ns,
                Box::from(String::from_utf8_lossy(local).as_ref()),
                value,
            )
        })
        .collect();
    Ok(Frame {
        element: Element {
            ns,
            name: Box::from(String::from_utf8_lossy(local).as_ref()),
            attrs,
            children: Vec::new(),
        },
        bindings,
    })
}

/// Replace an `mc:AlternateContent` by the children of the branch this reader
/// takes: the first choice whose requirements it understands, else the
/// fallback.
fn alternate(frame: Frame, stack: &[Frame]) -> Vec<Node> {
    let mut fallback = Vec::new();
    for node in frame.element.children {
        let Node::Element(branch) = node else {
            continue;
        };
        if branch.is(Ns::Mc, "Choice") {
            let understood = branch.attr(Ns::None, "Requires").is_some_and(|req| {
                req.split_whitespace().all(|prefix| {
                    frame
                        .bindings
                        .iter()
                        .find(|(p, _)| &**p == prefix)
                        .map(|(_, u)| &**u)
                        .or_else(|| resolve(stack, prefix))
                        .is_some_and(|uri| UNDERSTOOD.contains(&uri))
                })
            });
            if understood {
                return branch.children;
            }
        } else if branch.is(Ns::Mc, "Fallback") {
            fallback = branch.children;
        }
    }
    fallback
}

/// Parse a part into its root element.
pub(crate) fn parse(part: &[u8]) -> Result<Element> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<Element> = None;
    let mut error: Option<anyhow::Error> = None;
    let close = |stack: &mut Vec<Frame>, root: &mut Option<Element>| {
        let Some(frame) = stack.pop() else {
            return;
        };
        let nodes = if frame.element.is(Ns::Mc, "AlternateContent") {
            alternate(frame, stack)
        } else {
            vec![Node::Element(frame.element)]
        };
        match stack.last_mut() {
            Some(parent) => parent.element.children.extend(nodes),
            None => {
                *root = nodes.into_iter().find_map(|n| match n {
                    Node::Element(e) => Some(e),
                    Node::Text(..) => None,
                })
            }
        }
    };
    walk(part, |step| {
        if error.is_some() {
            return;
        }
        match step {
            Step::Open { .. } if stack.len() == crate::MAX_DEPTH => {
                error = Some(anyhow!(
                    "the part nests elements deeper than {}",
                    crate::MAX_DEPTH
                ))
            }
            Step::Open { element, empty } => match open(&stack, element) {
                Ok(frame) => {
                    stack.push(frame);
                    if empty {
                        close(&mut stack, &mut root);
                    }
                }
                Err(e) => error = Some(e),
            },
            Step::Close(_) => close(&mut stack, &mut root),
            Step::Text { node, text, .. } => {
                if let Some(parent) = stack.last_mut() {
                    parent
                        .element
                        .children
                        .push(Node::Text(node, Box::from(text)));
                }
            }
        }
    })?;
    if let Some(e) = error {
        return Err(e);
    }
    match root {
        Some(root) => Ok(root),
        None => bail!("the part has no root element"),
    }
}

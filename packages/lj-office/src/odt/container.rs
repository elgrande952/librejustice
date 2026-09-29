//! An OpenDocument package and every package nested in it, with each XML part
//! parsed once: the reader and the edits walk the same leaves.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Range;

use anyhow::{anyhow, bail, Result};

use super::package::{is_zip, Package};
use super::xml::{Attr, Child, Element, Kind, Ns, Xml};
use crate::ooxml::{directory_len, is_author_property, is_local_file};
use crate::ooxml::{MAX_CHARACTERS, MAX_UNPACKED};
use crate::{record, views_showing, Anchor, NodeEdit, Replacement, Views, EVERY_VIEW};

const NESTING: usize = 4;

pub struct Part {
    pub package: usize,
    pub entry: usize,
    pub xml: Xml,
}

struct Nested {
    package: Package,
    parent: Option<(usize, usize)>,
    odf: bool,
    dropped: HashSet<usize>,
    touched: bool,
}

pub struct OdtPackage {
    packages: Vec<Nested>,
    pub(crate) parts: Vec<Part>,
    /// Entries that are neither XML nor a package, which only a byte scan reads.
    binaries: Vec<(usize, usize)>,
    /// The replacements recorded, by part as anchors number them.
    edits: Vec<BTreeMap<u32, NodeEdit>>,
    clear_authors: bool,
    drop_thumbnails: bool,
}

fn looks_xml(name: &str, data: &[u8]) -> bool {
    let lower = name.to_ascii_lowercase();
    if [".xml", ".rdf", ".svg", ".xhp", ".xlb", ".xba", ".xdl"]
        .iter()
        .any(|x| lower.ends_with(x))
    {
        return true;
    }
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let first = data.iter().find(|b| !b.is_ascii_whitespace());
    first == Some(&b'<') && std::str::from_utf8(&data[..data.len().min(512)]).is_ok()
}

/// Element that, for a reader, starts and ends a line of its own.
fn breaks(e: &Element) -> bool {
    let local = e.local.as_str();
    match e.ns {
        Ns::Text => matches!(
            local,
            "p" | "h" | "list-item" | "list-header" | "note-body" | "index-title" | "section"
        ),
        Ns::Table => matches!(
            local,
            "table" | "table-row" | "table-cell" | "covered-table-cell"
        ),
        Ns::Dc | Ns::Meta | Ns::Draw => true,
        Ns::Config => local == "config-item",
        Ns::Svg => matches!(local, "title" | "desc"),
        Ns::Other => matches!(
            local,
            "p" | "tr" | "br" | "tab" | "si" | "c" | "text" | "title" | "desc"
        ),
        _ => false,
    }
}

fn is_paragraph(e: &Element) -> bool {
    e.ns == Ns::Text && matches!(e.local.as_str(), "p" | "h")
}

/// Content inside a paragraph that a reader does not see in the line: a
/// comment, a note body, a frame. Walked after the part so a name split around
/// it is still contiguous.
fn out_of_flow(e: &Element) -> bool {
    e.ns == Ns::Draw
        || matches!(
            (e.ns, e.local.as_str()),
            (Ns::Office, "annotation")
                | (Ns::Office, "change-info")
                | (Ns::Text, "note-body")
                | (Ns::Text, "ruby-text")
                | (Ns::Svg, "title")
                | (Ns::Svg, "desc")
        )
}

/// Author metadata, which [`OdtPackage::clear_authors`] empties.
fn is_author(e: &Element) -> bool {
    match (e.ns, e.local.as_str()) {
        (Ns::Dc, "creator")
        | (Ns::Meta, "initial-creator")
        | (Ns::Meta, "printed-by")
        | (Ns::Meta, "creator-initials")
        | (Ns::Loext, "sender-initials") => true,
        (Ns::Meta, "user-defined") => e.attr(Ns::Meta, "name").is_some_and(is_author_property),
        _ => false,
    }
}

/// An attribute naming a style of the template, or a gradient, hatch or marker
/// it defines, which no reader sees.
fn names_a_style(element: &Element, ns: Ns, local: &str) -> bool {
    local.ends_with("style-name")
        || matches!(local, "master-page-name" | "page-layout-name")
        || matches!(local, "name" | "display-name")
            && (ns == Ns::Style
                || element.ns == Ns::Draw
                    && matches!(
                        element.local.as_str(),
                        "gradient" | "hatch" | "marker" | "stroke-dash" | "fill-image" | "opacity"
                    ))
}

/// An attribute naming a bookmark, a variable, a sequence, a drawing or a
/// link's frame, or a link landing on a bookmark of the document.
fn names_an_identifier(element: &Element, attr: &Attr) -> bool {
    match (attr.ns, attr.local.as_str()) {
        (Ns::Office, "target-frame-name") | (Ns::Text, "ref-name") | (Ns::Draw, "name") => true,
        (Ns::Xlink, "href") => attr.value.starts_with('#'),
        (Ns::Text, "name") => {
            element.ns == Ns::Text
                && matches!(
                    element.local.as_str(),
                    "bookmark"
                        | "bookmark-start"
                        | "bookmark-end"
                        | "reference-mark"
                        | "reference-mark-start"
                        | "reference-mark-end"
                        | "user-field-decl"
                        | "user-field-get"
                        | "user-field-input"
                        | "variable-decl"
                        | "variable-set"
                        | "variable-get"
                        | "variable-input"
                        | "sequence-decl"
                        | "sequence"
                )
        }
        _ => false,
    }
}

/// A field that shows author metadata, which LibreOffice reads again from
/// the emptied metadata unless the field is fixed.
fn shows_an_author(e: &Element) -> bool {
    match (e.ns, e.local.as_str()) {
        (Ns::Text, "initial-creator" | "creator" | "printed-by") => true,
        (Ns::Text, "user-defined") => e.attr(Ns::Text, "name").is_some_and(is_author_property),
        _ => false,
    }
}

/// The edit that makes a field fixed: `fixed="true"` written over its value,
/// or added at the end of its start tag.
fn fixed(data: &[u8], e: &Element) -> Option<(Range<u32>, Vec<u8>)> {
    if e.attr(Ns::Text, "fixed") == Some("true") {
        return None;
    }
    let start = e.span.start as usize;
    let mut quote = None;
    let close = data[start..].iter().position(|&b| {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None => return b == b'>',
        }
        false
    })? + start;
    let tag = &data[start..close];
    let name = tag
        .iter()
        .position(|b| b.is_ascii_whitespace() || *b == b'/')
        .unwrap_or(tag.len());
    let prefix = std::str::from_utf8(&tag[1..name]).ok()?.split_once(':')?.0;
    let at = |offset: usize| (start + offset) as u32;
    if e.attr(Ns::Text, "fixed").is_some() {
        let key = format!("{prefix}:fixed");
        let key_at = tag.windows(key.len()).position(|w| w == key.as_bytes())?;
        let open = key_at
            + tag[key_at..]
                .iter()
                .position(|b| *b == b'"' || *b == b'\'')?;
        let end = open + 1 + tag[open + 1..].iter().position(|b| *b == tag[open])?;
        return Some((at(open + 1)..at(end), b"true".to_vec()));
    }
    let end = if tag.ends_with(b"/") {
        tag.len() - 1
    } else {
        tag.len()
    };
    Some((
        at(end)..at(end),
        format!(" {prefix}:fixed=\"true\"").into_bytes(),
    ))
}

/// Entries dropped unread: the thumbnail and a saved version show the document
/// as it was, and a signature carries its signer and breaks at any change.
fn dropped_whole(name: &str) -> bool {
    name.starts_with("Thumbnails/")
        || name.starts_with("Versions/")
        || matches!(
            name,
            "VersionList.xml" | "META-INF/documentsignatures.xml" | "META-INF/macrosignatures.xml"
        )
}

/// A paragraph signature's RDF statement: its digest holds the signer's
/// certificate, hex encoded, which LibreOffice prints back into the paragraph.
fn signs_a_paragraph(xml: &Xml, e: &Element) -> bool {
    e.local == "Description"
        && e.children.iter().any(|c| match c {
            Child::Element(id) => xml.element(*id).ns == Ns::ParagraphSignature,
            Child::Node(_) => false,
        })
}

/// Characters with the views that show each.
struct Feed<'o> {
    out: &'o mut Vec<(char, Anchor)>,
    views: &'o mut Vec<Views>,
}

impl Feed<'_> {
    /// A line break shown in `views`, or those views added to the one the
    /// feed ends on.
    fn sep(&mut self, views: Views) {
        match self.out.last().copied() {
            Some(('\n', _)) => *self.views.last_mut().expect("one view per character") |= views,
            Some((_, a)) => {
                self.out.push(('\n', a));
                self.views.push(views);
            }
            None => {}
        }
    }

    fn node(&mut self, xml: &Xml, part: u16, node: u32, views: Views) {
        for (offset, c) in xml.nodes[node as usize].text.chars().enumerate() {
            self.out.push((
                c,
                Anchor::XmlText {
                    part,
                    node,
                    offset: offset as u32,
                },
            ));
            self.views.push(views);
        }
    }
}

/// Where a walk stands in a tracked deletion drawn at its mark.
type Deleted = bool;

enum Step {
    Enter(u32, bool, Deleted),
    /// An element's children, without the line its own element would start.
    Content(u32, bool, Deleted),
    Leaf(u32, Deleted),
    Exit(u32, Deleted),
    Sep(Deleted),
}

/// The deletion each `text:changed-region` holds, by change identifier.
pub(super) fn deletions(xml: &Xml) -> HashMap<&str, u32> {
    changes(xml, "deletion")
}

/// The element of kind `kind`, `deletion` or `insertion`, each
/// `text:changed-region` holds, by change identifier.
fn changes<'x>(xml: &'x Xml, kind: &str) -> HashMap<&'x str, u32> {
    let mut out = HashMap::new();
    for e in &xml.elements {
        if !e.is(Ns::Text, "changed-region") {
            continue;
        }
        let Some(name) = e.attr(Ns::Text, "id").or_else(|| e.attr(Ns::Xml, "id")) else {
            continue;
        };
        let change = e.children.iter().find_map(|c| match c {
            Child::Element(d) if xml.element(*d).is(Ns::Text, kind) => Some(*d),
            _ => None,
        });
        if let Some(d) = change {
            out.insert(name, d);
        }
    }
    out
}

/// The part's characters in reading order: the flow, then what sits out of it,
/// then free-text attribute values and comments, one line each. A tracked
/// deletion reads where its `text:change` mark stands, as LibreOffice shows it.
/// Each character goes with the views that show it: a deletion is left out
/// once changes are accepted, what stands between the two marks of an
/// insertion once they are rejected, and a text node of `hidden` once hidden
/// text is.
fn feed(xml: &Xml, part: u16, hidden: &HashSet<(u16, u32)>, feed: &mut Feed<'_>) {
    let deletions = deletions(xml);
    let insertions = changes(xml, "insertion");
    let mut inserting: HashSet<&str> = HashSet::new();
    let mut inlined: HashSet<u32> = HashSet::new();
    let mut deferred: std::collections::VecDeque<u32> = xml.root.into_iter().collect();
    let mut tracked: Vec<u32> = Vec::new();
    let mut walk = |start: u32,
                    deferred: &mut std::collections::VecDeque<u32>,
                    tracked: &mut Vec<u32>,
                    inlined: &mut HashSet<u32>,
                    feed: &mut Feed<'_>| {
        feed.sep(EVERY_VIEW);
        let mut todo = vec![Step::Enter(start, false, false)];
        while let Some(step) = todo.pop() {
            let shown = |deleted: Deleted, inserting: &HashSet<&str>| {
                views_showing(deleted, !inserting.is_empty(), false)
            };
            let (id, inside, deleted) = match step {
                Step::Enter(id, in_paragraph, deleted) => {
                    let e = xml.element(id);
                    if e.is(Ns::Text, "tracked-changes") && !inlined.contains(&id) {
                        tracked.push(id);
                        continue;
                    }
                    if breaks(e) {
                        feed.sep(shown(deleted, &inserting));
                    }
                    let change = e.attr(Ns::Text, "change-id");
                    match change.filter(|c| insertions.contains_key(c)) {
                        Some(c) if e.is(Ns::Text, "change-start") => {
                            inserting.insert(c);
                        }
                        Some(c) if e.is(Ns::Text, "change-end") => {
                            inserting.remove(c);
                        }
                        _ => {}
                    }
                    todo.push(Step::Exit(id, deleted));
                    if e.is(Ns::Text, "change") {
                        let deletion = change.and_then(|c| deletions.get(c)).copied();
                        if let Some(d) = deletion.filter(|d| inlined.insert(*d)) {
                            let paragraphs: Vec<u32> = xml
                                .element(d)
                                .children
                                .iter()
                                .filter_map(|c| match c {
                                    Child::Element(c) if !out_of_flow(xml.element(*c)) => Some(*c),
                                    _ => None,
                                })
                                .collect();
                            for (k, c) in paragraphs.iter().enumerate().rev() {
                                if is_paragraph(xml.element(*c)) {
                                    todo.push(Step::Content(*c, true, true));
                                } else {
                                    todo.push(Step::Enter(*c, true, true));
                                }
                                if k > 0 {
                                    todo.push(Step::Sep(true));
                                }
                            }
                        }
                        continue;
                    }
                    (id, in_paragraph || is_paragraph(e), deleted)
                }
                Step::Content(id, in_paragraph, deleted) => (id, in_paragraph, deleted),
                Step::Leaf(n, deleted) => {
                    let views =
                        views_showing(deleted, !inserting.is_empty(), hidden.contains(&(part, n)));
                    feed.node(xml, part, n, views);
                    continue;
                }
                Step::Exit(id, deleted) => {
                    if breaks(xml.element(id)) {
                        feed.sep(shown(deleted, &inserting));
                    }
                    continue;
                }
                Step::Sep(deleted) => {
                    feed.sep(shown(deleted, &inserting));
                    continue;
                }
            };
            let e = xml.element(id);
            let mut later = Vec::new();
            for child in e.children.iter() {
                if let Child::Element(c) = child {
                    if inside && out_of_flow(xml.element(*c)) {
                        later.push(*c);
                    }
                }
            }
            for child in e.children.iter().rev() {
                match child {
                    Child::Node(n) => todo.push(Step::Leaf(*n, deleted)),
                    Child::Element(c) if !later.contains(c) && !inlined.contains(c) => {
                        todo.push(Step::Enter(*c, inside, deleted))
                    }
                    Child::Element(_) => {}
                }
            }
            deferred.extend(later);
        }
    };
    while let Some(start) = deferred.pop_front() {
        walk(start, &mut deferred, &mut tracked, &mut inlined, feed);
    }
    // What no mark placed: the change metadata, and deletions whose mark is gone.
    for t in std::mem::take(&mut tracked) {
        inlined.insert(t);
        walk(t, &mut deferred, &mut tracked, &mut inlined, feed);
        while let Some(start) = deferred.pop_front() {
            walk(start, &mut deferred, &mut tracked, &mut inlined, feed);
        }
    }
    for (n, node) in xml.nodes.iter().enumerate() {
        if matches!(node.kind, Kind::Attr | Kind::Comment) {
            feed.sep(EVERY_VIEW);
            feed.node(xml, part, n as u32, EVERY_VIEW);
        }
    }
}

fn leaves_under(xml: &Xml, e: &Element, out: &mut Vec<u32>) {
    let mut todo = vec![e];
    while let Some(e) = todo.pop() {
        for c in &e.children {
            match c {
                Child::Node(n) => out.push(*n),
                Child::Element(id) => todo.push(xml.element(*id)),
            }
        }
    }
}

impl OdtPackage {
    pub fn open(bytes: &[u8]) -> Result<Self> {
        let mut left = MAX_UNPACKED;
        let package = Package::open(bytes, &mut left)?;
        let mimetype = package
            .get("mimetype")
            .map(|m| String::from_utf8_lossy(m).trim().to_string());
        let manifest = package.get("META-INF/manifest.xml").unwrap_or_default();
        let declared = match &mimetype {
            Some(m) => m.clone(),
            None => manifest_root_type(manifest).unwrap_or_default(),
        };
        if !declared.starts_with("application/vnd.oasis.opendocument.text") {
            bail!("not an OpenDocument text: {declared:?}");
        }
        if memchr(manifest, b"encryption-data") {
            bail!("the document is encrypted");
        }
        let mut this = Self {
            packages: Vec::new(),
            parts: Vec::new(),
            binaries: Vec::new(),
            edits: Vec::new(),
            clear_authors: false,
            drop_thumbnails: false,
        };
        this.walk(package, None, 0, &mut left)?;
        let characters: usize = this
            .parts
            .iter()
            .flat_map(|p| &p.xml.nodes)
            .map(|n| n.text.chars().count())
            .sum();
        if characters > MAX_CHARACTERS {
            bail!("the package carries {characters} characters, more than {MAX_CHARACTERS}");
        }
        if this.part(0, "content.xml").is_none() {
            bail!("the package has no content.xml");
        }
        this.edits = vec![BTreeMap::new(); this.parts.len()];
        Ok(this)
    }

    fn walk(
        &mut self,
        package: Package,
        parent: Option<(usize, usize)>,
        depth: usize,
        left: &mut u64,
    ) -> Result<()> {
        let id = self.packages.len();
        let odf =
            package.get("mimetype").is_some() || package.get("META-INF/manifest.xml").is_some();
        let names: Vec<(String, bool, bool)> = package
            .entries
            .iter()
            .map(|e| (e.name.clone(), is_zip(&e.data), looks_xml(&e.name, &e.data)))
            .collect();
        self.packages.push(Nested {
            package,
            parent,
            odf,
            dropped: HashSet::new(),
            touched: false,
        });
        for (entry, (name, zip, xml)) in names.into_iter().enumerate() {
            if name == "mimetype" || name.ends_with('/') || (odf && dropped_whole(&name)) {
                continue;
            }
            if zip && depth < NESTING {
                let nested = Package::open(&self.packages[id].package.entries[entry].data, left);
                if let Ok(nested) = nested {
                    self.walk(nested, Some((id, entry)), depth + 1, left)?;
                    continue;
                }
            }
            if xml {
                let data = &self.packages[id].package.entries[entry].data;
                match Xml::parse(data) {
                    Ok(xml) => {
                        self.parts.push(Part {
                            package: id,
                            entry,
                            xml,
                        });
                        continue;
                    }
                    Err(e) if id == 0 && matches!(name.as_str(), "content.xml" | "styles.xml") => {
                        return Err(e.context(name));
                    }
                    Err(_) => {}
                }
            }
            self.binaries.push((id, entry));
        }
        Ok(())
    }

    pub fn name(&self, package: usize, entry: usize) -> &str {
        &self.packages[package].package.entries[entry].name
    }

    /// The global index of a part, the one an anchor carries.
    pub fn part(&self, package: usize, name: &str) -> Option<usize> {
        self.parts
            .iter()
            .position(|p| p.package == package && self.name(p.package, p.entry) == name)
    }

    pub fn entry(&self, package: usize, name: &str) -> Option<&[u8]> {
        self.packages.get(package)?.package.get(name)
    }

    /// Every character of every part of every nested package, each anchored.
    pub fn characters(&self) -> Vec<(char, Anchor)> {
        let mut out = Vec::new();
        self.feed(&HashSet::new(), &mut out, &mut Vec::new());
        out
    }

    /// For each character of [`Self::characters`], the views that show it.
    /// Hidden text is what the document model hides, a tracked deletion
    /// drawn whatever the file says of showing changes.
    pub fn views(&self) -> Result<Vec<Views>> {
        let model = super::to_model(&super::reader::read_showing_changes(self)?);
        let hidden: HashSet<(u16, u32)> = model
            .hidden()
            .into_iter()
            .filter_map(|a| match a {
                Anchor::XmlText { part, node, .. } => Some((part, node)),
                _ => None,
            })
            .collect();
        let mut views = Vec::new();
        self.feed(&hidden, &mut Vec::new(), &mut views);
        Ok(views)
    }

    fn feed(
        &self,
        hidden: &HashSet<(u16, u32)>,
        out: &mut Vec<(char, Anchor)>,
        views: &mut Vec<Views>,
    ) {
        let mut sink = Feed { out, views };
        for (i, part) in self.parts.iter().enumerate() {
            sink.sep(EVERY_VIEW);
            feed(&part.xml, i as u16, hidden, &mut sink);
        }
    }

    /// The characters of [`Self::characters`] that name a style, a bookmark,
    /// a variable, a drawing or a link's frame.
    pub fn markup(&self) -> HashSet<Anchor> {
        let mut out = HashSet::new();
        for (i, part) in self.parts.iter().enumerate() {
            for e in &part.xml.elements {
                let names = e.attrs.iter().filter_map(|a| {
                    a.node
                        .filter(|_| names_a_style(e, a.ns, &a.local) || names_an_identifier(e, a))
                });
                for node in names {
                    let length = part.xml.nodes[node as usize].text.chars().count() as u32;
                    out.extend((0..length).map(|offset| Anchor::XmlText {
                        part: i as u16,
                        node,
                        offset,
                    }));
                }
            }
        }
        out
    }

    /// Every entry no XML reader walks, nested packages included, by name.
    pub fn binaries(&self) -> Vec<(&str, &[u8])> {
        self.binaries
            .iter()
            .map(|&(package, entry)| {
                let data = &self.packages[package].package.entries[entry].data;
                (self.name(package, entry), data.as_slice())
            })
            .collect()
    }

    /// Rewrite every entry [`Self::binaries`] lists, in place; `rewrite`
    /// returns whether it changed the bytes, which rebuilds a nested package.
    pub fn rewrite_binaries(&mut self, mut rewrite: impl FnMut(&mut Vec<u8>) -> bool) {
        for &(package, entry) in &self.binaries {
            let nested = &mut self.packages[package];
            if rewrite(&mut nested.package.entries[entry].data) {
                nested.touched = true;
            }
        }
    }

    /// Record `replacements`, which [`Self::save`] applies. Returns how many
    /// characters go.
    pub fn replace(&mut self, replacements: &[Replacement]) -> Result<usize> {
        let parts = &self.parts;
        record(replacements, &mut self.edits, |part| {
            parts[part].xml.nodes.len()
        })
    }

    /// Empty the author metadata, cut the folders from a path naming the
    /// author's machine, and fix the fields that show an author, when
    /// [`Self::save`] runs.
    pub fn clear_authors(&mut self) {
        self.clear_authors = true;
    }

    /// Drop the thumbnails, the saved versions and the signatures, and the
    /// preview image of every object an edit touched, with what points at
    /// them, when [`Self::save`] runs.
    pub fn drop_thumbnails(&mut self) {
        self.drop_thumbnails = true;
    }

    fn apply(&mut self) -> Result<()> {
        let mut doomed = std::mem::take(&mut self.edits);
        let mut touched_objects: BTreeSet<(usize, String)> = BTreeSet::new();
        for (i, part) in self.parts.iter().enumerate() {
            if doomed[i].is_empty() {
                continue;
            }
            let name = self.name(part.package, part.entry);
            if let Some((dir, _)) = name.split_once('/') {
                touched_objects.insert((part.package, dir.to_string()));
            }
        }

        let authors = self.clear_authors;
        for (i, part) in self.parts.iter().enumerate().filter(|_| authors) {
            let mut authors = Vec::new();
            for e in &part.xml.elements {
                if is_author(e) {
                    leaves_under(&part.xml, e, &mut authors);
                }
            }
            for n in authors {
                let count = part.xml.nodes[n as usize].text.chars().count() as u32;
                doomed[i].entry(n).or_default().removed.extend(0..count);
            }
            for e in &part.xml.elements {
                for a in &e.attrs {
                    let Some(n) = a.node.filter(|_| a.ns == Ns::Xlink && a.local == "href") else {
                        continue;
                    };
                    if e.is(Ns::Meta, "template") || is_local_file(&a.value) {
                        let text = &part.xml.nodes[n as usize].text;
                        let folders = directory_len(text) as u32;
                        if folders > 0 {
                            doomed[i].entry(n).or_default().removed.extend(0..folders);
                        }
                    }
                }
            }
        }

        for id in 0..self.packages.len() {
            if !self.drop_thumbnails || !self.packages[id].odf {
                continue;
            }
            let entries = &self.packages[id].package.entries;
            let mut dropped = HashSet::new();
            for (e, entry) in entries.iter().enumerate() {
                if dropped_whole(&entry.name) {
                    dropped.insert(e);
                }
                if let Some(object) = entry.name.strip_prefix("ObjectReplacements/") {
                    if touched_objects.contains(&(id, object.to_string())) {
                        dropped.insert(e);
                    }
                }
            }
            self.packages[id].dropped = dropped;
        }

        let mut cuts: Vec<Vec<(Range<u32>, Vec<u8>)>> = vec![Vec::new(); self.parts.len()];
        for (i, part) in self.parts.iter().enumerate() {
            let data = &self.packages[part.package].package.entries[part.entry].data;
            for e in &part.xml.elements {
                if self.drop_thumbnails && signs_a_paragraph(&part.xml, e) {
                    cuts[i].push((e.span.clone(), Vec::new()));
                }
                if self.clear_authors && shows_an_author(e) {
                    cuts[i].extend(fixed(data, e));
                }
            }
            let package = &self.packages[part.package];
            if package.dropped.is_empty() {
                continue;
            }
            let gone: HashSet<&str> = package
                .dropped
                .iter()
                .map(|e| package.package.entries[*e].name.as_str())
                .collect();
            let is_gone = |path: &str| {
                let path = path.strip_prefix("./").unwrap_or(path);
                gone.contains(path) || gone.contains(format!("{path}/").as_str())
            };
            for e in &part.xml.elements {
                let hit = match (e.ns, e.local.as_str()) {
                    (Ns::Manifest, "file-entry") => e
                        .attr(Ns::Manifest, "full-path")
                        .is_some_and(|p| is_gone(p) || dropped_whole(p)),
                    (Ns::Draw, "image") => e.attr(Ns::Xlink, "href").is_some_and(is_gone),
                    _ => false,
                };
                if hit {
                    cuts[i].push((e.span.clone(), Vec::new()));
                }
            }
        }

        for (i, part) in self.parts.iter().enumerate() {
            if doomed[i].is_empty() && cuts[i].is_empty() {
                continue;
            }
            let package = &mut self.packages[part.package];
            let data = &mut package.package.entries[part.entry].data;
            *data = part.xml.rewrite(data, &doomed[i], &cuts[i]);
            package.touched = true;
        }
        for package in &mut self.packages {
            if !package.dropped.is_empty() {
                package.touched = true;
            }
        }
        Ok(())
    }

    /// Apply what was recorded, then write the package back, each nested
    /// package rebuilt only if something inside it changed.
    pub fn save(mut self) -> Result<Vec<u8>> {
        self.apply()?;
        let mut rebuilt: BTreeMap<(usize, usize), Vec<u8>> = BTreeMap::new();
        let mut touched: Vec<bool> = self.packages.iter().map(|p| p.touched).collect();
        for id in (1..self.packages.len()).rev() {
            if touched[id] {
                if let Some((parent, _)) = self.packages[id].parent {
                    touched[parent] = true;
                }
            }
        }
        for id in (0..self.packages.len()).rev() {
            let nested = &self.packages[id];
            if id > 0 && !touched[id] {
                continue;
            }
            let entries = nested
                .package
                .entries
                .iter()
                .enumerate()
                .filter(|(e, _)| !nested.dropped.contains(e))
                .map(|(e, entry)| {
                    let data = rebuilt.remove(&(id, e));
                    (entry, data)
                })
                .collect::<Vec<_>>();
            let bytes = super::package::write(&entries)?;
            match nested.parent {
                Some(at) => {
                    rebuilt.insert(at, bytes);
                }
                None => return Ok(bytes),
            }
        }
        Err(anyhow!("the package tree has no root"))
    }
}

fn memchr(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn manifest_root_type(manifest: &[u8]) -> Option<String> {
    let xml = Xml::parse(manifest).ok()?;
    xml.elements
        .iter()
        .find(|e| {
            e.is(Ns::Manifest, "file-entry") && e.attr(Ns::Manifest, "full-path") == Some("/")
        })
        .and_then(|e| e.attr(Ns::Manifest, "media-type"))
        .map(str::to_string)
}

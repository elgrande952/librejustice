use std::collections::HashMap;
use std::sync::Arc;

use crate::graphics::{PageBox, Rect};

use crate::objects::error::{PdfError, PdfResult};
use crate::objects::types::{ObjectRef, PdfDictionary, PdfFile, PdfObject, PdfValue};

#[derive(Debug, Clone)]
pub struct DocumentCatalog {
    pub catalog_ref: ObjectRef,
    pub pages_ref: ObjectRef,
}

#[derive(Debug, Clone)]
pub struct PageInfo {
    pub page_ref: ObjectRef,
    /// Shared by every page that inherits the same dictionary: a tree node's
    /// resources can be large and its pages many.
    pub resources: Arc<PdfDictionary>,
    pub page_box: PageBox,
    pub content_refs: Vec<ObjectRef>,
    pub annotation_refs: Vec<ObjectRef>,
}

#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub file: PdfFile,
    pub catalog: DocumentCatalog,
    pub pages: Vec<PageInfo>,
}

pub fn build_document(file: PdfFile) -> PdfResult<ParsedDocument> {
    // Encrypted PDFs are decrypted in-place earlier in the pipeline
    // (parser::decrypt_document_if_encrypted removes the /Encrypt entry from
    // the trailer on success). A trailer that still contains /Encrypt by the
    // time we get here means the Standard Security Handler code path was
    // skipped entirely, an unsupported scheme, for example.
    if file.trailer.contains_key("Encrypt") {
        return Err(PdfError::Unsupported(
            "encrypted PDFs are not supported".to_string(),
        ));
    }

    let root = file
        .trailer
        .get("Root")
        .ok_or_else(|| PdfError::Corrupt("trailer is missing Root".to_string()))?;
    let root_ref = match root {
        PdfValue::Reference(object_ref) => *object_ref,
        _ => return Err(PdfError::Corrupt("Root is not a reference".to_string())),
    };
    let root_dict = file.get_dictionary(root_ref)?;

    let pages_ref = match root_dict.get("Pages") {
        Some(PdfValue::Reference(object_ref)) => *object_ref,
        _ => return Err(PdfError::Corrupt("Catalog is missing Pages".to_string())),
    };
    let catalog = DocumentCatalog {
        catalog_ref: root_ref,
        pages_ref,
    };

    let mut pages = Vec::new();
    if file.get_dictionary(pages_ref).is_ok() {
        let mut walk = Walk::default();
        collect_pages(
            &file,
            pages_ref,
            &mut pages,
            Inherited::default(),
            0,
            &mut walk,
        )?;
    } else {
        // A catalog whose page tree is gone: every page object, in file order.
        let page_refs: Vec<ObjectRef> = file
            .objects
            .iter()
            .filter(|(_, object)| {
                matches!(object, PdfObject::Value(PdfValue::Dictionary(dict))
                    if dict.get("Type").and_then(PdfValue::as_name) == Some("Page"))
            })
            .map(|(object_ref, _)| *object_ref)
            .collect();
        if page_refs.is_empty() {
            return Err(PdfError::Corrupt(
                "the catalog has no page tree".to_string(),
            ));
        }
        let mut walk = Walk::default();
        for page_ref in page_refs {
            let dictionary = file.get_dictionary(page_ref)?;
            pages.push(page_info(
                &file,
                page_ref,
                dictionary,
                Inherited::default(),
                &mut walk,
            )?);
        }
    }

    Ok(ParsedDocument {
        file,
        catalog,
        pages,
    })
}

const MAX_PAGE_TREE_DEPTH: usize = 64;

/// More pages are refused: a tree can name one page a million times in a few
/// kilobytes of object stream.
const MAX_PAGES: usize = 100_000;

/// What walking the page tree remembers: the tree nodes already walked, and
/// each resource dictionary already given to a page.
#[derive(Default)]
struct Walk {
    visited: std::collections::BTreeSet<ObjectRef>,
    resources: HashMap<*const PdfDictionary, Arc<PdfDictionary>>,
    empty: Arc<PdfDictionary>,
}

impl Walk {
    fn resources(&mut self, dictionary: Option<&PdfDictionary>) -> Arc<PdfDictionary> {
        match dictionary {
            None => self.empty.clone(),
            Some(dictionary) => self
                .resources
                .entry(dictionary as *const PdfDictionary)
                .or_insert_with(|| Arc::new(dictionary.clone()))
                .clone(),
        }
    }
}

/// The page size readers assume when no node of the tree gives one.
const US_LETTER: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 612.0,
    height: 792.0,
};

/// What a page takes from the nodes above it.
#[derive(Clone, Copy, Default)]
struct Inherited<'a> {
    resources: Option<&'a PdfDictionary>,
    media_box: Option<Rect>,
    crop_box: Option<Rect>,
    rotate: Option<i32>,
}

impl<'a> Inherited<'a> {
    fn under(self, file: &'a PdfFile, node: &'a PdfDictionary) -> PdfResult<Self> {
        Ok(Self {
            resources: node
                .get("Resources")
                .and_then(|value| file.resolve_dict(value).ok())
                .or(self.resources),
            media_box: page_box(file, node, "MediaBox").or(self.media_box),
            crop_box: page_box(file, node, "CropBox").or(self.crop_box),
            rotate: node
                .get("Rotate")
                .map(|value| parse_rotation(file.resolve(value)?))
                .transpose()?
                .or(self.rotate),
        })
    }
}

fn collect_pages<'a>(
    file: &'a PdfFile,
    node_ref: ObjectRef,
    output: &mut Vec<PageInfo>,
    inherited: Inherited<'a>,
    depth: usize,
    walk: &mut Walk,
) -> PdfResult<()> {
    if depth > MAX_PAGE_TREE_DEPTH {
        return Err(PdfError::Corrupt(
            "page tree exceeds maximum depth".to_string(),
        ));
    }
    // A kid that is no dictionary is an empty page, as MuPDF shows it.
    let empty = PdfDictionary::new();
    let dictionary = file.get_dictionary(node_ref).unwrap_or(&empty);
    let is_tree_node = match dictionary.get("Type").and_then(PdfValue::as_name) {
        Some("Pages") => true,
        Some("Page") => false,
        _ => dictionary.contains_key("Kids"),
    };
    // A page named twice shows twice; a tree node is walked once.
    if !is_tree_node {
        if output.len() == MAX_PAGES {
            return Err(PdfError::Unsupported(format!(
                "the page tree names more than {MAX_PAGES} pages"
            )));
        }
        output.push(page_info(file, node_ref, dictionary, inherited, walk)?);
        return Ok(());
    }
    if !walk.visited.insert(node_ref) {
        return Ok(());
    }
    let inherited = inherited.under(file, dictionary)?;
    let kids = dictionary
        .get("Kids")
        .and_then(|kids| file.resolve(kids).ok())
        .and_then(PdfValue::as_array)
        .ok_or_else(|| PdfError::Corrupt("Pages node is missing Kids".to_string()))?;
    for kid in kids {
        let kid_ref = match kid {
            PdfValue::Reference(object_ref) => *object_ref,
            _ => {
                return Err(PdfError::Corrupt(
                    "Pages Kids entry is not an object reference".to_string(),
                ));
            }
        };
        collect_pages(file, kid_ref, output, inherited, depth + 1, walk)?;
    }
    Ok(())
}

fn page_info(
    file: &PdfFile,
    page_ref: ObjectRef,
    dictionary: &PdfDictionary,
    inherited: Inherited<'_>,
    walk: &mut Walk,
) -> PdfResult<PageInfo> {
    let own = inherited.under(file, dictionary)?;
    let media_box = own
        .media_box
        .filter(|media| media.width > 0.0 && media.height > 0.0)
        .unwrap_or(US_LETTER);
    let crop_box = own
        .crop_box
        .and_then(|crop| crop.intersection(&media_box))
        .unwrap_or(media_box);
    let annotation_refs = dictionary
        .get("Annots")
        .and_then(|v| file.resolve(v).ok())
        .and_then(PdfValue::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| match entry {
                    PdfValue::Reference(object_ref) => Some(*object_ref),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(PageInfo {
        page_ref,
        resources: walk.resources(own.resources),
        page_box: PageBox {
            media_box,
            crop_box,
            rotate: own.rotate.unwrap_or(0),
        },
        content_refs: parse_contents_refs(file, dictionary),
        annotation_refs,
    })
}

/// A page box, left out when it does not read as four numbers.
fn page_box(file: &PdfFile, node: &PdfDictionary, key: &str) -> Option<Rect> {
    node.get(key)
        .and_then(|value| resolve_rect(file, value).ok())
}

fn parse_rotation(value: &PdfValue) -> PdfResult<i32> {
    value
        .as_integer()
        .map(|value| value as i32)
        .ok_or_else(|| PdfError::Corrupt("Rotate is not an integer".to_string()))
}

fn resolve_rect(file: &PdfFile, value: &PdfValue) -> PdfResult<Rect> {
    match file.resolve(value)? {
        PdfValue::Array(items) => parse_rect(&PdfValue::Array(
            items
                .iter()
                .map(|item| file.resolve(item).cloned())
                .collect::<PdfResult<_>>()?,
        )),
        other => parse_rect(other),
    }
}

pub(crate) fn parse_rect(value: &PdfValue) -> PdfResult<Rect> {
    let array = value
        .as_array()
        .ok_or_else(|| PdfError::Corrupt("expected box array".to_string()))?;
    if array.len() != 4 {
        return Err(PdfError::Corrupt(
            "box array must contain four numbers".to_string(),
        ));
    }
    let left = array[0]
        .as_number()
        .ok_or_else(|| PdfError::Corrupt("invalid box value".to_string()))?;
    let bottom = array[1]
        .as_number()
        .ok_or_else(|| PdfError::Corrupt("invalid box value".to_string()))?;
    let right = array[2]
        .as_number()
        .ok_or_else(|| PdfError::Corrupt("invalid box value".to_string()))?;
    let top = array[3]
        .as_number()
        .ok_or_else(|| PdfError::Corrupt("invalid box value".to_string()))?;
    Ok(Rect {
        x: left,
        y: bottom,
        width: right - left,
        height: top - bottom,
    }
    .normalize())
}

/// The content streams of a page, a reference to an array of them read as
/// the array, an entry that is no reference skipped.
fn parse_contents_refs(file: &PdfFile, page: &PdfDictionary) -> Vec<ObjectRef> {
    let refs = |entries: &[PdfValue]| {
        entries
            .iter()
            .filter_map(|entry| match entry {
                PdfValue::Reference(object_ref) => Some(*object_ref),
                _ => None,
            })
            .collect()
    };
    match page.get("Contents") {
        Some(PdfValue::Reference(object_ref)) => match file.get_value(*object_ref) {
            Ok(PdfValue::Array(entries)) => refs(entries),
            _ => vec![*object_ref],
        },
        Some(PdfValue::Array(entries)) => refs(entries),
        _ => Vec::new(),
    }
}

pub fn get_stream(
    file: &PdfFile,
    object_ref: ObjectRef,
) -> PdfResult<&crate::objects::types::PdfStream> {
    match file.get_object(object_ref)? {
        PdfObject::Stream(stream) => Ok(stream),
        PdfObject::Value(_) => Err(PdfError::Corrupt(format!(
            "expected stream object at {} {}",
            object_ref.object_number, object_ref.generation
        ))),
    }
}

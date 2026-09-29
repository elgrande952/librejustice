use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::graphics::Size;
use crate::objects::document::get_stream;
use crate::objects::{
    PageInfo, ParsedDocument, PdfDictionary, PdfFile, PdfObject, PdfResult, PdfValue, parse_pdf,
    parse_pdf_with_certificate, parse_pdf_with_password,
};
use crate::redact::{ApplyReport, apply_redactions};
use crate::targets::{NormalizedRedactionPlan, normalize_plan};
use crate::text::{ExtractedPageText, TextItem, analyze_page_text, search_page_text, text_string};
use crate::writer::save_document;
use serde::{Deserialize, Serialize};

/// Normalized page size in PDF user-space units.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSize {
    /// Page width after crop-box translation and page rotation normalization.
    pub width: f64,
    /// Page height after crop-box translation and page rotation normalization.
    pub height: f64,
}

impl From<Size> for PageSize {
    fn from(value: Size) -> Self {
        Self {
            width: value.width,
            height: value.height,
        }
    }
}

/// Extracted text content and geometry for a single page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageText {
    /// Zero-based page index.
    pub page_index: usize,
    /// Human-readable page text assembled from the supported extraction subset.
    pub text: String,
    /// Structured text items with bounding geometry suitable for later authoring tools.
    pub items: Vec<TextItem>,
}

/// Parsed PDF document handle used for inspection, redaction, and save.
pub struct PdfDocument {
    parsed: ParsedDocument,
    /// Per-page cached results of [`analyze_page_text`]. Successive
    /// `extract_text` / `search_text` calls on the same page reuse the
    /// cached extraction instead of walking the content stream again;
    /// `apply_redactions` clears the cache since the underlying content
    /// streams are about to be rewritten.
    text_cache: Mutex<HashMap<usize, Arc<ExtractedPageText>>>,
}

impl PdfDocument {
    /// Opens an unencrypted PDF from raw bytes, or an encrypted PDF
    /// whose user password is empty. For encrypted PDFs that require a
    /// user- or owner-supplied password, use
    /// [`PdfDocument::open_with_password`].
    pub fn open(bytes: &[u8]) -> PdfResult<Self> {
        Ok(Self::with_parsed(parse_pdf(bytes)?))
    }

    /// Opens an encrypted PDF from raw bytes using the supplied password.
    /// The password is tried first as the user password, then as the
    /// owner password; if neither authenticates, the function returns
    /// [`crate::objects::PdfError::InvalidPassword`]. For unencrypted
    /// documents the password is ignored.
    pub fn open_with_password(bytes: &[u8], password: &[u8]) -> PdfResult<Self> {
        Ok(Self::with_parsed(parse_pdf_with_password(bytes, password)?))
    }

    /// Opens an Adobe.PubSec-encrypted PDF using the recipient's X.509
    /// certificate and matching RSA private key, both DER-encoded
    /// (PKCS#8 for the private key, the standard form returned by most
    /// browser key-management APIs). Returns
    /// [`crate::objects::PdfError::InvalidPassword`] when no recipient blob
    /// in the PDF unwraps with the supplied private key. For
    /// password-encrypted or unencrypted documents this returns
    /// [`crate::objects::PdfError::Unsupported`], use
    /// [`PdfDocument::open_with_password`] / [`PdfDocument::open`]
    /// respectively.
    pub fn open_with_certificate(
        bytes: &[u8],
        cert_der: &[u8],
        private_key_der: &[u8],
    ) -> PdfResult<Self> {
        Ok(Self::with_parsed(parse_pdf_with_certificate(
            bytes,
            cert_der,
            private_key_der,
        )?))
    }

    fn with_parsed(parsed: ParsedDocument) -> Self {
        Self {
            parsed,
            text_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the number of pages in the parsed document.
    pub fn page_count(&self) -> usize {
        self.parsed.pages.len()
    }

    /// Returns the normalized page size for a zero-based page index.
    pub fn page_size(&self, page_index: usize) -> PdfResult<PageSize> {
        let page = self
            .parsed
            .pages
            .get(page_index)
            .ok_or(crate::objects::PdfError::InvalidPageIndex(page_index))?;
        Ok(page.page_box.size().into())
    }

    /// Whether the page's resources name a picture, its own or one of the forms
    /// it draws, without drawing it. A picture inline in the content is not seen.
    pub fn page_names_picture(&self, page_index: usize) -> PdfResult<bool> {
        let page = self.get_page(page_index)?;
        Ok(names_picture(&self.parsed.file, &page.resources, 0))
    }

    /// The page drawn as a display list, lined up with the quads of its text.
    pub fn draw_page(&self, page_index: usize) -> PdfResult<lj_draw::Page> {
        crate::draw::draw_page(&self.parsed.file, self.get_page(page_index)?)
    }

    /// Extracts page text and geometry for the current supported subset.
    pub fn extract_text(&self, page_index: usize) -> PdfResult<PageText> {
        let extracted = self.cached_page_text(page_index)?;
        Ok(PageText {
            page_index,
            text: extracted.text.clone(),
            items: extracted.items.clone(),
        })
    }

    /// Searches page text in visual glyph order and returns page-space match geometry.
    pub fn search_text(&self, page_index: usize, query: &str) -> PdfResult<Vec<TextMatch>> {
        let extracted = self.cached_page_text(page_index)?;
        Ok(search_page_text(&extracted, query))
    }

    /// Applies a redaction plan in place to the opened document.
    pub fn apply_redactions(
        &mut self,
        plan: crate::targets::RedactionPlan,
    ) -> PdfResult<ApplyReport> {
        let normalized = self.normalize_plan(plan)?;
        let report = apply_redactions(&mut self.parsed.file, &mut self.parsed.pages, &normalized)?;
        // Content streams have been rewritten, so any cached extraction
        // from before the plan applied is stale.
        if let Ok(mut cache) = self.text_cache.lock() {
            cache.clear();
        }
        Ok(report)
    }

    /// Empties every string of the document's dictionaries whose text
    /// `matches`: an outline title, an annotation note or link, a page label,
    /// the name of a layer or of a structure element. Page contents are the
    /// redaction's.
    pub fn clear_strings(&mut self, mut matches: impl FnMut(&str) -> bool) {
        let file = &mut self.parsed.file;
        let mut stack: Vec<&mut PdfValue> = file.trailer.values_mut().collect();
        for object in file.objects.values_mut() {
            match object {
                PdfObject::Value(value) => stack.push(value),
                PdfObject::Stream(stream) => stack.extend(stream.dict.values_mut()),
            }
        }
        while let Some(value) = stack.pop() {
            match value {
                PdfValue::String(string) => {
                    if matches(&text_string(&string.0)) {
                        string.0.clear();
                    }
                }
                PdfValue::Array(items) => stack.extend(items.iter_mut()),
                PdfValue::Dictionary(dict) => stack.extend(dict.values_mut()),
                _ => {}
            }
        }
    }

    /// The text of every string of the document's dictionaries.
    pub fn strings(&self) -> Vec<String> {
        let file = &self.parsed.file;
        let mut stack: Vec<&PdfValue> = file.trailer.values().collect();
        for object in file.objects.values() {
            match object {
                PdfObject::Value(value) => stack.push(value),
                PdfObject::Stream(stream) => stack.extend(stream.dict.values()),
            }
        }
        let mut out = Vec::new();
        while let Some(value) = stack.pop() {
            match value {
                PdfValue::String(string) => out.push(text_string(&string.0)),
                PdfValue::Array(items) => stack.extend(items),
                PdfValue::Dictionary(dict) => stack.extend(dict.values()),
                _ => {}
            }
        }
        out
    }

    fn cached_page_text(&self, page_index: usize) -> PdfResult<Arc<ExtractedPageText>> {
        if let Ok(cache) = self.text_cache.lock()
            && let Some(entry) = cache.get(&page_index)
        {
            return Ok(entry.clone());
        }
        let page = self.get_page(page_index)?;
        let extracted = Arc::new(analyze_page_text(&self.parsed.file, page_index, page)?);
        if let Ok(mut cache) = self.text_cache.lock() {
            cache.insert(page_index, extracted.clone());
        }
        Ok(extracted)
    }

    /// Saves the current document state as a new deterministic full-save PDF.
    pub fn save(&self) -> PdfResult<Vec<u8>> {
        Ok(save_document(&self.parsed.file))
    }

    fn normalize_plan(
        &self,
        plan: crate::targets::RedactionPlan,
    ) -> PdfResult<NormalizedRedactionPlan> {
        let sizes = self
            .parsed
            .pages
            .iter()
            .map(|page| page.page_box.size())
            .collect::<Vec<_>>();
        normalize_plan(plan, &sizes)
    }

    fn get_page(&self, page_index: usize) -> PdfResult<&PageInfo> {
        self.parsed
            .pages
            .get(page_index)
            .ok_or(crate::objects::PdfError::InvalidPageIndex(page_index))
    }
}

/// Forms nest pictures in their own resources; a file may loop them.
fn names_picture(file: &PdfFile, resources: &PdfDictionary, depth: usize) -> bool {
    let Some(xobjects) = resources
        .get("XObject")
        .and_then(|v| file.resolve_dict(v).ok())
    else {
        return false;
    };
    xobjects.values().any(|value| {
        let PdfValue::Reference(object_ref) = value else {
            return false;
        };
        let Ok(stream) = get_stream(file, *object_ref) else {
            return false;
        };
        match stream.dict.get("Subtype").and_then(PdfValue::as_name) {
            Some("Image") => true,
            Some("Form") if depth < 8 => stream
                .dict
                .get("Resources")
                .and_then(|v| file.resolve_dict(v).ok())
                .is_some_and(|inner| names_picture(file, inner, depth + 1)),
            _ => false,
        }
    })
}

pub use crate::objects::PdfError;
pub use crate::targets::{FillColor, RedactionMode, RedactionPlan, RedactionTarget};
pub use crate::text::TextMatch;
pub use crate::text::cmap::{missing_cmaps, provide_cmap};

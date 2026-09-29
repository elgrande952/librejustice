//! A PDF: draw its pages as SVG, show where the named strings sit, remove
//! their glyphs and whatever a rectangle covers.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use lj_redact_core::{pdf, Anchor, Occurrence};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::{fail, Applied, Hit, PageSize};

/// One word of a page's selectable layer, with its box in page space.
#[derive(Serialize)]
struct WordView {
    text: String,
    bounds: [f64; 4],
}

/// A rectangle the viewer drew, in PDF user space.
#[derive(Deserialize)]
struct RectInput {
    page: usize,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// One word a text reader found on a rasterised page, in that page's own
/// coordinates. Words sharing a `line` are one run of text; a new line, or a new
/// page, breaks the run.
#[derive(Deserialize)]
struct WordInput {
    page: usize,
    line: usize,
    text: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

enum Format {
    Pdf,
    /// An image, which the page decodes and rewrites itself. The module holds it
    /// only to carry what a reader found on it.
    Scan,
}

/// A face a drawn page writes its text in, served under `/redactor/fonts`.
#[derive(Clone, Serialize)]
struct FaceView {
    family: String,
    file: &'static str,
}

/// A document held in the page, parsed once for its characters and, when it
/// draws pages, kept open for the drawing.
#[wasm_bindgen]
pub struct PdfDocument {
    format: Format,
    bytes: Vec<u8>,
    /// What the file yields on its own, which a reading adds to.
    native: Vec<(char, Anchor)>,
    characters: Vec<(char, Anchor)>,
    pages: usize,
    engine: Option<lj_pdf::PdfDocument>,
    cmaps: Vec<String>,
    faces: RefCell<BTreeMap<usize, Vec<FaceView>>>,
}

/// The picture a scanned page is made of, as the file stores it: the reader
/// reads its own pixels, and a box found there reaches the page through
/// `transform`, which stretches the picture's unit square, top left corner at
/// the origin, over the page in points with y down.
#[wasm_bindgen]
pub struct ScanPicture {
    mime: &'static str,
    data: Vec<u8>,
    transform: [f64; 6],
}

#[wasm_bindgen]
impl ScanPicture {
    #[wasm_bindgen(getter)]
    pub fn mime(&self) -> String {
        self.mime.to_string()
    }

    #[wasm_bindgen(getter)]
    pub fn data(&self) -> Vec<u8> {
        self.data.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn transform(&self) -> Vec<f64> {
        self.transform.to_vec()
    }
}

/// Hand over an Adobe CMap a document named in `cmapsNeeded`, kept for every
/// document the tab opens after.
#[wasm_bindgen(js_name = addCmap)]
pub fn add_cmap(name: String, bytes: Vec<u8>) {
    lj_pdf::provide_cmap(&name, bytes);
}

fn hit_of(occurrence: &Occurrence) -> Hit {
    let mut page = None;
    let mut quads = Vec::new();
    for (index, quad) in occurrence.anchors.iter().filter_map(Anchor::pdf) {
        page.get_or_insert(index);
        quads.push(quad);
    }
    Hit {
        needle: occurrence.needle,
        page,
        quads,
        start: occurrence.source.start,
        end: occurrence.source.end,
    }
}

#[wasm_bindgen]
impl PdfDocument {
    /// How many pages the viewer has to render.
    #[wasm_bindgen(getter, js_name = pageCount)]
    pub fn page_count(&self) -> usize {
        self.pages
    }

    /// Characters the document yields. Zero means no text layer, so there is
    /// nothing to search and the file is refused rather than returned untouched.
    #[wasm_bindgen(getter, js_name = characterCount)]
    pub fn character_count(&self) -> usize {
        self.characters.len()
    }

    /// The Adobe CMaps its text asked for and did not find. Once they are
    /// handed over, the document is opened again to be read with them.
    #[wasm_bindgen(getter, js_name = cmapsNeeded)]
    pub fn cmaps_needed(&self) -> Vec<String> {
        self.cmaps.clone()
    }

    fn engine(&self) -> Result<&lj_pdf::PdfDocument, JsError> {
        self.engine
            .as_ref()
            .ok_or_else(|| JsError::new("this document draws no page"))
    }

    /// Size of one page in its own units.
    #[wasm_bindgen(js_name = pageSize)]
    pub fn page_size(&self, index: usize) -> Result<JsValue, JsError> {
        let size = self.engine()?.page_size(index).map_err(fail)?;
        serde_wasm_bindgen::to_value(&PageSize {
            width: size.width,
            height: size.height,
        })
        .map_err(fail)
    }

    /// One page as a standalone SVG in points, on the same bytes the
    /// redaction reads.
    #[wasm_bindgen(js_name = pageSvg)]
    pub fn page_svg(&self, index: usize) -> Result<String, JsError> {
        let page = self.engine()?.draw_page(index).map_err(fail)?;
        let faces = page
            .families()
            .into_iter()
            .filter_map(|family| {
                Some(FaceView {
                    family: family.to_string(),
                    file: lj_pdf::draw::substitute_file(family)?,
                })
            })
            .collect();
        self.faces.borrow_mut().insert(index, faces);
        Ok(lj_draw::svg(&page, &format!("p{index}-")))
    }

    /// The picture a scanned page is made of, when the page is read on it
    /// rather than on its drawing.
    #[wasm_bindgen(js_name = pageScan)]
    pub fn page_scan(&self, index: usize) -> Result<Option<ScanPicture>, JsError> {
        Ok(pdf::scan_picture(self.engine()?, index)
            .map_err(fail)?
            .map(|picture| ScanPicture {
                mime: picture.mime,
                data: picture.data.to_vec(),
                transform: picture.transform,
            }))
    }

    /// The faces the last [`Self::page_svg`] of `index` writes its text in.
    #[wasm_bindgen(js_name = pageFonts)]
    pub fn page_fonts(&self, index: usize) -> Result<JsValue, JsError> {
        let faces = self.faces.borrow().get(&index).cloned().unwrap_or_default();
        serde_wasm_bindgen::to_value(&faces).map_err(fail)
    }

    /// The document as blocks of text, which is what the viewer shows when a
    /// page cannot be drawn.
    pub fn blocks(&self) -> Result<JsValue, JsError> {
        crate::blocks(&self.characters)
    }

    /// The words of one page, line by line, for the viewer's selectable layer.
    #[wasm_bindgen(js_name = pageText)]
    pub fn page_text(&self, index: usize) -> Result<JsValue, JsError> {
        let lines: Vec<Vec<WordView>> = lj_redact_core::layout::page_lines(&self.characters, index)
            .into_iter()
            .map(|line| {
                line.words
                    .into_iter()
                    .map(|w| WordView {
                        text: w.text,
                        bounds: w.bounds,
                    })
                    .collect()
            })
            .collect();
        serde_wasm_bindgen::to_value(&lines).map_err(fail)
    }

    /// What the rules propose to hide, before the user types anything.
    pub fn suggestions(&self) -> Result<JsValue, JsError> {
        crate::suggestions(lj_redact_core::suggest(&self.characters))
    }

    /// The pages a text reader has something to find on: those that name a
    /// picture, and those the file yields no character for.
    #[wasm_bindgen(js_name = pagesToRead)]
    pub fn pages_to_read(&self) -> Result<JsValue, JsError> {
        let Some(engine) = &self.engine else {
            return serde_wasm_bindgen::to_value(&(0..self.pages).collect::<Vec<_>>())
                .map_err(fail);
        };
        let written: BTreeSet<usize> = self
            .native
            .iter()
            .filter_map(|(_, anchor)| anchor.pdf().map(|(page, _)| page))
            .collect();
        let pages: Vec<usize> = (0..self.pages)
            .filter(|&page| {
                !written.contains(&page) || matches!(pdf::names_picture(engine, page), Ok(true))
            })
            .collect();
        serde_wasm_bindgen::to_value(&pages).map_err(fail)
    }

    /// Add what a text reader found on the pages to the characters the file
    /// yields on its own, replacing any earlier reading. The quads it carries
    /// are boxes around words, so what lands on them is a rectangle and never
    /// the removal of a glyph: a picture has none.
    #[wasm_bindgen(js_name = adoptWords)]
    pub fn adopt_words(&mut self, words: JsValue) -> Result<usize, JsError> {
        let words: Vec<WordInput> = serde_wasm_bindgen::from_value(words).map_err(fail)?;
        let words: Vec<pdf::ReadWord> = words
            .into_iter()
            .map(|w| pdf::ReadWord {
                page: w.page,
                line: w.line,
                text: w.text,
                x: w.x,
                y: w.y,
                width: w.width,
                height: w.height,
            })
            .collect();
        self.characters = pdf::merge_read(&self.native, &words);
        Ok(self.characters.len())
    }

    /// Where each named string sits, without touching the document.
    pub fn search(&self, needles: Vec<String>) -> Result<JsValue, JsError> {
        let hits: Vec<Hit> = lj_redact_core::locate(&self.characters, &needles)
            .iter()
            .map(hit_of)
            .collect();
        serde_wasm_bindgen::to_value(&hits).map_err(fail)
    }

    /// Remove every occurrence and whatever the drawn rectangles cover, then
    /// re-read the bytes produced to check no needle survived.
    pub fn apply(&self, needles: Vec<String>, rects: JsValue) -> Result<Applied, JsError> {
        let rects: Vec<RectInput> = match rects.is_undefined() || rects.is_null() {
            true => Vec::new(),
            false => serde_wasm_bindgen::from_value(rects).map_err(fail)?,
        };
        let rects: Vec<pdf::PageRect> = rects
            .into_iter()
            .map(|r| pdf::PageRect {
                page: r.page,
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            })
            .collect();
        let outcome = match self.format {
            Format::Pdf => pdf::redact(&self.bytes, &needles, &rects).map_err(fail)?,
            Format::Scan => return Err(JsError::new("the page rewrites an image itself")),
        };
        Ok(outcome.into())
    }
}

/// Parse `bytes` as a PDF, kept open for the drawing.
pub(crate) fn open(bytes: Vec<u8>) -> Result<PdfDocument, JsError> {
    lj_pdf::missing_cmaps();
    let characters = pdf::glyphs(&bytes).map_err(fail)?;
    let engine = lj_pdf::PdfDocument::open(&bytes).map_err(fail)?;
    Ok(PdfDocument {
        format: Format::Pdf,
        bytes,
        native: characters.clone(),
        characters,
        pages: engine.page_count(),
        engine: Some(engine),
        cmaps: lj_pdf::missing_cmaps(),
        faces: RefCell::default(),
    })
}

/// An empty document for an image, which the module never parses. It exists to
/// hold what a reader found on the picture and to answer the same questions as
/// any other document about it.
#[wasm_bindgen(js_name = openScan)]
pub fn open_scan() -> PdfDocument {
    PdfDocument {
        format: Format::Scan,
        bytes: Vec::new(),
        native: Vec::new(),
        characters: Vec::new(),
        pages: 1,
        engine: None,
        cmaps: Vec::new(),
        faces: RefCell::default(),
    }
}

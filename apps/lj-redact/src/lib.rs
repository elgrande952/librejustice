//! Browser binding of `lj-redact-core`: one module reads a PDF, a scan and a
//! Word document.

mod office;
mod pdf;

use lj_redact_core::{Anchor, Outcome};
use serde::Serialize;
use wasm_bindgen::prelude::*;

type Quad = [[f64; 2]; 4];

/// Where one occurrence sits on one page, and where it sits in the character
/// feed the text view shows. A character no page draws, in a comment or the
/// properties, has no page.
#[derive(Serialize)]
struct Hit {
    needle: usize,
    page: Option<usize>,
    quads: Vec<Quad>,
    start: usize,
    end: usize,
}

/// One paragraph of the text view, addressed by its position in the character
/// feed, which is the coordinate a hit also carries.
#[derive(Serialize)]
struct BlockView {
    start: usize,
    text: String,
}

#[derive(Serialize)]
struct SuggestionView {
    text: String,
    kind: &'static str,
}

/// A page in its own coordinates, which is what the overlay draws against.
#[derive(Serialize)]
struct PageSize {
    width: f64,
    height: f64,
}

fn fail(e: impl std::fmt::Display) -> JsError {
    JsError::new(&format!("{e:#}"))
}

/// The document as blocks of text, for the text view.
fn blocks(characters: &[(char, Anchor)]) -> Result<JsValue, JsError> {
    let blocks: Vec<BlockView> = lj_redact_core::blocks(characters)
        .into_iter()
        .map(|b| BlockView {
            start: b.start,
            text: b.text,
        })
        .collect();
    serde_wasm_bindgen::to_value(&blocks).map_err(fail)
}

/// What the rules propose to hide, before the user types anything.
fn suggestions(found: Vec<lj_redact_core::Suggestion>) -> Result<JsValue, JsError> {
    let found: Vec<SuggestionView> = found
        .into_iter()
        .map(|s| SuggestionView {
            text: s.text,
            kind: s.kind.name(),
        })
        .collect();
    serde_wasm_bindgen::to_value(&found).map_err(fail)
}

/// What one pass produced, with the proof it was re-read.
#[wasm_bindgen]
pub struct Applied {
    bytes: Vec<u8>,
    found: usize,
    removed: usize,
    leaks: Vec<String>,
}

impl From<Outcome> for Applied {
    fn from(outcome: Outcome) -> Self {
        Self {
            bytes: match outcome.leaks.is_empty() {
                true => outcome.bytes,
                false => Vec::new(),
            },
            found: outcome.found,
            removed: outcome.removed,
            leaks: outcome.leaks,
        }
    }
}

#[wasm_bindgen]
impl Applied {
    /// The redacted document. Empty when `leaks` is not.
    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn found(&self) -> usize {
        self.found
    }

    #[wasm_bindgen(getter)]
    pub fn removed(&self) -> usize {
        self.removed
    }

    /// Strings still readable in what was produced. Non-empty means the file is
    /// withheld rather than handed over.
    #[wasm_bindgen(getter)]
    pub fn leaks(&self) -> Vec<String> {
        self.leaks.clone()
    }
}

/// A page image made ready for the text reader, and the way back from its
/// pixels to the image it came from.
#[wasm_bindgen]
pub struct PreparedScan(lj_redact_core::scan::Prepared);

#[wasm_bindgen]
impl PreparedScan {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.0.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.0.height
    }

    /// The prepared image as opaque RGBA, the shape `ImageData` takes.
    #[wasm_bindgen(getter)]
    pub fn pixels(&self) -> Vec<u8> {
        let pixels: Vec<[u8; 4]> = self.0.grey.iter().map(|&g| [g, g, g, 255]).collect();
        pixels.into_flattened()
    }

    /// `[x, y, width, height]` on the source image of a box read on this one.
    #[wasm_bindgen(js_name = sourceBox)]
    pub fn source_box(&self, x: f64, y: f64, width: f64, height: f64) -> Vec<f64> {
        self.0.source_box(x, y, width, height).to_vec()
    }
}

/// Flatten, straighten and, when its print is small, enlarge a page image
/// given as straight-alpha RGBA.
#[wasm_bindgen(js_name = prepareScan)]
pub fn prepare_scan(rgba: &[u8], width: u32, height: u32) -> PreparedScan {
    PreparedScan(lj_redact_core::scan::prepare(rgba, width, height))
}

/// Read a PDF, a `.docx`, an `.odt`, a `.doc` or a web page, refusing anything else.
#[wasm_bindgen]
pub fn open(bytes: Vec<u8>) -> Result<JsValue, JsError> {
    match bytes.starts_with(b"%PDF") {
        true => pdf::open(bytes).map(JsValue::from),
        false => office::open(bytes).map(JsValue::from),
    }
}

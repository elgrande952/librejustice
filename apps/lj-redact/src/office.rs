//! The Word module: lay a word-processing document out on the faces the page
//! fetched, draw its pages, show where the named strings sit, remove them.

use std::collections::BTreeMap;

use lj_office::{Fonts, Layout};
use lj_redact_core::labels::{Labels, Needle};
use lj_redact_core::Anchor;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::{fail, Applied, Hit, PageSize, Quad};

/// A string to replace, filed under the family the page lists it in.
#[derive(Deserialize)]
struct Entry {
    text: String,
    kind: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    held: bool,
}

#[derive(Serialize)]
struct LabelView<'a> {
    text: &'a str,
    label: &'a str,
}

fn entries(value: JsValue) -> Result<Vec<Entry>, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(fail)
}

fn needles(entries: &[Entry]) -> Result<Vec<Needle<'_>>, JsError> {
    entries
        .iter()
        .map(|e| Needle::filed(&e.text, &e.kind, e.label.as_deref(), e.held))
        .collect::<anyhow::Result<_>>()
        .map_err(fail)
}

/// The label of each entry not held, in the order given.
#[wasm_bindgen]
pub fn labels(entries: JsValue) -> Result<JsValue, JsError> {
    let entries = self::entries(entries)?;
    let labels = Labels::assign(&needles(&entries)?);
    let shown: Vec<LabelView> = entries
        .iter()
        .filter(|e| !e.held)
        .enumerate()
        .map(|(i, e)| LabelView {
            text: &e.text,
            label: labels.label(i),
        })
        .collect();
    serde_wasm_bindgen::to_value(&shown).map_err(fail)
}

/// A font file the page fetches from `/redactor/fonts`, and the family it
/// declares it under, which is the one the pages draw with.
#[derive(Serialize)]
struct FaceView {
    family: &'static str,
    file: &'static str,
}

/// A document held in the page: its model, laid out once the faces it names
/// have been handed over, and the character feed its redaction searches,
/// with the views of it tracked changes and hidden text make.
#[wasm_bindgen]
pub struct OfficeDocument {
    bytes: Vec<u8>,
    model: lj_office::Document,
    characters: Vec<(char, Anchor)>,
    views: Vec<Vec<usize>>,
    fonts: Fonts,
    layout: Option<Layout>,
}

impl OfficeDocument {
    fn laid_out(&self) -> Result<&Layout, JsError> {
        self.layout
            .as_ref()
            .ok_or_else(|| JsError::new("the document has not been laid out"))
    }

    fn page(&self, index: usize) -> Result<&Layout, JsError> {
        let layout = self.laid_out()?;
        match index < layout.page_count() {
            true => Ok(layout),
            false => Err(JsError::new(&format!("no page {index}"))),
        }
    }
}

#[wasm_bindgen]
impl OfficeDocument {
    /// The font files the document is drawn from, known before any is loaded.
    #[wasm_bindgen(js_name = fontsNeeded)]
    pub fn fonts_needed(&self) -> Result<JsValue, JsError> {
        let faces: Vec<FaceView> = self
            .model
            .fonts_needed()
            .into_iter()
            .map(|file| FaceView {
                family: lj_office::fonts::vendored_name(file)
                    .expect("fonts_needed names catalogued files"),
                file,
            })
            .collect();
        serde_wasm_bindgen::to_value(&faces).map_err(fail)
    }

    /// Hand over the bytes of a file [`Self::fonts_needed`] named.
    #[wasm_bindgen(js_name = addFont)]
    pub fn add_font(&mut self, _file: String, bytes: Vec<u8>) -> Result<(), JsError> {
        self.fonts.add(bytes).map_err(fail)?;
        Ok(())
    }

    /// Break lines and pages on the faces handed over.
    pub fn layout(&mut self) -> Result<(), JsError> {
        if self.fonts.is_empty() {
            return Err(JsError::new("no face was handed over to lay out with"));
        }
        self.layout = Some(self.model.layout(&self.fonts));
        Ok(())
    }

    #[wasm_bindgen(getter, js_name = pageCount)]
    pub fn page_count(&self) -> usize {
        self.layout.as_ref().map_or(0, Layout::page_count)
    }

    /// Characters the document yields. Zero means there is nothing to search,
    /// and the file is refused rather than returned untouched.
    #[wasm_bindgen(getter, js_name = characterCount)]
    pub fn character_count(&self) -> usize {
        self.characters.len()
    }

    /// Size of one page in points.
    #[wasm_bindgen(js_name = pageSize)]
    pub fn page_size(&self, index: usize) -> Result<JsValue, JsError> {
        let (width, height) = self.page(index)?.page_size(index);
        serde_wasm_bindgen::to_value(&PageSize { width, height }).map_err(fail)
    }

    /// One page as a standalone SVG in points, whose text stays text.
    #[wasm_bindgen(js_name = pageSvg)]
    pub fn page_svg(&self, index: usize) -> Result<String, JsError> {
        Ok(self.page(index)?.svg(index))
    }

    /// The document as blocks of text, for the text view.
    pub fn blocks(&self) -> Result<JsValue, JsError> {
        crate::blocks(&self.characters)
    }

    /// What the rules propose to hide, before the user types anything.
    pub fn suggestions(&self) -> Result<JsValue, JsError> {
        crate::suggestions(
            lj_redact_core::office::suggestions(&self.bytes, &self.characters, &self.views)
                .map_err(fail)?,
        )
    }

    /// Where each named string sits, without touching the document: one hit
    /// per page an occurrence is drawn on, a header repeating it on each.
    pub fn search(&self, needles: Vec<String>) -> Result<JsValue, JsError> {
        let layout = self.laid_out()?;
        let mut hits = Vec::new();
        let occurrences = lj_redact_core::office::locate(&self.characters, &self.views, &needles);
        let anchors: Vec<Vec<lj_office::Anchor>> = occurrences
            .iter()
            .map(|o| o.anchors.iter().filter_map(Anchor::office).collect())
            .collect();
        let groups: Vec<&[lj_office::Anchor]> = anchors.iter().map(Vec::as_slice).collect();
        for (occurrence, drawn) in occurrences.iter().zip(layout.quads(&groups)) {
            let mut pages: BTreeMap<usize, Vec<Quad>> = BTreeMap::new();
            for (page, quad) in drawn {
                pages.entry(page).or_default().push(quad);
            }
            let (start, end) = (occurrence.source.start, occurrence.source.end);
            if pages.is_empty() {
                hits.push(Hit {
                    needle: occurrence.needle,
                    page: None,
                    quads: Vec::new(),
                    start,
                    end,
                });
            }
            for (page, quads) in pages {
                hits.push(Hit {
                    needle: occurrence.needle,
                    page: Some(page),
                    quads,
                    start,
                    end,
                });
            }
        }
        serde_wasm_bindgen::to_value(&hits).map_err(fail)
    }

    /// Replace every occurrence of each entry by its label, then re-read the
    /// bytes produced to check no entry survived.
    pub fn redact(&self, entries: JsValue) -> Result<Applied, JsError> {
        let entries = self::entries(entries)?;
        let outcome =
            lj_redact_core::office::redact(&self.bytes, &needles(&entries)?).map_err(fail)?;
        Ok(outcome.into())
    }
}

/// Read a `.docx`, `.odt`, `.doc` or web page, refusing anything else. Its search reads
/// the characters its redaction reads, comments and properties included.
pub(crate) fn open(bytes: Vec<u8>) -> Result<OfficeDocument, JsError> {
    let model = lj_office::open(&bytes).map_err(fail)?;
    let characters = lj_redact_core::office::characters(&bytes).map_err(fail)?;
    let views = lj_redact_core::office::views(&bytes).map_err(fail)?;
    Ok(OfficeDocument {
        bytes,
        model,
        characters,
        views,
        fonts: Fonts::new(),
        layout: None,
    })
}

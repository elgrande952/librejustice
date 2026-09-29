//! The faces a layout measures with: loaded from their bytes, resolved from the
//! family a run names, shaped by HarfBuzz as LibreOffice shapes them.

mod catalog;
mod substitute;
mod symbols;

use std::collections::hash_map::{Entry, HashMap};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use harfrust::{Feature, FontRef, ShaperData, Tag, UnicodeBuffer};
use read_fonts::tables::os2::SelectionFlags;
use read_fonts::TableProvider;

use crate::model::Generic;
pub use substitute::substitute;
pub use symbols::{recode, SYMBOL_FACE};

/// Index of a face in its [`Fonts`].
pub type FaceId = usize;

/// Vertical metrics in font units, as LibreOffice reads them.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub units_per_em: u16,
    pub ascent: i32,
    pub descent: i32,
    pub line_gap: i32,
    pub underline_position: i32,
    pub underline_thickness: i32,
    pub strikeout_position: i32,
    pub strikeout_size: i32,
}

pub struct Face {
    data: Arc<[u8]>,
    index: u32,
    /// The family CSS and `@font-face` know the face by: the typographic
    /// family when the name table has one.
    pub family: String,
    /// The face's own PostScript name, which names it alone.
    pub name: String,
    /// Lowercase names a document may call the face by, the legacy family
    /// first.
    names: Vec<String>,
    pub weight: u16,
    pub italic: bool,
    /// Width class, 5 being normal.
    pub stretch: u16,
    pub metrics: Metrics,
    shaper: ShaperData,
}

impl Face {
    fn font(&self) -> FontRef<'_> {
        FontRef::from_index(&self.data, self.index).expect("the face parsed when it was added")
    }

    pub fn has(&self, c: char) -> bool {
        self.font()
            .cmap()
            .is_ok_and(|cmap| cmap.map_codepoint(c).is_some())
    }

    /// Advance of each character of `text`, in font units. A glyph standing
    /// for several characters, as a ligature does, shares its advance between
    /// them.
    pub fn advances(&self, text: &str, kerning: bool) -> Vec<f64> {
        let font = self.font();
        let shaper = self.shaper.shaper(&font).build();
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let features = [Feature::new(Tag::new(b"kern"), u32::from(kerning), ..)];
        let glyphs = shaper.shape(buffer, harfrust::ShapeOptions::new().features(&features));
        let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let mut clusters: Vec<(usize, f64)> = Vec::new();
        let mut index: HashMap<usize, usize> = HashMap::new();
        for (info, position) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
            let cluster = info.cluster as usize;
            match index.entry(cluster) {
                Entry::Occupied(at) => clusters[*at.get()].1 += position.x_advance as f64,
                Entry::Vacant(at) => {
                    at.insert(clusters.len());
                    clusters.push((cluster, position.x_advance as f64));
                }
            }
        }
        clusters.sort_by_key(|(c, _)| *c);
        let mut advances = vec![0.0; starts.len()];
        for (k, (cluster, sum)) in clusters.iter().enumerate() {
            let end = clusters.get(k + 1).map_or(text.len(), |(c, _)| *c);
            let first = starts.partition_point(|s| s < cluster);
            let last = starts.partition_point(|s| *s < end).max(first + 1);
            let share = sum / (last - first) as f64;
            for advance in &mut advances[first..last.min(starts.len())] {
                *advance = share;
            }
        }
        advances
    }
}

fn names(font: &FontRef<'_>) -> (Vec<String>, String, Option<String>) {
    let mut legacy = Vec::new();
    let mut typographic = Vec::new();
    let mut postscript = Vec::new();
    if let Ok(name) = font.name() {
        let data = name.string_data();
        for record in name.name_record() {
            let id = record.name_id().to_u16();
            if !matches!(id, 1 | 6 | 16) || !record.is_unicode() {
                continue;
            }
            let Ok(string) = record.string(data) else {
                continue;
            };
            let value: String = string.chars().collect();
            let list = match id {
                1 => &mut legacy,
                6 => &mut postscript,
                _ => &mut typographic,
            };
            if !value.is_empty() && !list.contains(&value) {
                list.push(value);
            }
        }
    }
    let family = typographic
        .first()
        .or(legacy.first())
        .cloned()
        .unwrap_or_default();
    let names = legacy
        .into_iter()
        .chain(typographic)
        .map(|n| n.to_lowercase())
        .collect();
    (names, family, postscript.into_iter().next())
}

/// LibreOffice's vertical metrics: `hhea`, the typographic ones of `OS/2`
/// when the face sets USE_TYPO_METRICS, the Windows ones when `hhea` is empty.
/// With them, the line pitch of 21 vendored faces at 11 pt, and of five of them
/// at six sizes, lands on LibreOffice's to the twip.
fn metrics(font: &FontRef<'_>) -> Result<Metrics> {
    let head = font.head()?;
    let (mut ascent, mut descent, mut line_gap) = (0, 0, 0);
    if let Ok(hhea) = font.hhea() {
        let (a, d) = (hhea.ascender().to_i16(), hhea.descender().to_i16());
        if a >= 0 && d <= 0 {
            (ascent, descent, line_gap) = (a as i32, -(d as i32), hhea.line_gap().to_i16() as i32);
        }
    }
    let mut strikeout = (0, 0);
    if let Ok(os2) = font.os2() {
        if ascent == 0 && descent == 0 {
            (ascent, descent, line_gap) =
                (os2.us_win_ascent() as i32, os2.us_win_descent() as i32, 0);
        }
        let (a, d) = (os2.s_typo_ascender(), os2.s_typo_descender());
        if os2
            .fs_selection()
            .contains(SelectionFlags::USE_TYPO_METRICS)
            && a >= 0
            && d <= 0
        {
            (ascent, descent, line_gap) = (a as i32, -(d as i32), os2.s_typo_line_gap() as i32);
        }
        strikeout = (
            os2.y_strikeout_position() as i32,
            os2.y_strikeout_size() as i32,
        );
    }
    let units_per_em = head.units_per_em();
    let (underline_position, underline_thickness) = font
        .post()
        .map(|p| {
            (
                p.underline_position().to_i16() as i32,
                p.underline_thickness().to_i16() as i32,
            )
        })
        .unwrap_or((-(units_per_em as i32) / 10, units_per_em as i32 / 20));
    Ok(Metrics {
        units_per_em,
        ascent,
        descent,
        line_gap,
        underline_position,
        underline_thickness: underline_thickness.max(1),
        strikeout_position: if strikeout.0 == 0 {
            ascent / 3
        } else {
            strikeout.0
        },
        strikeout_size: strikeout.1.max(1),
    })
}

/// A face picked for a run: which one, and what the renderer must add to it
/// because the family has no face in that style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub face: FaceId,
    pub synthetic_bold: bool,
    pub synthetic_italic: bool,
}

/// The weight a bold run asks for, and the heaviest face that still reads as
/// regular: a bold run on such a face is emboldened.
const BOLD: u16 = 700;
const MEDIUM: u16 = 500;

/// Pick among `(weight, italic, stretch)` candidates the way CSS font matching
/// does: normal width first, then style, then the nearest weight.
fn pick(candidates: &[(u16, bool, u16)], weight: u16, italic: bool) -> Option<usize> {
    let width = candidates.iter().map(|c| c.2.abs_diff(5)).min()?;
    let style = |i: bool| {
        candidates
            .iter()
            .any(|c| c.2.abs_diff(5) == width && c.1 == i)
    };
    let want = if style(italic) { italic } else { !italic };
    let rank = |w: u16| -> (u8, u16) {
        if w == weight {
            return (0, 0);
        }
        match weight {
            400 if w == 500 => (1, 0),
            _ if weight <= 500 => {
                if w < weight {
                    (2, weight - w)
                } else {
                    (3, w - weight)
                }
            }
            _ => {
                if w > weight {
                    (2, w - weight)
                } else {
                    (3, weight - w)
                }
            }
        }
    };
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.2.abs_diff(5) == width && c.1 == want)
        .min_by_key(|(_, c)| rank(c.0))
        .map(|(i, _)| i)
}

/// The families a request tries in turn: itself, embedded or installed, its
/// Office substitute, then the fallback of its generic family (`Noto Sans`,
/// `DejaVu Sans Mono` for a fixed pitch, `Noto Serif` otherwise), as
/// LibreOffice falls back.
fn families(family: &str, generic: Generic) -> [String; 3] {
    let fallback = match generic {
        Generic::Swiss => "Noto Sans",
        Generic::Fixed => "DejaVu Sans Mono",
        _ => "Noto Serif",
    };
    let family = substitute::requested(family);
    [
        family.to_string(),
        substitute(family).unwrap_or(family).to_string(),
        fallback.to_string(),
    ]
}

/// Whether a file reads as a font [`Fonts::add`] takes.
pub fn readable(bytes: &[u8]) -> bool {
    Fonts::new()
        .add(bytes.to_vec())
        .is_ok_and(|faces| faces > 0)
}

#[derive(Default)]
pub struct Fonts {
    faces: Vec<Face>,
}

impl Fonts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add every face of a font or collection file, returning how many.
    pub fn add(&mut self, bytes: Vec<u8>) -> Result<usize> {
        let data: Arc<[u8]> = Arc::from(bytes);
        let file = read_fonts::FileRef::new(&data).map_err(|e| anyhow!("reading a font: {e}"))?;
        let mut added = 0;
        for (index, font) in file.fonts().enumerate() {
            let font = font.map_err(|e| anyhow!("reading a font: {e}"))?;
            let (names, family, postscript) = names(&font);
            let metrics = metrics(&font).map_err(|e| anyhow!("reading font metrics: {e}"))?;
            let (weight, italic, stretch) = match font.os2() {
                Ok(os2) => (
                    os2.us_weight_class(),
                    os2.fs_selection().contains(SelectionFlags::ITALIC),
                    os2.us_width_class(),
                ),
                Err(_) => (400, false, 5),
            };
            let shaper = ShaperData::new(&font);
            let name = postscript.unwrap_or_else(|| {
                let style = if italic { "Italic" } else { "" };
                format!("{}-{weight}{style}", family.replace(' ', ""))
            });
            self.faces.push(Face {
                data: data.clone(),
                index: index as u32,
                family,
                name,
                names,
                weight,
                italic,
                stretch,
                metrics,
                shaper,
            });
            added += 1;
        }
        Ok(added)
    }

    /// Every `.ttf`, `.otf` and `.ttc` of a directory.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_dir(dir: impl AsRef<std::path::Path>) -> Result<Self> {
        let mut fonts = Self::new();
        let mut paths: Vec<_> = std::fs::read_dir(dir.as_ref())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc")
                })
            })
            .collect();
        paths.sort();
        for path in paths {
            fonts.add(std::fs::read(&path)?)?;
        }
        Ok(fonts)
    }

    pub fn face(&self, id: FaceId) -> &Face {
        &self.faces[id]
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }

    /// Faces a family name matches: by legacy family if any, else by
    /// typographic family.
    fn family(&self, name: &str) -> Vec<FaceId> {
        let name = name.to_lowercase();
        let legacy: Vec<FaceId> = (0..self.faces.len())
            .filter(|&i| self.faces[i].names.first() == Some(&name))
            .collect();
        if !legacy.is_empty() {
            return legacy;
        }
        (0..self.faces.len())
            .filter(|&i| self.faces[i].names.contains(&name))
            .collect()
    }

    fn pick_in(&self, faces: &[FaceId], bold: bool, italic: bool) -> Option<Resolved> {
        let candidates: Vec<(u16, bool, u16)> = faces
            .iter()
            .map(|&i| {
                let f = &self.faces[i];
                (f.weight, f.italic, f.stretch)
            })
            .collect();
        let weight = if bold { BOLD } else { 400 };
        let face = faces[pick(&candidates, weight, italic)?];
        let found = &self.faces[face];
        Some(Resolved {
            face,
            synthetic_bold: bold && found.weight <= MEDIUM,
            synthetic_italic: italic && !found.italic,
        })
    }

    /// The face a run naming `family` is drawn with.
    pub fn resolve(
        &self,
        family: &str,
        bold: bool,
        italic: bool,
        generic: Generic,
    ) -> Option<Resolved> {
        for name in families(family, generic) {
            let faces = self.family(&name);
            if !faces.is_empty() {
                return self.pick_in(&faces, bold, italic);
            }
        }
        let all: Vec<FaceId> = (0..self.faces.len()).collect();
        self.pick_in(&all, bold, italic)
    }

    /// Where a character the run's face lacks is drawn: the glyph fallback
    /// family, else the first face that has it.
    pub fn fallback(&self, c: char, bold: bool, italic: bool) -> Option<Resolved> {
        let faces = self.family(substitute::GLYPH_FALLBACK);
        if let Some(found) = self.pick_in(&faces, bold, italic) {
            if self.faces[found.face].has(c) {
                return Some(found);
            }
        }
        let face = (0..self.faces.len()).find(|&i| self.faces[i].has(c))?;
        Some(Resolved {
            face,
            synthetic_bold: bold && self.faces[face].weight <= MEDIUM,
            synthetic_italic: italic && !self.faces[face].italic,
        })
    }
}

/// The vendored file a run naming `family` is drawn from, known before any
/// face is loaded.
pub fn vendored_file(
    family: &str,
    bold: bool,
    italic: bool,
    generic: Generic,
) -> Option<&'static str> {
    let table = catalog::VENDORED;
    for name in families(family, generic) {
        let faces: Vec<usize> = (0..table.len())
            .filter(|&i| table[i].1.eq_ignore_ascii_case(&name))
            .collect();
        if faces.is_empty() {
            continue;
        }
        let candidates: Vec<(u16, bool, u16)> = faces
            .iter()
            .map(|&i| (table[i].3, table[i].4, table[i].5))
            .collect();
        let weight = if bold { BOLD } else { 400 };
        return pick(&candidates, weight, italic).map(|k| table[faces[k]].0);
    }
    None
}

/// The name the face of a vendored file is drawn under, its PostScript one.
pub fn vendored_name(file: &str) -> Option<&'static str> {
    catalog::VENDORED
        .iter()
        .find(|entry| entry.0 == file)
        .map(|entry| entry.2)
}

/// The vendored file characters their own face lacks are drawn from.
pub fn fallback_file(bold: bool, italic: bool) -> &'static str {
    vendored_file(substitute::GLYPH_FALLBACK, bold, italic, Generic::Swiss)
        .expect("the glyph fallback is vendored")
}

#[cfg(test)]
mod tests {
    use super::pick;

    #[test]
    fn weights_resolve_as_css_matching_does() {
        let bookman = [
            (300, false, 5),
            (300, true, 5),
            (600, false, 5),
            (600, true, 5),
        ];
        assert_eq!(pick(&bookman, 400, false), Some(0));
        assert_eq!(pick(&bookman, 700, false), Some(2));
        assert_eq!(pick(&bookman, 700, true), Some(3));

        let selawik = [
            (400, false, 5),
            (700, false, 5),
            (300, false, 5),
            (600, false, 5),
        ];
        assert_eq!(pick(&selawik, 400, true), Some(0));
        assert_eq!(pick(&selawik, 700, false), Some(1));

        let dejavu = [(400, false, 4), (400, false, 5), (700, false, 5)];
        assert_eq!(pick(&dejavu, 400, false), Some(1));
    }
}

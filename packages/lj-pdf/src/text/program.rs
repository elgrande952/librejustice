//! Glyph outlines read from the font program a file embeds, and the glyph
//! each code of a PDF font selects in it.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use lj_draw::Segment;
use read_fonts::model::pen::OutlinePen;
use read_fonts::ps::cff::CffFontRef;
use read_fonts::ps::encoding::PredefinedEncoding;
use read_fonts::ps::type1::Type1Font;
use read_fonts::tables::cmap::{CmapSubtable, PlatformId};
use read_fonts::tables::glyf::{Anchor, CompositeGlyphFlags, Glyph as GlyfGlyph};
use read_fonts::types::{GlyphId, GlyphId16, Tag};
use read_fonts::{FontRef, TableProvider};

use super::{
    SimpleEncodingBase, base_char, glyph_name_to_char, mac_roman_byte_to_char,
    matrix_from_pdf_values,
};
use crate::graphics::Matrix;
use crate::objects::{ObjectRef, PdfDictionary, PdfFile, PdfValue, decode_stream};

/// Outline coordinates count thousandths of the em, as PDF widths do, y up.
pub(crate) const UNITS: f64 = 1000.0;

/// Nesting a composite glyph may reach before it is taken for a loop.
const COMPOSITE_DEPTH: usize = 8;

/// What draws a decoded glyph.
pub(crate) enum Drawn<'a> {
    /// The file embeds nothing to draw it with: text in a free face.
    Text,
    /// A glyph of the embedded program.
    Outline(&'a Arc<Program>, u32),
    /// A Type3 glyph procedure.
    Procedure(&'a Type3, ObjectRef),
    /// The font draws this code with no glyph.
    Nothing,
}

/// What draws the glyphs of a simple font.
#[derive(Debug, Clone)]
pub(crate) enum SimpleGlyphs {
    /// The glyph each one-byte code selects in the embedded program.
    Program {
        program: Arc<Program>,
        codes: Arc<[Option<u32>]>,
    },
    Type3(Arc<Type3>),
}

#[derive(Debug)]
pub(crate) struct Type3 {
    /// Glyph space to text space.
    pub matrix: Matrix,
    /// The procedure each code runs.
    pub procs: BTreeMap<u8, ObjectRef>,
    /// `None` when the procedures take the resources of the page.
    pub resources: Option<PdfDictionary>,
}

impl Type3 {
    pub(crate) fn load(
        file: &PdfFile,
        font: &PdfDictionary,
        differences: &BTreeMap<u8, String>,
    ) -> Type3 {
        let procs = font
            .get("CharProcs")
            .and_then(|v| file.resolve_dict(v).ok())
            .map(|procs| {
                differences
                    .iter()
                    .filter_map(|(code, name)| match procs.get(name) {
                        Some(PdfValue::Reference(procedure)) => Some((*code, *procedure)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let matrix = font
            .get("FontMatrix")
            .and_then(|v| file.resolve(v).ok())
            .and_then(PdfValue::as_array)
            .and_then(|m| matrix_from_pdf_values(m).ok())
            .unwrap_or(Matrix::scale(0.001, 0.001));
        Type3 {
            matrix,
            procs,
            resources: font
                .get("Resources")
                .and_then(|v| file.resolve_dict(v).ok())
                .cloned(),
        }
    }
}

pub(crate) struct Program {
    kind: Kind,
    /// The glyph each name reaches, for a program that names its glyphs.
    names: HashMap<Box<str>, u32>,
    /// The glyph each character reaches by its name.
    chars: HashMap<char, u32>,
    /// CID to glyph, for a CID-keyed CFF.
    cids: Option<HashMap<u32, u32>>,
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            Kind::TrueType(_) => "TrueType",
            Kind::Cff { .. } => "CFF",
            Kind::Type1(_) => "Type1",
        };
        f.debug_struct("Program").field("kind", &kind).finish()
    }
}

enum Kind {
    TrueType(Arc<[u8]>),
    /// A CFF program, with the OpenType wrapper that carries its cmap.
    Cff {
        cff: Arc<[u8]>,
        sfnt: Option<Arc<[u8]>>,
    },
    Type1(Box<Type1Font>),
}

impl Program {
    /// The program a font descriptor embeds, `None` when it embeds none or
    /// one this reader cannot open.
    pub(crate) fn load(file: &PdfFile, descriptor: &PdfDictionary) -> Option<Arc<Program>> {
        let key = ["FontFile", "FontFile2", "FontFile3"]
            .into_iter()
            .find_map(|key| match descriptor.get(key) {
                Some(PdfValue::Reference(reference)) => Some(*reference),
                _ => None,
            })?;
        if let Some(program) = file.font_programs.lock().unwrap().outlines.get(&key) {
            return program.clone();
        }
        let program = crate::objects::document::get_stream(file, key)
            .ok()
            .and_then(|stream| decode_stream(stream).ok())
            .and_then(|data| Program::parse(data.into()))
            .map(Arc::new);
        file.font_programs
            .lock()
            .unwrap()
            .outlines
            .insert(key, program.clone());
        program
    }

    /// Reads a program by its first bytes rather than by the key that holds
    /// it, which producers get wrong.
    pub(crate) fn parse(data: Arc<[u8]>) -> Option<Program> {
        let kind = match data.get(..4)? {
            [0, 1, 0, 0] | b"true" | b"ttcf" => {
                let font = sfnt(&data)?;
                match font.table_data(Tag::new(b"CFF ")) {
                    Some(cff) if font.table_data(Tag::new(b"glyf")).is_none() => Kind::Cff {
                        cff: cff.as_bytes().into(),
                        sfnt: Some(data.clone()),
                    },
                    _ => Kind::TrueType(data.clone()),
                }
            }
            b"OTTO" => {
                let cff = sfnt(&data)?
                    .table_data(Tag::new(b"CFF "))?
                    .as_bytes()
                    .into();
                Kind::Cff {
                    cff,
                    sfnt: Some(data.clone()),
                }
            }
            [1, _, _, _] => Kind::Cff {
                cff: data.clone(),
                sfnt: None,
            },
            _ => Kind::Type1(Box::new(Type1Font::new(&data).ok()?)),
        };
        let mut cids = None;
        let names: Vec<(u32, String)> = match &kind {
            Kind::TrueType(_) => Vec::new(),
            Kind::Cff { cff, .. } => {
                let font = CffFontRef::new(cff, 0, None).ok()?;
                let charset = font.charset();
                let entries = charset.iter().flat_map(|charset| charset.iter());
                match font.is_cid() {
                    true => {
                        cids = Some(
                            entries
                                .map(|(gid, cid)| (u32::from(cid.to_u16()), gid.to_u32()))
                                .collect(),
                        );
                        Vec::new()
                    }
                    false => entries
                        .filter_map(|(gid, sid)| {
                            let name = std::str::from_utf8(font.string(sid)?).ok()?;
                            Some((gid.to_u32(), name.to_string()))
                        })
                        .collect(),
                }
            }
            Kind::Type1(font) => font
                .glyph_names()
                .map(|(gid, name)| (gid.to_u32(), name.to_string()))
                .collect(),
        };
        let mut program = Program {
            kind,
            names: HashMap::new(),
            chars: HashMap::new(),
            cids,
        };
        for (gid, name) in names {
            program.name(&name, gid);
        }
        Some(program)
    }

    fn name(&mut self, name: &str, gid: u32) {
        if name == ".notdef" {
            return;
        }
        self.names.entry(name.into()).or_insert(gid);
        if let Some(character) = glyph_name_to_char(name) {
            self.chars.entry(character).or_insert(gid);
        }
    }

    /// The outline of glyph `gid` in thousandths of the em, y up; `None` for
    /// a glyph the program lacks or cannot draw.
    pub(crate) fn outline(&self, gid: u32) -> Option<Vec<Segment>> {
        let mut pen = Pen::default();
        match &self.kind {
            Kind::TrueType(data) => {
                let font = sfnt(data)?;
                let upem = f64::from(font.head().ok()?.units_per_em()).max(1.0);
                let mut points = Vec::new();
                let mut ends = Vec::new();
                truetype_points(&font, gid, 0, &mut points, &mut ends)?;
                pen.scale = UNITS / upem;
                pen.contours(&points, &ends);
            }
            Kind::Cff { cff, .. } => {
                let font = CffFontRef::new(cff, 0, None).ok()?;
                let gid = GlyphId::new(gid);
                let subfont = font
                    .subfont(font.subfont_index(gid).unwrap_or(0), &[])
                    .ok()?;
                pen.scale = UNITS / f64::from(font.upem().max(1));
                font.draw(&subfont, gid, &[], None, &mut pen).ok()?;
            }
            Kind::Type1(font) => {
                pen.scale = UNITS / f64::from(font.upem().max(1));
                font.draw(GlyphId::new(gid), None, &mut pen).ok()?;
            }
        }
        Some(pen.segments)
    }

    /// The glyph a CID selects: through the charset of a CID-keyed CFF,
    /// else through `CIDToGIDMap`, which MuPDF reads whatever the program,
    /// else the CID itself.
    pub(crate) fn cid_glyph(&self, cid: u32, cid_to_gid: Option<&[u16]>) -> Option<u32> {
        match (&self.cids, cid_to_gid) {
            (Some(cids), _) => cids.get(&cid).copied(),
            (None, Some(map)) => map
                .get(usize::try_from(cid).ok()?)
                .map(|gid| u32::from(*gid)),
            (None, None) => Some(cid),
        }
    }

    /// The glyph each one-byte code selects. `base` is the base encoding the
    /// font names, `None` when it leaves the program's own in force.
    pub(super) fn code_glyphs(
        &self,
        differences: &BTreeMap<u8, String>,
        base: Option<SimpleEncodingBase>,
        symbolic: bool,
    ) -> Vec<Option<u32>> {
        match &self.kind {
            Kind::TrueType(data) => sfnt_code_glyphs(data, differences, base, symbolic),
            Kind::Cff {
                sfnt: Some(data), ..
            } if self.names.is_empty() => sfnt_code_glyphs(data, differences, base, symbolic),
            Kind::Cff { .. } | Kind::Type1(_) => (0..=255u8)
                .map(|code| match differences.get(&code) {
                    Some(name) => self.names.get(name.as_str()).copied().or_else(|| {
                        glyph_name_to_char(name).and_then(|c| self.chars.get(&c).copied())
                    }),
                    None => match base {
                        Some(base) => {
                            base_char(base, code).and_then(|c| self.chars.get(&c).copied())
                        }
                        None => self.builtin(code),
                    },
                })
                .collect(),
        }
    }

    /// The glyph the program's own encoding gives a code.
    fn builtin(&self, code: u8) -> Option<u32> {
        let standard = || {
            self.names
                .get(PredefinedEncoding::Standard.name(code))
                .copied()
        };
        match &self.kind {
            Kind::Type1(font) => font
                .encoding()
                .and_then(|encoding| encoding.glyph_name(code))
                .and_then(|name| self.names.get(name).copied())
                .or_else(standard),
            Kind::Cff { cff, .. } => CffFontRef::new(cff, 0, None)
                .ok()
                .and_then(|font| font.encoding())
                .and_then(|encoding| encoding.map(code))
                .map(|gid| gid.to_u32())
                .filter(|gid| *gid != 0)
                .or_else(standard),
            Kind::TrueType(_) => None,
        }
    }
}

fn sfnt(data: &[u8]) -> Option<FontRef<'_>> {
    FontRef::new(data)
        .or_else(|_| FontRef::from_index(data, 0))
        .ok()
}

/// A TrueType code reaches its glyph through the cmap its symbolic flag picks,
/// the `post` names and the other cmaps standing behind.
fn sfnt_code_glyphs(
    data: &[u8],
    differences: &BTreeMap<u8, String>,
    base: Option<SimpleEncodingBase>,
    symbolic: bool,
) -> Vec<Option<u32>> {
    let Some(font) = sfnt(data) else {
        return vec![None; 256];
    };
    let cmap = font.cmap().ok();
    let subtable = |platform: PlatformId, encoding: Option<u16>| -> Option<CmapSubtable<'_>> {
        let cmap = cmap.as_ref()?;
        cmap.encoding_records()
            .iter()
            .find(|r| r.platform_id() == platform && encoding.is_none_or(|e| r.encoding_id() == e))
            .and_then(|r| r.subtable(cmap.offset_data()).ok())
    };
    let unicode = subtable(PlatformId::Windows, Some(1))
        .or_else(|| subtable(PlatformId::Windows, Some(10)))
        .or_else(|| subtable(PlatformId::Unicode, None));
    let symbol = subtable(PlatformId::Windows, Some(0));
    let mac = subtable(PlatformId::Macintosh, Some(0));
    let map = |table: &Option<CmapSubtable>, code: u32| {
        table
            .as_ref()
            .and_then(|t| t.map_codepoint(code))
            .map(|gid| gid.to_u32())
            .filter(|gid| *gid != 0)
    };
    let symbol_code = |code: u8| {
        [0u32, 0xF000, 0xF100, 0xF200]
            .into_iter()
            .find_map(|offset| map(&symbol, offset + u32::from(code)))
    };
    let post = font.post().ok();
    let glyphs = font.maxp().map_or(0, |maxp| maxp.num_glyphs());
    let mut post_names: Option<HashMap<&str, u32>> = None;
    let mut post_chars: Option<HashMap<char, u32>> = None;
    let base = base.unwrap_or(match symbolic {
        true => SimpleEncodingBase::Identity,
        false => SimpleEncodingBase::Standard,
    });
    let no_cmap = unicode.is_none() && symbol.is_none() && mac.is_none();
    (0..=255u8)
        .map(|code| {
            let name = differences.get(&code).map(String::as_str);
            let character = match name {
                Some(name) => glyph_name_to_char(name),
                None if symbolic => None,
                None => base_char(base, code),
            };
            let mut by_name = || {
                let name = name?;
                let names = post_names.get_or_insert_with(|| {
                    let Some(post) = post.as_ref() else {
                        return HashMap::new();
                    };
                    (1..glyphs)
                        .filter_map(|gid| {
                            Some((post.glyph_name(GlyphId16::new(gid))?, u32::from(gid)))
                        })
                        .collect()
                });
                names.get(name).copied()
            };
            let mut by_post_char = || {
                let character = character?;
                let chars = post_chars.get_or_insert_with(|| {
                    let Some(post) = post.as_ref() else {
                        return HashMap::new();
                    };
                    let mut chars = HashMap::new();
                    for gid in 1..glyphs {
                        if let Some(c) = post
                            .glyph_name(GlyphId16::new(gid))
                            .and_then(glyph_name_to_char)
                        {
                            chars.entry(c).or_insert(u32::from(gid));
                        }
                    }
                    chars
                });
                chars.get(&character).copied()
            };
            let by_unicode = || character.and_then(|c| map(&unicode, u32::from(c)));
            let by_mac = || {
                let code = match character {
                    Some(c) => (0..=255u8).find(|b| mac_roman_byte_to_char(*b) == Some(c))?,
                    None => code,
                };
                map(&mac, u32::from(code))
            };
            let found = match (symbolic, &unicode, &symbol) {
                (false, Some(_), _) => by_unicode().or_else(by_mac),
                (false, None, _) if mac.is_some() => by_mac(),
                (true, _, Some(_)) => symbol_code(code),
                _ => None,
            };
            found
                .or_else(&mut by_name)
                .or_else(|| symbol_code(code))
                .or_else(|| map(&mac, u32::from(code)))
                .or_else(by_unicode)
                .or_else(|| map(&unicode, u32::from(code)))
                .or_else(&mut by_post_char)
                .or_else(|| (no_cmap && u16::from(code) < glyphs).then_some(u32::from(code)))
        })
        .collect()
}

type Point = (f64, f64, bool);

fn truetype_points(
    font: &FontRef,
    gid: u32,
    depth: usize,
    points: &mut Vec<Point>,
    ends: &mut Vec<usize>,
) -> Option<()> {
    if depth > COMPOSITE_DEPTH {
        return None;
    }
    let glyf = font.glyf().ok()?;
    let loca = font.loca(None).ok()?;
    let Some(glyph) = loca.get_glyf(GlyphId::new(gid), &glyf).ok()? else {
        return Some(());
    };
    match glyph {
        GlyfGlyph::Simple(simple) => {
            let start = points.len();
            points.extend(
                simple
                    .points()
                    .map(|p| (f64::from(p.x), f64::from(p.y), p.on_curve)),
            );
            let count = points.len() - start;
            for end in simple.end_pts_of_contours() {
                let end = usize::from(end.get());
                if end >= count {
                    break;
                }
                ends.push(start + end);
            }
        }
        GlyfGlyph::Composite(composite) => {
            for component in composite.components() {
                let mut own = Vec::new();
                let mut own_ends = Vec::new();
                truetype_points(
                    font,
                    u32::from(component.glyph.to_u16()),
                    depth + 1,
                    &mut own,
                    &mut own_ends,
                )?;
                let t = component.transform;
                let (xx, yx, xy, yy) = (
                    f64::from(t.xx.to_f32()),
                    f64::from(t.yx.to_f32()),
                    f64::from(t.xy.to_f32()),
                    f64::from(t.yy.to_f32()),
                );
                for p in &mut own {
                    *p = (xx * p.0 + xy * p.1, yx * p.0 + yy * p.1, p.2);
                }
                let (dx, dy) = match component.anchor {
                    Anchor::Offset { x, y } => {
                        let (x, y) = (f64::from(x), f64::from(y));
                        let scaled = component
                            .flags
                            .contains(CompositeGlyphFlags::SCALED_COMPONENT_OFFSET)
                            && !component
                                .flags
                                .contains(CompositeGlyphFlags::UNSCALED_COMPONENT_OFFSET);
                        match scaled {
                            true => (xx * x + xy * y, yx * x + yy * y),
                            false => (x, y),
                        }
                    }
                    Anchor::Point { base, component } => {
                        let from = points.get(usize::from(base))?;
                        let to = own.get(usize::from(component))?;
                        (from.0 - to.0, from.1 - to.1)
                    }
                };
                let start = points.len();
                points.extend(own.into_iter().map(|p| (p.0 + dx, p.1 + dy, p.2)));
                ends.extend(own_ends.into_iter().map(|end| start + end));
            }
        }
    }
    Some(())
}

#[derive(Default)]
struct Pen {
    scale: f64,
    segments: Vec<Segment>,
    /// The current point, which a quadratic segment needs to become cubic.
    at: [f64; 2],
}

impl Pen {
    fn point(&self, x: f64, y: f64) -> [f64; 2] {
        [x * self.scale, y * self.scale]
    }

    fn quad(&mut self, control: [f64; 2], to: [f64; 2]) {
        let from = self.at;
        let c1 = [
            from[0] + 2.0 / 3.0 * (control[0] - from[0]),
            from[1] + 2.0 / 3.0 * (control[1] - from[1]),
        ];
        let c2 = [
            to[0] + 2.0 / 3.0 * (control[0] - to[0]),
            to[1] + 2.0 / 3.0 * (control[1] - to[1]),
        ];
        self.segments.push(Segment::Cubic(c1, c2, to));
        self.at = to;
    }

    /// TrueType contours: an off-curve point between two others implies the
    /// on-curve point halfway.
    fn contours(&mut self, points: &[Point], ends: &[usize]) {
        let mut start = 0;
        for &end in ends {
            if end < start {
                continue;
            }
            let contour = &points[start..=end];
            start = end + 1;
            let n = contour.len();
            let first_on = contour.iter().position(|p| p.2);
            let (origin, from) = match first_on {
                Some(i) => (self.point(contour[i].0, contour[i].1), i),
                None if n >= 2 => {
                    let (a, b) = (contour[0], contour[1]);
                    (self.point((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0), 0)
                }
                None => continue,
            };
            self.segments.push(Segment::Move(origin));
            self.at = origin;
            let mut control: Option<[f64; 2]> = None;
            let steps = if first_on.is_some() { n } else { n + 1 };
            for k in 1..steps {
                let p = contour[(from + k) % n];
                let point = self.point(p.0, p.1);
                match (p.2, control) {
                    (true, None) => {
                        self.segments.push(Segment::Line(point));
                        self.at = point;
                    }
                    (true, Some(c)) => {
                        self.quad(c, point);
                        control = None;
                    }
                    (false, None) => control = Some(point),
                    (false, Some(c)) => {
                        let mid = [(c[0] + point[0]) / 2.0, (c[1] + point[1]) / 2.0];
                        self.quad(c, mid);
                        control = Some(point);
                    }
                }
            }
            if let Some(c) = control {
                self.quad(c, origin);
            }
            self.segments.push(Segment::Close);
        }
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let point = self.point(f64::from(x), f64::from(y));
        self.segments.push(Segment::Move(point));
        self.at = point;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let point = self.point(f64::from(x), f64::from(y));
        self.segments.push(Segment::Line(point));
        self.at = point;
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let control = self.point(f64::from(cx0), f64::from(cy0));
        let to = self.point(f64::from(x), f64::from(y));
        self.quad(control, to);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let c1 = self.point(f64::from(cx0), f64::from(cy0));
        let c2 = self.point(f64::from(cx1), f64::from(cy1));
        let to = self.point(f64::from(x), f64::from(y));
        self.segments.push(Segment::Cubic(c1, c2, to));
        self.at = to;
    }

    fn close(&mut self) {
        self.segments.push(Segment::Close);
    }
}

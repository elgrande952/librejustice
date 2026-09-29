use std::collections::BTreeMap;
use std::sync::Arc;

use read_fonts::ps::cff::CffFontRef;
use read_fonts::ps::type1::Type1Font;
use read_fonts::tables::cmap::PlatformId;
use read_fonts::types::{GlyphId, GlyphId16};
use read_fonts::{FontRef, TableProvider};

use crate::objects::{PdfDictionary, PdfFile, PdfValue, decode_stream};

/// The text each one-byte code draws according to the embedded font program.
pub(crate) fn builtin_text(
    file: &PdfFile,
    descriptor: &PdfDictionary,
) -> Arc<BTreeMap<u8, String>> {
    let Some(key) = ["FontFile", "FontFile3", "FontFile2"]
        .into_iter()
        .find_map(|key| match descriptor.get(key) {
            Some(PdfValue::Reference(reference)) => Some(*reference),
            _ => None,
        })
    else {
        return Arc::default();
    };
    if let Some(text) = file.font_programs.lock().unwrap().codes.get(&key) {
        return text.clone();
    }
    let text = Arc::new(read_builtin_text(file, descriptor));
    file.font_programs
        .lock()
        .unwrap()
        .codes
        .insert(key, text.clone());
    text
}

/// The character each glyph of an embedded TrueType program draws, by its
/// name in `post` or else by the Unicode cmap that reaches it.
pub(crate) fn glyph_text(file: &PdfFile, descriptor: &PdfDictionary) -> Arc<BTreeMap<u16, char>> {
    let Some(PdfValue::Reference(key)) = descriptor.get("FontFile2") else {
        return Arc::default();
    };
    if let Some(text) = file.font_programs.lock().unwrap().glyphs.get(key) {
        return text.clone();
    }
    let text = Arc::new(
        crate::objects::document::get_stream(file, *key)
            .ok()
            .and_then(|stream| decode_stream(stream).ok())
            .map(|data| truetype_glyphs(&data))
            .unwrap_or_default(),
    );
    file.font_programs
        .lock()
        .unwrap()
        .glyphs
        .insert(*key, text.clone());
    text
}

fn truetype_glyphs(data: &[u8]) -> BTreeMap<u16, char> {
    let Ok(font) = FontRef::new(data) else {
        return BTreeMap::new();
    };
    let mut text: BTreeMap<u16, char> = BTreeMap::new();
    if let Ok(cmap) = font.cmap() {
        for record in cmap.encoding_records() {
            let unicode = matches!(
                (record.platform_id(), record.encoding_id()),
                (PlatformId::Windows, 1 | 10) | (PlatformId::Unicode, _)
            );
            let Some(table) = unicode
                .then(|| record.subtable(cmap.offset_data()).ok())
                .flatten()
            else {
                continue;
            };
            for (codepoint, gid) in table.iter() {
                if let (Ok(gid), Some(character)) =
                    (u16::try_from(gid.to_u32()), char::from_u32(codepoint))
                {
                    text.entry(gid).or_insert(character);
                }
            }
        }
    }
    if let Ok(post) = font.post() {
        let glyphs = font.maxp().map_or(0, |maxp| maxp.num_glyphs());
        for gid in 1..glyphs {
            if let Some(character) = post
                .glyph_name(GlyphId16::new(gid))
                .and_then(super::glyph_name_to_char)
            {
                text.insert(gid, character);
            }
        }
    }
    text
}

fn read_builtin_text(file: &PdfFile, descriptor: &PdfDictionary) -> BTreeMap<u8, String> {
    let program = |key: &str| {
        let stream = match descriptor.get(key)? {
            PdfValue::Reference(reference) => {
                crate::objects::document::get_stream(file, *reference).ok()?
            }
            _ => return None,
        };
        let subtype = stream
            .dict
            .get("Subtype")
            .and_then(PdfValue::as_name)
            .map(str::to_string);
        decode_stream(stream).ok().map(|data| (data, subtype))
    };
    let names = if let Some((data, _)) = program("FontFile") {
        type1_names(&data)
    } else if let Some((data, subtype)) = program("FontFile3") {
        match subtype.as_deref() {
            Some("OpenType") => return truetype_text(&data),
            _ => cff_names(&data),
        }
    } else if let Some((data, _)) = program("FontFile2") {
        return truetype_text(&data);
    } else {
        BTreeMap::new()
    };
    names
        .into_iter()
        .filter_map(|(code, name)| Some((code, super::glyph_name_to_text(&name)?)))
        .collect()
}

fn type1_names(data: &[u8]) -> BTreeMap<u8, String> {
    let Ok(font) = Type1Font::new(data) else {
        return BTreeMap::new();
    };
    let Some(encoding) = font.encoding() else {
        return BTreeMap::new();
    };
    (0..=255u8)
        .filter_map(|code| Some((code, encoding.glyph_name(code)?.to_string())))
        .filter(|(_, name)| name != ".notdef")
        .collect()
}

fn cff_names(data: &[u8]) -> BTreeMap<u8, String> {
    let Ok(font) = CffFontRef::new(data, 0, None) else {
        return BTreeMap::new();
    };
    let (Some(encoding), Some(charset)) = (font.encoding(), font.charset()) else {
        return BTreeMap::new();
    };
    (0..=255u8)
        .filter_map(|code| {
            let gid = encoding.map(code)?;
            let sid = charset.string_id(gid).ok()?;
            let name = std::str::from_utf8(font.string(sid)?).ok()?;
            (name != ".notdef").then(|| (code, name.to_string()))
        })
        .collect()
}

/// A symbolic TrueType font draws a code through its (3,0) cmap, the code
/// offset into the private area, or else through its (1,0) cmap.
fn truetype_text(data: &[u8]) -> BTreeMap<u8, String> {
    let Ok(font) = FontRef::new(data) else {
        return BTreeMap::new();
    };
    let Ok(cmap) = font.cmap() else {
        return BTreeMap::new();
    };
    let subtable = |platform: PlatformId, encoding: u16| {
        cmap.encoding_records()
            .iter()
            .find(|r| r.platform_id() == platform && r.encoding_id() == encoding)
            .and_then(|r| r.subtable(cmap.offset_data()).ok())
    };
    let symbol = subtable(PlatformId::Windows, 0);
    let mac = subtable(PlatformId::Macintosh, 0);
    let unicode = subtable(PlatformId::Windows, 1).or_else(|| subtable(PlatformId::Unicode, 3));
    let mut by_glyph: Option<BTreeMap<GlyphId, char>> = None;
    let post = font.post().ok();
    let mut text = BTreeMap::new();
    for code in 0..=255u8 {
        let gid = symbol
            .as_ref()
            .and_then(|table| {
                [0u32, 0xF000, 0xF100, 0xF200]
                    .iter()
                    .find_map(|base| table.map_codepoint(base + u32::from(code)))
            })
            .or_else(|| mac.as_ref().and_then(|table| table.map_codepoint(code)));
        let Some(gid) = gid.filter(|gid| gid.to_u32() != 0) else {
            continue;
        };
        let named = post
            .as_ref()
            .zip(GlyphId16::try_from(gid).ok())
            .and_then(|(post, gid)| post.glyph_name(gid))
            .and_then(super::glyph_name_to_char);
        let reversed = || {
            by_glyph
                .get_or_insert_with(|| {
                    unicode
                        .iter()
                        .flat_map(|table| table.iter())
                        .filter_map(|(codepoint, gid)| Some((gid, char::from_u32(codepoint)?)))
                        .collect()
                })
                .get(&gid)
                .copied()
        };
        if let Some(character) = named.or_else(reversed) {
            text.insert(code, character.to_string());
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::truetype_text;

    fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut font = vec![0, 1, 0, 0];
        font.extend((tables.len() as u16).to_be_bytes());
        font.extend([0; 6]);
        let mut offset = 12 + 16 * tables.len();
        let mut body = Vec::new();
        for (tag, data) in tables {
            font.extend(*tag);
            font.extend([0; 4]);
            font.extend((offset as u32).to_be_bytes());
            font.extend((data.len() as u32).to_be_bytes());
            body.extend(data);
            while body.len() % 4 != 0 {
                body.push(0);
            }
            offset = 12 + 16 * tables.len() + body.len();
        }
        font.extend(body);
        font
    }

    #[test]
    fn symbolic_truetype_reads_its_mac_cmap_and_names_glyphs_by_post() {
        let mut cmap = vec![0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 12, 0, 0, 1, 6, 0, 0];
        let mut glyphs = [0u8; 256];
        glyphs[0x41] = 1;
        glyphs[0x42] = 2;
        cmap.extend(glyphs);
        let mut post = vec![0, 2, 0, 0];
        post.extend([0; 28]);
        post.extend([0, 3, 0, 0, 1, 2, 0, 36]);
        post.push(7);
        post.extend(b"uni00E9");
        let text = truetype_text(&sfnt(&[(b"cmap", cmap), (b"post", post)]));
        assert_eq!(
            text.into_iter().collect::<Vec<_>>(),
            [(0x41, "\u{e9}".to_string()), (0x42, "A".to_string())]
        );
    }
}

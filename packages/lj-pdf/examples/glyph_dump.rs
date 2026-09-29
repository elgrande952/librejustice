//! Writes the glyphs extracted from every PDF under a folder, one JSON line per
//! file, for `bench/mupdf_parity.py` to compare against MuPDF.
//!
//! cargo run --release -p lj-pdf --example glyph_dump -- <folder> <out.jsonl>

use std::path::{Path, PathBuf};
use std::time::Instant;

use lj_pdf::objects::parse_pdf;
use lj_pdf::text::analyze_page_text;
use serde_json::{Value, json};

fn pdfs_under(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            pdfs_under(&path, out)?;
        } else if path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("pdf"))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn glyphs(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let parsed = parse_pdf(bytes).map_err(|e| format!("parsing the PDF: {e}"))?;
    let mut out = Vec::new();
    for (page, info) in parsed.pages.iter().enumerate() {
        let extracted = analyze_page_text(&parsed.file, page, info)
            .map_err(|e| format!("extracting page {page}: {e}"))?;
        for glyph in extracted.glyphs {
            let xs = glyph.quad.points.map(|p| p.x);
            let ys = glyph.quad.points.map(|p| p.y);
            out.push(json!([
                page,
                glyph.text.to_string(),
                xs.iter().copied().fold(f64::INFINITY, f64::min),
                ys.iter().copied().fold(f64::INFINITY, f64::min),
                xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            ]));
        }
    }
    Ok(out)
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, corpus, out] = args.as_slice() else {
        panic!("usage: glyph_dump <folder> <out.jsonl>");
    };
    let mut files = Vec::new();
    pdfs_under(&Path::new(corpus).canonicalize()?, &mut files)?;
    files.sort();
    let mut lines = String::new();
    for file in &files {
        let bytes = std::fs::read(file)?;
        let started = Instant::now();
        let extracted =
            std::panic::catch_unwind(|| glyphs(&bytes)).unwrap_or_else(|_| Err("panic".to_owned()));
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let line = match extracted {
            Ok(glyphs) => json!({"file": file, "ms": ms, "glyphs": glyphs}),
            Err(error) => json!({"file": file, "ms": ms, "error": error}),
        };
        lines.push_str(&line.to_string());
        lines.push('\n');
    }
    std::fs::write(out, lines)?;
    println!("{} files -> {out}", files.len());
    Ok(())
}

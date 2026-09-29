//! One JSON line per PDF, a digest of each page's text, items and glyphs: two
//! runs around a change to the interpreter must write the same lines.
//! cargo run --release -p lj-pdf --example text_identity -- <out.jsonl> <path>...

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use lj_pdf::objects::parse_pdf;
use lj_pdf::text::analyze_page_text;
use serde_json::json;
use sha2::{Digest, Sha256};

const THREADS: usize = 3;

fn pdfs_under(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if root.is_file() {
        out.push(root.to_path_buf());
        return Ok(());
    }
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

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn fingerprint(path: &Path) -> serde_json::Value {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => return json!({ "error": format!("reading: {e}") }),
    };
    let parsed = match parse_pdf(&bytes) {
        Ok(parsed) => parsed,
        Err(e) => return json!({ "error": format!("parsing: {e}") }),
    };
    let pages: Vec<String> = parsed
        .pages
        .iter()
        .enumerate()
        .map(
            |(index, info)| match analyze_page_text(&parsed.file, index, info) {
                Ok(page) => digest(&format!("{page:?}")),
                Err(e) => format!("error: {e}"),
            },
        )
        .collect();
    json!({ "pages": pages })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .expect("usage: text_identity <out.jsonl> <path>...");
    let mut files = Vec::new();
    for root in args {
        pdfs_under(Path::new(&root), &mut files).expect("reading a folder");
    }
    files.sort();
    let lines = Mutex::new(vec![String::new(); files.len()]);
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else {
                        return;
                    };
                    let mut value = std::panic::catch_unwind(|| fingerprint(path))
                        .unwrap_or_else(|_| json!({ "error": "panic" }));
                    value["file"] = json!(path.display().to_string());
                    lines.lock().unwrap()[i] = value.to_string();
                }
            });
        }
    });
    let mut writer = std::io::BufWriter::new(std::fs::File::create(out).expect("creating out"));
    for line in lines.into_inner().unwrap() {
        writeln!(writer, "{line}").expect("writing out");
    }
}

//! `robust-batch`: each file of a list through what the Word or the PDF module
//! runs, on a thread with a wasm stack, announced before it starts so a death
//! names it. Built for `wasm32-wasip1`, it runs on the main thread of a real
//! wasm32 module, where a panic aborts the process after printing itself.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use lj_office::{Fonts, Item};
use lj_redact_core::labels::Needle;
use lj_redact_core::{office, pdf, Anchor, Kind};
use serde::{Deserialize, Serialize};

/// The stack of the main thread of a wasm32 module, which rustc links at 1 MiB.
#[cfg(not(target_arch = "wasm32"))]
const WASM_STACK: usize = 1 << 20;

/// Needles the redaction pass takes from the suggestions, at most.
const NEEDLES: usize = 40;

#[derive(Deserialize)]
struct Job {
    sha256: String,
    format: String,
    source: String,
    bytes: u64,
    path: PathBuf,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum Verdict {
    Ok,
    /// The reader refuses the format for now.
    Unsupported,
    Error,
    Panic,
    Crash,
}

#[derive(Serialize)]
struct Record {
    sha256: String,
    format: String,
    source: String,
    bytes: u64,
    verdict: Verdict,
    /// The stage the file stopped in, or the last one it ran.
    stage: String,
    detail: String,
    /// CPU milliseconds per stage.
    ms: BTreeMap<String, f64>,
    seconds: f64,
    cpu: f64,
    /// Peak resident memory while this file ran, in MiB.
    rss_mb: f64,
    /// Resident memory when it started, the fonts already read included.
    base_mb: f64,
    pages: usize,
    characters: usize,
    needles: usize,
    found: usize,
    removed: usize,
    /// Needles read only in an entry no reader walks, a picture or a font
    /// name, or that a PDF still shows, which withhold the file rather than
    /// leak it.
    withheld: usize,
    /// PDF pages the drawing refused, which the viewer leaves blank.
    undrawn: usize,
    /// What went wrong without stopping the file.
    issues: Vec<String>,
    /// The parent's answer: whether LibreOffice opens a file ours refused.
    libreoffice: Option<String>,
}

impl Record {
    fn new(job: &Job) -> Self {
        Self {
            sha256: job.sha256.clone(),
            format: job.format.clone(),
            source: job.source.clone(),
            bytes: job.bytes,
            verdict: Verdict::Ok,
            stage: String::new(),
            detail: String::new(),
            ms: BTreeMap::new(),
            seconds: 0.0,
            cpu: 0.0,
            rss_mb: 0.0,
            base_mb: 0.0,
            pages: 0,
            characters: 0,
            needles: 0,
            found: 0,
            removed: 0,
            withheld: 0,
            undrawn: 0,
            issues: Vec::new(),
            libreoffice: None,
        }
    }

    fn failed(job: &Job, verdict: Verdict, stage: &str, detail: String) -> Self {
        Self {
            verdict,
            stage: stage.to_string(),
            detail,
            ..Self::new(job)
        }
    }
}

fn clip(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

/// CPU seconds of the calling thread, which the load of a shared host does
/// not inflate as it does the wall clock.
#[cfg(not(target_arch = "wasm32"))]
fn thread_cpu() -> f64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid timespec for the call to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut now) };
    now.tv_sec as f64 + now.tv_nsec as f64 / 1e9
}

/// A wasm module has one thread and no CPU clock; the parent reads the CPU of
/// the runtime's process.
#[cfg(target_arch = "wasm32")]
fn thread_cpu() -> f64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn proc_mb(field: &str) -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with(field))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

static STAGE: Mutex<String> = Mutex::new(String::new());
static PANIC: Mutex<Option<String>> = Mutex::new(None);

pub fn run_batch(list: &Path, fonts: &Path) -> Result<()> {
    let jobs: Vec<Job> = std::fs::read_to_string(list)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    std::panic::set_hook(Box::new(|info| {
        #[cfg(target_arch = "wasm32")]
        eprintln!("{info}");
        *PANIC.lock().unwrap_or_else(|e| e.into_inner()) = Some(info.to_string());
    }));
    let mut files = FaceFiles {
        dir: fonts.to_path_buf(),
        loaded: HashMap::new(),
    };
    let stdout = std::io::stdout();
    for (i, job) in jobs.iter().enumerate() {
        {
            let mut out = stdout.lock();
            writeln!(out, "start\t{i}")?;
            out.flush()?;
        }
        // Resets VmHWM, so each file reads its own peak.
        let _ = std::fs::write("/proc/self/clear_refs", "5");
        let base_mb = proc_mb("VmRSS:").unwrap_or(0.0);
        let started = Instant::now();
        let bytes = std::fs::read(&job.path)?;
        #[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut))]
        let mut run = || {
            let cpu = thread_cpu();
            let mut record = check(job, &bytes, &mut files);
            record.cpu = thread_cpu() - cpu;
            record
        };
        #[cfg(not(target_arch = "wasm32"))]
        let joined = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(WASM_STACK)
                .spawn_scoped(scope, run)
                .map(|worker| worker.join())
        });
        #[cfg(target_arch = "wasm32")]
        let joined: std::io::Result<std::thread::Result<Record>> = Ok(Ok(run()));
        let mut record = match joined {
            Ok(Ok(record)) => record,
            Ok(Err(_)) => {
                let message = PANIC
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                    .unwrap_or_default();
                let stage = STAGE.lock().unwrap_or_else(|e| e.into_inner()).clone();
                Record::failed(job, Verdict::Panic, &stage, clip(&message, 600))
            }
            Err(e) => Record::failed(job, Verdict::Crash, "spawn", e.to_string()),
        };
        record.seconds = started.elapsed().as_secs_f64();
        record.rss_mb = proc_mb("VmHWM:").unwrap_or(0.0);
        record.base_mb = base_mb;
        let mut out = stdout.lock();
        writeln!(out, "done\t{}", serde_json::to_string(&record)?)?;
        out.flush()?;
    }
    Ok(())
}

struct FaceFiles {
    dir: PathBuf,
    loaded: HashMap<&'static str, Vec<u8>>,
}

impl FaceFiles {
    fn get(&mut self, file: &'static str) -> Result<Vec<u8>> {
        if !self.loaded.contains_key(file) {
            let bytes = std::fs::read(self.dir.join(file))
                .with_context(|| format!("the vendored face {file}"))?;
            self.loaded.insert(file, bytes);
        }
        Ok(self.loaded[file].clone())
    }
}

/// Names each stage to the parent before it runs, and times it.
fn stage<T>(record: &mut Record, name: &str, work: impl FnOnce() -> Result<T>) -> Result<T> {
    {
        let mut stage = STAGE.lock().unwrap_or_else(|e| e.into_inner());
        stage.clear();
        stage.push_str(name);
    }
    {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "stage\t{name}");
        let _ = out.flush();
    }
    record.stage = name.to_string();
    let started = thread_cpu();
    let result = work();
    *record.ms.entry(name.to_string()).or_default() += (thread_cpu() - started) * 1000.0;
    result
}

fn check(job: &Job, bytes: &[u8], files: &mut FaceFiles) -> Record {
    let mut record = Record::new(job);
    let result = match job.format.starts_with("pdf") {
        true => pdf_stages(bytes, &mut record),
        false => stages(bytes, files, &mut record),
    };
    if let Err(e) = result {
        let detail = format!("{e:#}");
        record.verdict = match detail.contains("not yet supported") {
            true => Verdict::Unsupported,
            false => Verdict::Error,
        };
        record.detail = clip(&detail, 600);
    }
    record
}

/// The two longest capitalised words of six to forty letters, which any text
/// has even when the rules propose nothing.
fn capitalised(characters: &[(char, Anchor)]) -> Vec<String> {
    let text: String = characters.iter().map(|(c, _)| *c).collect();
    let mut words: Vec<&str> = text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| {
            (6..=40).contains(&w.chars().count())
                && w.chars().next().is_some_and(char::is_uppercase)
        })
        .collect();
    words.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    words.dedup();
    words.into_iter().take(2).map(str::to_string).collect()
}

fn svg_issue(page: usize, svg: &str) -> Option<String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(svg);
    let non_finite = |token: &str| matches!(token, "NaN" | "inf" | "-inf");
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e) | Event::Empty(e)) => {
                for attr in e.attributes().flatten() {
                    let value = String::from_utf8_lossy(&attr.value);
                    if value
                        .split(|c: char| c.is_whitespace() || "(),".contains(c))
                        .any(non_finite)
                    {
                        return Some(format!("page {page}: the SVG carries a non-finite number"));
                    }
                }
            }
            Ok(_) => {}
            Err(e) => return Some(format!("page {page}: the SVG is not well-formed: {e}")),
        }
    }
    if svg.contains(['\u{FFFE}', '\u{FFFF}']) {
        return Some(format!("page {page}: the SVG carries a non-character"));
    }
    None
}

fn layout_issue(layout: &lj_office::Layout) -> Option<String> {
    for (i, page) in layout.pages.iter().enumerate() {
        let sized = page.width.is_finite()
            && page.height.is_finite()
            && page.width > 0.0
            && page.height > 0.0;
        let consistent = page.items.iter().all(|item| match item {
            Item::Glyphs(g) => {
                let n = g.text.chars().count();
                g.advances.len() == n && g.anchors.len() == n
            }
            _ => true,
        });
        if !sized || !consistent {
            return Some(format!("page {i}: a page without a size or a run whose advances or anchors miss characters"));
        }
    }
    None
}

fn stages(bytes: &[u8], files: &mut FaceFiles, record: &mut Record) -> Result<()> {
    let model = stage(record, "open", || lj_office::open(bytes))?;
    let characters = stage(record, "characters", || office::characters(bytes))?;
    record.characters = characters.len();
    let views = stage(record, "views", || office::views(bytes))?;
    stage(record, "model-characters", || Ok(model.characters().len()))?;
    let fonts = stage(record, "fonts", || {
        let mut fonts = Fonts::new();
        for file in model.fonts_needed() {
            fonts.add(files.get(file)?)?;
        }
        Ok(fonts)
    })?;
    let layout = stage(record, "layout", || Ok(model.layout(&fonts)))?;
    record.pages = layout.page_count();
    let mut issues: Vec<String> = layout_issue(&layout).into_iter().collect();
    stage(record, "svg", || {
        for page in 0..layout.page_count() {
            issues.extend(svg_issue(page, &layout.svg(page)));
        }
        Ok(())
    })?;
    stage(record, "words", || {
        for page in 0..layout.page_count() {
            layout.words(page);
        }
        Ok(())
    })?;
    stage(record, "blocks", || {
        Ok(lj_redact_core::blocks(&characters).len())
    })?;
    let suggestions = stage(record, "suggest", || {
        office::suggestions(bytes, &characters, &views)
    })?;
    let mut needles: Vec<String> = Vec::new();
    let mut kinds: Vec<Option<Kind>> = Vec::new();
    for (text, kind) in suggestions
        .into_iter()
        .map(|s| (s.text, Some(s.kind)))
        .chain(capitalised(&characters).into_iter().map(|t| (t, None)))
    {
        if needles.len() < NEEDLES && !needles.contains(&text) {
            needles.push(text);
            kinds.push(kind);
        }
    }
    let entries: Vec<Needle> = needles
        .iter()
        .zip(&kinds)
        .map(|(text, kind)| Needle {
            text,
            kind: *kind,
            label: None,
            held: false,
        })
        .collect();
    record.needles = needles.len();
    let occurrences = stage(record, "locate", || {
        Ok(office::locate(&characters, &views, &needles))
    })?;
    stage(record, "quads", || {
        let anchors: Vec<Vec<lj_office::Anchor>> = occurrences
            .iter()
            .map(|o| o.anchors.iter().filter_map(Anchor::office).collect())
            .collect();
        let groups: Vec<&[lj_office::Anchor]> = anchors.iter().map(Vec::as_slice).collect();
        Ok(layout.quads(&groups))
    })?;
    if !needles.is_empty() {
        let outcome = stage(record, "redact", || office::redact(bytes, &entries))?;
        record.found = outcome.found;
        record.removed = outcome.removed;
        if let Err(e) = stage(record, "reopen", || lj_office::open(&outcome.bytes)) {
            issues.push(format!("the redacted file does not reopen: {e:#}"));
        }
        if !outcome.leaks.is_empty() {
            let shown = stage(record, "leaks", || readable(&outcome.bytes, &needles))?;
            if shown.is_empty() {
                record.withheld = outcome.leaks.len();
            } else {
                issues.push(format!(
                    "{} of {} needles survive in the text: {:?}",
                    shown.len(),
                    needles.len(),
                    shown.iter().take(3).collect::<Vec<_>>()
                ));
            }
        }
    }
    record.issues = issues;
    Ok(())
}

/// What the PDF module runs: the characters, every page drawn, its lines of
/// words, the suggestions, a search, and a redaction by glyph and by rectangle.
fn pdf_stages(bytes: &[u8], record: &mut Record) -> Result<()> {
    let characters = stage(record, "characters", || pdf::glyphs(bytes))?;
    record.characters = characters.len();
    let document = stage(record, "open", || {
        lj_pdf::PdfDocument::open(bytes).map_err(|e| anyhow!("parsing the PDF: {e}"))
    })?;
    let pages = document.page_count();
    record.pages = pages;
    let mut issues: Vec<String> = Vec::new();
    let mut undrawn = 0;
    stage(record, "svg", || {
        for page in 0..pages {
            let size = document
                .page_size(page)
                .map_err(|e| anyhow!("the size of page {page}: {e}"))?;
            if !(size.width > 0.0 && size.height > 0.0) {
                undrawn += 1;
                continue;
            }
            match document.draw_page(page) {
                Ok(drawn) => issues.extend(svg_issue(page, &lj_draw::svg(&drawn, ""))),
                Err(_) => undrawn += 1,
            }
        }
        Ok(())
    })?;
    record.undrawn = undrawn;
    stage(record, "words", || {
        for page in 0..pages {
            lj_redact_core::layout::page_lines(&characters, page);
        }
        Ok(())
    })?;
    stage(record, "blocks", || {
        Ok(lj_redact_core::blocks(&characters).len())
    })?;
    let suggestions = stage(record, "suggest", || {
        Ok(lj_redact_core::suggest(&characters))
    })?;
    let mut needles: Vec<String> = Vec::new();
    for text in suggestions
        .into_iter()
        .map(|s| s.text)
        .chain(capitalised(&characters))
    {
        if needles.len() < NEEDLES && !needles.contains(&text) {
            needles.push(text);
        }
    }
    record.needles = needles.len();
    stage(record, "locate", || {
        Ok(lj_redact_core::locate(&characters, &needles))
    })?;
    let rects: Vec<pdf::PageRect> = match document.page_size(0) {
        Ok(size) if size.width > 0.0 && size.height > 0.0 => vec![pdf::PageRect {
            page: 0,
            x: size.width / 4.0,
            y: size.height / 4.0,
            width: size.width / 2.0,
            height: size.height / 8.0,
        }],
        _ => Vec::new(),
    };
    if !needles.is_empty() || !rects.is_empty() {
        let outcome = stage(record, "redact", || pdf::redact(bytes, &needles, &rects))?;
        record.found = outcome.found;
        record.removed = outcome.removed;
        record.withheld = outcome.leaks.len();
        if let Err(e) = stage(record, "reopen", || {
            lj_pdf::PdfDocument::open(&outcome.bytes).map_err(|e| anyhow!("{e}"))
        }) {
            issues.push(format!("the redacted file does not reopen: {e:#}"));
        }
    }
    record.issues = issues;
    Ok(())
}

/// Needles still readable in `bytes`, in any of its views, where the
/// redaction should have removed them.
fn readable(bytes: &[u8], needles: &[String]) -> Result<Vec<String>> {
    let found: std::collections::BTreeSet<usize> =
        office::locate(&office::characters(bytes)?, &office::views(bytes)?, needles)
            .into_iter()
            .map(|o| o.needle)
            .collect();
    Ok(found.into_iter().map(|n| needles[n].clone()).collect())
}

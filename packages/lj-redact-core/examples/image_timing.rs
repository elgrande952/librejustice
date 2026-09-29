//! `image_timing REPEAT FILE...` times the image work of the redaction module,
//! natively or under wasmtime, and prints the median of REPEAT runs per file
//! with a hash of what came out, so that two builds compare on speed and on
//! bytes. A `.pgm` page is prepared for the reader; a PDF has its first five
//! pages drawn and written as SVG, then three rectangles blacked out on its
//! first page.

use std::time::Instant;

use lj_redact_core::pdf::{redact, PageRect};
use lj_redact_core::scan::prepare;

fn read_pgm(bytes: &[u8]) -> (Vec<u8>, u32, u32) {
    let mut fields = Vec::new();
    let mut at = 0;
    while fields.len() < 4 {
        while bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let start = at;
        while !bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..at]).into_owned());
    }
    assert_eq!(fields[0], "P5");
    let rgba = bytes[at + 1..]
        .iter()
        .flat_map(|v| [*v, *v, *v, 255])
        .collect();
    (rgba, fields[1].parse().unwrap(), fields[2].parse().unwrap())
}

fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

/// The median milliseconds of `repeat` calls, and what the last one returned.
fn timed<T>(repeat: usize, mut run: impl FnMut() -> T) -> (f64, T) {
    let mut times = Vec::with_capacity(repeat);
    let mut out = None;
    for _ in 0..repeat {
        let started = Instant::now();
        out = Some(run());
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    (times[repeat / 2], out.unwrap())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let repeat: usize = args[1].parse().unwrap();
    for path in &args[2..] {
        let bytes = std::fs::read(path).unwrap();
        let name = path.rsplit('/').next().unwrap();
        if path.ends_with(".pgm") {
            let (rgba, w, h) = read_pgm(&bytes);
            let (ms, page) = timed(repeat, || prepare(&rgba, w, h));
            let corner = page.source_box(10.0, 20.0, 30.0, 40.0);
            let digest = hash(&page.grey) ^ hash(format!("{corner:?}").as_bytes());
            println!("{name:28} prepare {ms:8.1}  #{digest:016x}");
            continue;
        }
        let document = lj_pdf::PdfDocument::open(&bytes).unwrap();
        let pages = document.page_count().min(5);
        let (draw, drawn) = timed(repeat, || {
            (0..pages)
                .map(|index| document.draw_page(index).unwrap())
                .collect::<Vec<_>>()
        });
        let (svg, svgs) = timed(repeat, || {
            drawn
                .iter()
                .map(|page| lj_draw::svg(page, "p"))
                .collect::<String>()
        });
        let rects: Vec<PageRect> = (0..3)
            .map(|k| PageRect {
                page: 0,
                x: 60.0,
                y: 100.0 + 150.0 * f64::from(k),
                width: 200.0,
                height: 20.0,
            })
            .collect();
        let (apply, produced) = timed(repeat, || redact(&bytes, &[], &rects).unwrap());
        println!(
            "{name:28} draw {draw:8.1} svg {svg:7.1} redact {apply:8.1}  svg #{:016x} pdf #{:016x}",
            hash(svgs.as_bytes()),
            hash(&produced.bytes)
        );
    }
}

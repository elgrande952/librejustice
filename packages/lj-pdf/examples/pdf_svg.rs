//! Each page of a PDF drawn by lj-pdf, for the drawing bench:
//! `pdf_svg <in.pdf> <outdir> [--first N] [--png <dpi>] [--fonts <dir>]` writes
//! `<n>.svg` for each page n from 1, and `<n>.png` rasterised by resvg with
//! `--png`, prints one JSON line, and exits 2 with a message on stderr when the
//! file is refused. A page that cannot be drawn is left blank and named on
//! stderr.

use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use lj_pdf::PdfDocument;
use resvg::{tiny_skia, usvg};
use serde_json::json;

const USAGE: &str = "usage: pdf_svg <in.pdf> <outdir> [--first N] [--png <dpi>] [--fonts <dir>]";

/// The long side of a raster, as the bench's reference fits it: a poster page
/// at 72 DPI would weigh hundreds of megapixels.
const MAX_PX: f64 = 3000.0;

struct Args {
    input: PathBuf,
    out: PathBuf,
    first: usize,
    png: Option<f64>,
    fonts: Option<PathBuf>,
}

fn args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(out)) = (args.next(), args.next()) else {
        return Err(USAGE.to_string());
    };
    let mut parsed = Args {
        input: input.into(),
        out: out.into(),
        first: usize::MAX,
        png: None,
        fonts: None,
    };
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{flag} takes a value\n{USAGE}"))?;
        match flag.as_str() {
            "--first" => {
                parsed.first = value
                    .parse()
                    .map_err(|_| format!("--first {value}: a page count\n{USAGE}"))?
            }
            "--png" => match value.parse::<f64>() {
                Ok(dpi) if dpi > 0.0 => parsed.png = Some(dpi),
                _ => return Err(format!("--png {value}: a positive resolution\n{USAGE}")),
            },
            "--fonts" => parsed.fonts = Some(value.into()),
            _ => return Err(format!("unknown argument {flag}\n{USAGE}")),
        }
    }
    Ok(parsed)
}

fn main() -> ExitCode {
    let args = match args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let started = Instant::now();
    let bytes = match std::fs::read(&args.input) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("reading {}: {e}", args.input.display());
            return ExitCode::FAILURE;
        }
    };
    let document = match PdfDocument::open(&bytes) {
        Ok(document) => document,
        Err(e) => {
            eprintln!("refused: {e}");
            return ExitCode::from(2);
        }
    };
    let options = match args.fonts.as_deref().map(options).transpose() {
        Ok(options) => options.unwrap_or_default(),
        Err(e) => {
            eprintln!("reading fonts: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&args.out) {
        eprintln!("creating {}: {e}", args.out.display());
        return ExitCode::FAILURE;
    }
    let total = document.page_count();
    let count = total.min(args.first);
    let mut page_ms = Vec::with_capacity(count);
    let mut svg_bytes = Vec::with_capacity(count);
    for index in 0..count {
        let page_started = Instant::now();
        let size = document.page_size(index).expect("a page of the document");
        let drawn = match size.width > 0.0 && size.height > 0.0 {
            true => document.draw_page(index).map_err(|e| e.to_string()),
            false => Err("its box is empty".to_string()),
        };
        let svg = match drawn {
            Ok(page) => lj_draw::svg(&page, ""),
            Err(e) => {
                eprintln!("cannot draw page {}: {e}", index + 1);
                lj_draw::svg(
                    &lj_draw::Page {
                        width: size.width.max(1.0),
                        height: size.height.max(1.0),
                        ..lj_draw::Page::default()
                    },
                    "",
                )
            }
        };
        let written = write_page(&args.out, index, &svg, args.png, &options);
        if let Err(e) = written {
            eprintln!("writing page {}: {e}", index + 1);
            return ExitCode::FAILURE;
        }
        svg_bytes.push(svg.len());
        page_ms.push(round(page_started.elapsed().as_secs_f64() * 1000.0));
    }
    println!(
        "{}",
        json!({
            "pages": count,
            "total_pages": total,
            "ms": round(started.elapsed().as_secs_f64() * 1000.0),
            "page_ms": page_ms,
            "svg_bytes": svg_bytes,
        })
    );
    ExitCode::SUCCESS
}

fn round(ms: f64) -> f64 {
    (ms * 10.0).round() / 10.0
}

fn write_page(
    out: &Path,
    index: usize,
    svg: &str,
    png: Option<f64>,
    options: &usvg::Options,
) -> Result<(), String> {
    std::fs::write(out.join(format!("{}.svg", index + 1)), svg).map_err(|e| e.to_string())?;
    let Some(dpi) = png else {
        return Ok(());
    };
    let tree = usvg::Tree::from_str(svg, options).map_err(|e| e.to_string())?;
    let pixmap = rasterise(&tree, dpi)?;
    write_png(&pixmap, &out.join(format!("{}.png", index + 1))).map_err(|e| e.to_string())
}

/// The SVG names each face by its PostScript name alone.
fn options(fonts: &Path) -> std::io::Result<usvg::Options<'static>> {
    let mut options = usvg::Options::default();
    let db = options.fontdb_mut();
    for entry in std::fs::read_dir(fonts)? {
        let path = entry?.path();
        let font = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"));
        if font {
            db.load_font_data(std::fs::read(path)?);
        }
    }
    let faces: Vec<usvg::fontdb::FaceInfo> = db.faces().cloned().collect();
    for mut face in faces {
        db.remove_face(face.id);
        face.families = vec![(
            face.post_script_name.clone(),
            usvg::fontdb::Language::English_UnitedStates,
        )];
        db.push_face_info(face);
    }
    Ok(options)
}

fn rasterise(tree: &usvg::Tree, dpi: f64) -> Result<tiny_skia::Pixmap, String> {
    let size = tree.size();
    let (width, height) = (size.width() as f64, size.height() as f64);
    let mut scale = dpi / 72.0;
    let long = width.max(height) * scale;
    if long > MAX_PX {
        scale *= MAX_PX / long;
    }
    let (w, h) = (
        ((width * scale).round() as u32).max(1),
        ((height * scale).round() as u32).max(1),
    );
    let mut pixmap =
        tiny_skia::Pixmap::new(w, h).ok_or_else(|| format!("a {w}x{h} page has no pixmap"))?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let transform =
        tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(tree, transform, &mut pixmap.as_mut());
    Ok(pixmap)
}

fn write_png(pixmap: &tiny_skia::Pixmap, path: &Path) -> Result<(), png::EncodingError> {
    let data: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue()]
        })
        .collect();
    let file = BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, pixmap.width(), pixmap.height());
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&data)
}

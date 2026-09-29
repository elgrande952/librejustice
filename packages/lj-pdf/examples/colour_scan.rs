//! Counts, for each PDF of a list, the colour spaces, functions, shadings,
//! patterns, soft masks, blend modes and transparency groups its objects
//! declare: one tab-separated line per file that declares any.
//!
//! cargo run --release -p lj-pdf --example colour_scan -- <list.txt> <root>

use std::collections::BTreeMap;

use lj_pdf::objects::{PdfObject, PdfValue, parse_pdf};

fn walk(value: &PdfValue, tags: &mut BTreeMap<String, usize>) {
    match value {
        PdfValue::Array(items) => {
            if let Some(PdfValue::Name(name)) = items.first()
                && matches!(
                    name.as_str(),
                    "ICCBased"
                        | "Indexed"
                        | "Separation"
                        | "DeviceN"
                        | "Lab"
                        | "CalRGB"
                        | "CalGray"
                )
                && items.len() >= 2
            {
                *tags.entry(format!("cs:{name}")).or_default() += 1;
            }
            for item in items {
                walk(item, tags);
            }
        }
        PdfValue::Dictionary(dict) => dictionary(dict, tags),
        _ => {}
    }
}

fn dictionary(dict: &lj_pdf::objects::PdfDictionary, tags: &mut BTreeMap<String, usize>) {
    if let Some(n) = dict.get("FunctionType").and_then(PdfValue::as_integer) {
        *tags.entry(format!("fn:{n}")).or_default() += 1;
    }
    if let Some(n) = dict.get("ShadingType").and_then(PdfValue::as_integer) {
        *tags.entry(format!("sh:{n}")).or_default() += 1;
    }
    if let Some(n) = dict.get("PatternType").and_then(PdfValue::as_integer) {
        let paint = dict
            .get("PaintType")
            .and_then(PdfValue::as_integer)
            .unwrap_or(0);
        *tags.entry(format!("pat:{n}.{paint}")).or_default() += 1;
    }
    if let Some(mask) = dict.get("SMask") {
        match mask {
            PdfValue::Dictionary(m) => {
                let s = m.get("S").and_then(PdfValue::as_name).unwrap_or("?");
                *tags.entry(format!("smask:{s}")).or_default() += 1;
                if m.contains_key("TR") {
                    *tags.entry("smask:TR".into()).or_default() += 1;
                }
                if m.contains_key("BC") {
                    *tags.entry("smask:BC".into()).or_default() += 1;
                }
            }
            PdfValue::Reference(_) => {
                *tags.entry("smask:ref".into()).or_default() += 1;
            }
            _ => {}
        }
    }
    if let Some(bm) = dict.get("BM") {
        let name = match bm {
            PdfValue::Name(n) => n.clone(),
            PdfValue::Array(a) => a.first().and_then(PdfValue::as_name).unwrap_or("?").into(),
            _ => "?".into(),
        };
        if name != "Normal" && name != "Compatible" {
            *tags.entry(format!("bm:{name}")).or_default() += 1;
        }
    }
    for key in ["CA", "ca"] {
        if let Some(v) = dict.get(key).and_then(PdfValue::as_number)
            && v < 1.0
        {
            *tags.entry(format!("alpha:{key}")).or_default() += 1;
        }
    }
    if dict.get("S").and_then(PdfValue::as_name) == Some("Transparency") {
        *tags.entry("group".into()).or_default() += 1;
        if dict.get("I").and_then(PdfValue::as_bool) == Some(true) {
            *tags.entry("group:I".into()).or_default() += 1;
        }
        if dict.get("K").and_then(PdfValue::as_bool) == Some(true) {
            *tags.entry("group:K".into()).or_default() += 1;
        }
    }
    for value in dict.values() {
        walk(value, tags);
    }
}

fn main() {
    let list = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let root = std::env::args().nth(2).unwrap();
    for path in list.lines() {
        let Ok(bytes) = std::fs::read(format!("{root}/{path}")) else {
            continue;
        };
        let Ok(Ok(parsed)) = std::panic::catch_unwind(|| parse_pdf(&bytes)) else {
            continue;
        };
        let mut tags = BTreeMap::new();
        for object in parsed.file.objects.values() {
            match object {
                PdfObject::Value(value) => walk(value, &mut tags),
                PdfObject::Stream(stream) => dictionary(&stream.dict, &mut tags),
            }
        }
        if !tags.is_empty() {
            let tags: Vec<String> = tags.iter().map(|(k, v)| format!("{k}={v}")).collect();
            println!("{path}\t{}\t{}", parsed.pages.len(), tags.join(" "));
        }
    }
}

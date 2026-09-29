//! `prepare_scan prepare LIST` prepares each `in.pgm out.pgm` pair of LIST and
//! prints the milliseconds it took; `prepare_scan boxes IN.pgm` maps the
//! `[x0, y0, x1, y1]` boxes read on stdin, found on the prepared image, back
//! onto IN.

use std::io::Read;
use std::time::Instant;

use lj_redact_core::scan::{prepare, Prepared};

fn read_pgm(path: &str) -> (Vec<u8>, u32, u32) {
    let bytes = std::fs::read(path).unwrap();
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
    let (w, h): (u32, u32) = (fields[1].parse().unwrap(), fields[2].parse().unwrap());
    let rgba = bytes[at + 1..]
        .iter()
        .flat_map(|v| [*v, *v, *v, 255])
        .collect();
    (rgba, w, h)
}

fn prepared(path: &str) -> Prepared {
    let (rgba, w, h) = read_pgm(path);
    prepare(&rgba, w, h)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args[1].as_str() {
        "prepare" => {
            for line in std::fs::read_to_string(&args[2]).unwrap().lines() {
                let (source, target) = line.split_once(' ').unwrap();
                let (rgba, w, h) = read_pgm(source);
                let started = Instant::now();
                let out = prepare(&rgba, w, h);
                let ms = started.elapsed().as_secs_f64() * 1000.0;
                let mut file = format!("P5\n{} {}\n255\n", out.width, out.height).into_bytes();
                file.extend(&out.grey);
                std::fs::write(target, file).unwrap();
                println!("{ms:.1}");
            }
        }
        "boxes" => {
            let out = prepared(&args[2]);
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input).unwrap();
            let boxes: Vec<[f64; 4]> = serde_json::from_str(&input).unwrap();
            let mapped: Vec<[f64; 4]> = boxes
                .iter()
                .map(|b| {
                    let [x, y, w, h] = out.source_box(b[0], b[1], b[2] - b[0], b[3] - b[1]);
                    [x, y, x + w, y + h]
                })
                .collect();
            println!("{}", serde_json::to_string(&mapped).unwrap());
        }
        other => panic!("unknown command {other}"),
    }
}

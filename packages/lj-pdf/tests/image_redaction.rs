//! A rectangle over part of an image paints the samples it covers and keeps
//! the rest of the image, whatever its codec, bit depth or colour space.

use std::io::Write as _;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use lj_pdf::content::parse_content_stream;
use lj_pdf::objects::{
    ParsedDocument, PdfDictionary, PdfObject, PdfStream, PdfValue, decode_stream, parse_pdf,
};
use lj_pdf::redact::ApplyReport;
use lj_pdf::{PdfDocument, RedactionPlan, RedactionTarget};

const W: u32 = 64;
const H: u32 = 48;

/// The black pixels of `checker-64x48.g4` as MuPDF draws it: 4-pixel
/// squares, inverted in the bottom-right corner.
fn black(x: u32, y: u32) -> bool {
    ((x / 4 + y / 4) % 2 == 1) != (x >= 40 && y >= 24)
}

/// The grey of `checker-64x48.j2k`.
fn grey(x: u32, y: u32) -> u16 {
    match (x / 8 + y / 8) % 2 {
        1 => u16::from((x * 4) as u8),
        _ => 255 - u16::from((y * 5) as u8),
    }
}

/// The pixels the target of [`plan`] covers.
fn covered(x: u32, y: u32) -> bool {
    (10..30).contains(&x) && (8..20).contains(&y)
}

/// Pixels 10 to 30 across and 8 to 20 down of an image drawn one point a
/// pixel at (100, 100), its edges inside the border pixels.
fn plan(page_index: usize) -> RedactionPlan {
    RedactionPlan {
        targets: vec![RedactionTarget::Rect {
            page_index,
            x: 110.5,
            y: 128.5,
            width: 19.0,
            height: 11.0,
        }],
        mode: None,
        fill_color: None,
        overlay_text: None,
        remove_intersecting_annotations: Some(false),
        strip_metadata: Some(false),
        strip_attachments: Some(false),
        sanitize_hidden_ocgs: None,
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(path).expect("fixture should exist")
}

fn flate(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    [
        format!("<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes(),
        data,
        b"\nendstream",
    ]
    .concat()
}

fn build_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

/// A page drawing `/Im1`, object 5, one point a pixel at (100, 100); the
/// objects of `extra` follow from 6 on.
fn image_page(image: Vec<u8>, extra: Vec<Vec<u8>>) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] \
          /Resources << /XObject << /Im1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        stream("", format!("q {W} 0 0 {H} 100 100 cm /Im1 Do Q").as_bytes()),
        image,
    ];
    objects.extend(extra);
    build_pdf(&objects)
}

fn redact(pdf: &[u8], page_index: usize) -> (ApplyReport, ParsedDocument) {
    let mut document = PdfDocument::open(pdf).expect("fixture should open");
    let report = document
        .apply_redactions(plan(page_index))
        .expect("redaction should succeed");
    let saved = document.save().expect("save");
    (report, parse_pdf(&saved).expect("saved file should reopen"))
}

fn object<'a>(document: &'a ParsedDocument, value: &PdfValue) -> &'a PdfStream {
    let PdfValue::Reference(object_ref) = value else {
        panic!("{value:?} is no reference");
    };
    match document.file.get_object(*object_ref).expect("object") {
        PdfObject::Stream(stream) => stream,
        other => panic!("{other:?} is no stream"),
    }
}

fn xobject<'a>(
    document: &'a ParsedDocument,
    resources: &PdfDictionary,
    name: &str,
) -> &'a PdfStream {
    let map = document
        .file
        .resolve_dict(resources.get("XObject").expect("XObject"))
        .expect("XObject map");
    object(document, map.get(name).expect("named"))
}

fn page_xobject<'a>(document: &'a ParsedDocument, page: usize, name: &str) -> &'a PdfStream {
    xobject(document, &document.pages[page].resources, name)
}

fn integer(dict: &PdfDictionary, key: &str) -> i64 {
    dict.get(key).and_then(PdfValue::as_integer).expect(key)
}

/// The samples of an image stored as Flate by the redaction.
fn samples(stream: &PdfStream) -> Vec<u16> {
    let dict = &stream.dict;
    assert_eq!(
        dict.get("Filter").and_then(PdfValue::as_name),
        Some("FlateDecode"),
        "{dict:?}"
    );
    let (width, height) = (
        integer(dict, "Width") as usize,
        integer(dict, "Height") as usize,
    );
    let bits = integer(dict, "BitsPerComponent") as usize;
    let data = decode_stream(stream).expect("decode");
    unpack(&data, width, height, bits)
}

fn unpack(data: &[u8], width: usize, height: usize, bits: usize) -> Vec<u16> {
    let stride = (width * bits).div_ceil(8);
    assert!(data.len() >= stride * height, "{} bytes", data.len());
    let mut out = Vec::with_capacity(width * height);
    for row in data.chunks_exact(stride).take(height) {
        for x in 0..width {
            let bit = x * bits;
            out.push(match bits {
                8 => u16::from(row[x]),
                _ => u16::from((row[bit / 8] >> (8 - bits - bit % 8)) & ((1 << bits) - 1) as u8),
            });
        }
    }
    out
}

/// Every covered pixel reads `painted`, every other `original`.
fn assert_painted(pixels: &[u16], painted: u16, original: impl Fn(u32, u32) -> u16) {
    assert_eq!(pixels.len(), (W * H) as usize);
    for y in 0..H {
        for x in 0..W {
            let expected = if covered(x, y) {
                painted
            } else {
                original(x, y)
            };
            assert_eq!(pixels[(y * W + x) as usize], expected, "pixel ({x}, {y})");
        }
    }
}

/// DeviceGray at one bit: 0 is black.
fn bilevel(x: u32, y: u32) -> u16 {
    u16::from(!black(x, y))
}

const CCITT: &str = "/Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 64 /Rows 48 >>";

fn ccitt_image(extra: &str) -> Vec<u8> {
    stream(
        &format!("/Type /XObject /Subtype /Image /Width {W} /Height {H} {extra} {CCITT}"),
        &fixture("checker-64x48.g4"),
    )
}

#[test]
fn a_ccitt_scan_keeps_its_page_and_loses_the_covered_pixels() {
    let pdf = image_page(
        ccitt_image("/ColorSpace /DeviceGray /BitsPerComponent 1"),
        vec![],
    );
    let (report, document) = redact(&pdf, 0);
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (1, 0)
    );
    let image = page_xobject(&document, 0, "Im1");
    assert_eq!(integer(&image.dict, "BitsPerComponent"), 1);
    assert_painted(&samples(image), 0, bilevel);
}

#[test]
fn a_ccitt_stencil_mask_paints_the_covered_pixels() {
    let pdf = image_page(ccitt_image("/ImageMask true"), vec![]);
    let (report, document) = redact(&pdf, 0);
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (1, 0)
    );
    let image = page_xobject(&document, 0, "Im1");
    assert_eq!(image.dict.get("ImageMask"), Some(&PdfValue::Bool(true)));
    assert_painted(&samples(image), 0, bilevel);
}

/// An embedded JBIG2 stream: a page and one generic region coded in MMR,
/// which is Group 4.
fn jbig2(g4: &[u8]) -> Vec<u8> {
    let segment = |number: u32, kind: u8, data: &[u8]| {
        [
            &number.to_be_bytes()[..],
            &[kind, 0, 1],
            &(data.len() as u32).to_be_bytes(),
            data,
        ]
        .concat()
    };
    let area = [W.to_be_bytes(), H.to_be_bytes(), [0; 4], [0; 4]].concat();
    let page = [&area[..], &[0, 0, 0]].concat();
    let region = [&area[..], &[0, 1], g4].concat();
    [
        segment(0, 48, &page),
        segment(1, 38, &region),
        segment(2, 49, &[]),
    ]
    .concat()
}

#[test]
fn a_jbig2_scan_keeps_its_page_and_drops_its_globals() {
    let image = stream(
        &format!(
            "/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray \
             /BitsPerComponent 1 /Filter /JBIG2Decode /DecodeParms << /JBIG2Globals 6 0 R >>"
        ),
        &jbig2(&fixture("checker-64x48.g4")),
    );
    let globals = stream("", b"");
    let pdf = image_page(image, vec![globals]);
    let (report, document) = redact(&pdf, 0);
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (1, 0)
    );
    let image = page_xobject(&document, 0, "Im1");
    let parms = image.dict.get("DecodeParms").expect("predictor");
    assert!(!format!("{parms:?}").contains("JBIG2Globals"), "{parms:?}");
    assert_painted(&samples(image), 0, bilevel);
    let codecs = document.file.objects.values().filter(|object| {
        matches!(object, PdfObject::Stream(s) if s.dict.get("Filter").and_then(PdfValue::as_name) == Some("JBIG2Decode"))
    });
    assert_eq!(codecs.count(), 0, "the JBIG2 original stays in the file");
}

#[test]
fn a_jpeg_2000_scan_is_stored_back_in_its_own_grey() {
    let image = stream(
        &format!("/Type /XObject /Subtype /Image /Width {W} /Height {H} /Filter /JPXDecode"),
        &fixture("checker-64x48.j2k"),
    );
    let (report, document) = redact(&image_page(image, vec![]), 0);
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (1, 0)
    );
    let image = page_xobject(&document, 0, "Im1");
    assert_eq!(
        image.dict.get("ColorSpace").and_then(PdfValue::as_name),
        Some("DeviceGray")
    );
    assert_eq!(integer(&image.dict, "BitsPerComponent"), 8);
    assert_painted(&samples(image), 0, grey);
}

/// Rows of `bits` bits a sample, `sample(x, y)` each.
fn pack(bits: u32, sample: impl Fn(u32, u32) -> u16) -> Vec<u8> {
    let stride = (W * bits).div_ceil(8) as usize;
    let mut data = vec![0u8; stride * H as usize];
    for y in 0..H {
        for x in 0..W {
            let bit = (x * bits) as usize;
            let shift = 8 - bits as usize - bit % 8;
            data[y as usize * stride + bit / 8] |= (sample(x, y) as u8) << shift;
        }
    }
    data
}

#[test]
fn a_one_bit_flate_image_under_an_inverted_decode_is_painted_black() {
    let inverted = |x, y| 1 - bilevel(x, y);
    let image = stream(
        &format!(
            "/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray \
             /BitsPerComponent 1 /Decode [1 0] /Filter /FlateDecode"
        ),
        &flate(&pack(1, inverted)),
    );
    let (report, document) = redact(&image_page(image, vec![]), 0);
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (1, 0)
    );
    let image = page_xobject(&document, 0, "Im1");
    assert_painted(&samples(image), 1, inverted);
    assert!(image.dict.contains_key("Decode"));
}

#[test]
fn an_indexed_image_at_two_bits_is_painted_with_its_black_entry() {
    let index = |x: u32, y: u32| ((x + y) % 4) as u16;
    let image = stream(
        &format!(
            "/Type /XObject /Subtype /Image /Width {W} /Height {H} \
             /ColorSpace [/Indexed /DeviceRGB 3 <FFFFFFFF0000000000000FF0>] /BitsPerComponent 2"
        ),
        &pack(2, index),
    );
    let (report, document) = redact(&image_page(image, vec![]), 0);
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (1, 0)
    );
    let image = page_xobject(&document, 0, "Im1");
    assert_painted(&samples(image), 2, index);
}

#[test]
fn a_soft_mask_turns_opaque_under_the_rectangle() {
    let alpha = |x: u32, y: u32| u16::from(if black(x, y) { 0u8 } else { 200 });
    let image = stream(
        &format!(
            "/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray \
             /BitsPerComponent 8 /SMask 6 0 R"
        ),
        &pack(8, |_, _| 90),
    );
    let smask = stream(
        &format!(
            "/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray \
             /BitsPerComponent 8 /Filter /FlateDecode"
        ),
        &flate(&pack(8, alpha)),
    );
    let (report, document) = redact(&image_page(image, vec![smask]), 0);
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (1, 0)
    );
    let image = page_xobject(&document, 0, "Im1");
    assert_painted(&samples(image), 0, |_, _| 90);
    let smask = object(&document, image.dict.get("SMask").expect("SMask"));
    assert_painted(&samples(smask), 255, alpha);
}

#[test]
fn an_inline_scan_is_painted_and_the_other_inline_image_kept() {
    let g4 = fixture("checker-64x48.g4");
    let inline = |y: u32| {
        [
            format!(
                "q {W} 0 0 {H} 100 {y} cm BI /W {W} /H {H} /CS /G /BPC 1 /F /CCF \
                 /DP << /K -1 /Columns 64 /Rows 48 >> ID "
            )
            .as_bytes(),
            &g4,
            b"\nEI Q\n",
        ]
        .concat()
    };
    let content = [inline(100), inline(200)].concat();
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << >> /Contents 4 0 R >>"
            .to_vec(),
        stream("", &content),
    ]);
    let (report, document) = redact(&pdf, 0);
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (1, 0)
    );
    let page = &document.pages[0];
    let bytes: Vec<u8> = page
        .content_refs
        .iter()
        .flat_map(|r| match document.file.get_object(*r).expect("content") {
            PdfObject::Stream(s) => decode_stream(s).expect("decode"),
            other => panic!("{other:?}"),
        })
        .collect();
    let images: Vec<(PdfDictionary, Vec<u8>)> = parse_content_stream(&bytes)
        .expect("parse")
        .operations
        .into_iter()
        .filter(|op| op.operator == "BI")
        .map(|op| match op.operands.as_slice() {
            [PdfValue::Dictionary(dict), PdfValue::String(data)] => (dict.clone(), data.0.clone()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(images.len(), 2);
    let (dict, data) = &images[0];
    let mut expanded = PdfDictionary::new();
    expanded.insert("Filter".into(), dict.get("F").expect("F").clone());
    expanded.insert("DecodeParms".into(), dict.get("DP").expect("DP").clone());
    let decoded = decode_stream(&PdfStream {
        dict: expanded,
        data: data.clone(),
    })
    .expect("decode");
    assert_painted(&unpack(&decoded, W as usize, H as usize, 1), 0, bilevel);
    assert_eq!(
        images[1].1, g4,
        "the image the target misses keeps its bytes"
    );
}

#[test]
fn a_scan_shared_by_two_pages_is_painted_on_the_page_targeted_only() {
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] \
          /Resources << /XObject << /Im1 6 0 R >> >> /Contents 5 0 R >>"
            .to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] \
          /Resources << /XObject << /Im1 6 0 R >> >> /Contents 5 0 R >>"
            .to_vec(),
        stream("", format!("q {W} 0 0 {H} 100 100 cm /Im1 Do Q").as_bytes()),
        ccitt_image("/ColorSpace /DeviceGray /BitsPerComponent 1"),
    ]);
    let (report, document) = redact(&pdf, 0);
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (1, 0)
    );
    assert_painted(&samples(page_xobject(&document, 0, "Im1")), 0, bilevel);
    let untouched = page_xobject(&document, 1, "Im1");
    assert_eq!(
        untouched.dict.get("Filter").and_then(PdfValue::as_name),
        Some("CCITTFaxDecode")
    );
    assert_eq!(untouched.data, fixture("checker-64x48.g4"));
}

#[test]
fn a_scan_inside_a_form_is_painted_in_the_form_copy() {
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] \
          /Resources << /XObject << /Fm1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        stream("", b"/Fm1 Do"),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 64 48] /Matrix [1 0 0 1 100 100] \
             /Resources << /XObject << /Im1 6 0 R >> >>",
            format!("q {W} 0 0 {H} 0 0 cm /Im1 Do Q").as_bytes(),
        ),
        ccitt_image("/ColorSpace /DeviceGray /BitsPerComponent 1"),
    ]);
    let (report, document) = redact(&pdf, 0);
    assert_eq!(report.form_xobjects_rewritten, 1);
    let form = page_xobject(&document, 0, "Fm1");
    let resources = document
        .file
        .resolve_dict(form.dict.get("Resources").expect("Resources"))
        .expect("dict")
        .clone();
    let content = decode_stream(form).expect("form content");
    assert!(
        String::from_utf8_lossy(&content).contains("/Im1 Do"),
        "the form still draws its image"
    );
    assert_painted(&samples(xobject(&document, &resources, "Im1")), 0, bilevel);
}

/// A one bit image stored back as Group 4, decoded as its parameters say.
fn fax_samples(stream: &PdfStream) -> (bool, Vec<u16>) {
    let dict = &stream.dict;
    assert_eq!(
        dict.get("Filter").and_then(PdfValue::as_name),
        Some("CCITTFaxDecode"),
        "{dict:?}"
    );
    let parms = match dict.get("DecodeParms") {
        Some(PdfValue::Dictionary(parms)) => parms,
        other => panic!("{other:?}"),
    };
    assert_eq!(integer(parms, "K"), -1);
    let black_is_1 = parms.get("BlackIs1") == Some(&PdfValue::Bool(true));
    let settings = lj_image::bilevel::CcittSettings {
        columns: integer(parms, "Columns") as u32,
        rows: integer(parms, "Rows") as u32,
        end_of_block: true,
        end_of_line: false,
        rows_are_byte_aligned: false,
        encoding: lj_image::bilevel::EncodingMode::Group4,
        invert_black: black_is_1,
    };
    let rows = lj_image::bilevel::ccitt(&stream.data, settings).expect("decodes");
    assert_eq!(rows.rows_written(), H as usize);
    (black_is_1, unpack(&rows.data, W as usize, H as usize, 1))
}

#[test]
fn a_one_bit_page_is_stored_back_as_group_4_its_majority_coded_white() {
    // White pages: sample 1 under the default Decode, sample 0 under [1 0],
    // which BlackIs1 then carries.
    for (decode, white, painted, black_is_1) in [("", 1, 0, false), ("/Decode [1 0]", 0, 1, true)] {
        let page = |_, _| white;
        let image = stream(
            &format!(
                "/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray \
                 /BitsPerComponent 1 {decode} /Filter /FlateDecode"
            ),
            &flate(&pack(1, page)),
        );
        let (_, document) = redact(&image_page(image, vec![]), 0);
        let (coded, pixels) = fax_samples(page_xobject(&document, 0, "Im1"));
        assert_eq!(coded, black_is_1);
        assert_painted(&pixels, painted, page);
    }
}

/// An RGB JPEG subsampled 4:2:0 with a restart marker every two MCUs.
fn subsampled_jpeg(progressive: bool) -> Vec<u8> {
    let mut rgb = Vec::new();
    for y in 0..H {
        for x in 0..W {
            rgb.extend_from_slice(&[(x * 4) as u8, (y * 5) as u8, ((x ^ y) * 3) as u8]);
        }
    }
    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, 75);
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_2_0);
    encoder.set_restart_interval(2);
    encoder.set_progressive(progressive);
    encoder
        .encode(&rgb, W as u16, H as u16, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    out
}

#[test]
fn a_jpeg_keeps_the_bits_of_every_mcu_the_rectangle_misses_and_blackens_it() {
    masked_in_place(subsampled_jpeg(false));
}

#[test]
fn a_progressive_jpeg_is_masked_in_its_coefficients_and_written_baseline() {
    let original = subsampled_jpeg(true);
    assert!(original.windows(2).any(|m| m == [0xFF, 0xC2]));
    let masked = masked_in_place(original);
    assert!(masked.windows(2).any(|m| m == [0xFF, 0xC0]));
}

/// Redacts `original`, an RGB JPEG of the shape [`subsampled_jpeg`] gives,
/// checks its MCUs and pixels, and returns the JPEG written.
fn masked_in_place(original: Vec<u8>) -> Vec<u8> {
    let image = stream(
        &format!(
            "/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceRGB \
             /BitsPerComponent 8 /Filter /DCTDecode"
        ),
        &original,
    );
    let (report, document) = redact(&image_page(image, vec![]), 0);
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (1, 0)
    );
    let image = page_xobject(&document, 0, "Im1");
    assert_eq!(
        image.dict.get("Filter").and_then(PdfValue::as_name),
        Some("DCTDecode")
    );
    // Covered pixels 10 to 30 and 8 to 20, grown by a chroma sample, meet
    // the MCUs of 16 pixels in columns 0 and 1, rows 0 and 1.
    let (before, after) = (
        lj_image::jpeg::read(&original).unwrap(),
        lj_image::jpeg::read(&image.data).unwrap(),
    );
    for (a, b) in before.components.iter().zip(&after.components) {
        for (index, (block_a, block_b)) in a.blocks.iter().zip(&b.blocks).enumerate() {
            let (mx, my) = (index % a.blocks_wide / a.h, index / a.blocks_wide / a.v);
            match mx < 2 && my < 2 {
                true => assert!(block_b[1..].iter().all(|&c| c == 0)),
                false => assert_eq!(block_a, block_b, "block {index}"),
            }
        }
    }
    let pixels = jpeg_decoder::Decoder::new(image.data.as_slice())
        .decode()
        .unwrap();
    for y in 0..H {
        for x in 0..W {
            let at = ((y * W + x) * 3) as usize;
            if covered(x, y) {
                assert_eq!(pixels[at..at + 3], [0, 0, 0], "({x}, {y})");
            }
        }
    }
    image.data.clone()
}

#[test]
fn a_ycck_jpeg_is_blackened_through_its_adobe_transform() {
    let cmyk: Vec<u8> = (0..W * H)
        .flat_map(|i| [(i % 7 * 30) as u8, 60, (i % 5 * 40) as u8, 20])
        .collect();
    let mut original = Vec::new();
    jpeg_encoder::Encoder::new(&mut original, 80)
        .encode(
            &cmyk,
            W as u16,
            H as u16,
            jpeg_encoder::ColorType::CmykAsYcck,
        )
        .unwrap();
    assert_eq!(lj_image::jpeg::read(&original).unwrap().adobe, Some(2));
    let image = stream(
        &format!(
            "/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceCMYK \
             /BitsPerComponent 8 /Filter /DCTDecode"
        ),
        &original,
    );
    let (_, document) = redact(&image_page(image, vec![]), 0);
    let image = page_xobject(&document, 0, "Im1");
    assert_eq!(
        image.dict.get("Filter").and_then(PdfValue::as_name),
        Some("DCTDecode")
    );
    // The decoder hands CMYK inverted; black ink, K alone, comes out so.
    let pixels = jpeg_decoder::Decoder::new(image.data.as_slice())
        .decode()
        .unwrap();
    for y in 0..H {
        for x in 0..W {
            let at = ((y * W + x) * 4) as usize;
            if covered(x, y) {
                assert_eq!(pixels[at..at + 4], [255, 255, 255, 0], "({x}, {y})");
            }
        }
    }
}

#[test]
fn a_jpeg_whose_zone_does_not_read_back_painted_is_removed() {
    // A DC step of 255 puts the nearest flat block 16 levels off grey 112.
    let mut coarse = [1u16; 64];
    coarse[0] = 255;
    let table = jpeg_encoder::QuantizationTableType::Custom(Box::new(coarse));
    let luma: Vec<u8> = (0..W * H).map(|i| (i * 7 % 251) as u8).collect();
    let mut original = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut original, 90);
    encoder.set_quantization_tables(table.clone(), table);
    encoder
        .encode(&luma, W as u16, H as u16, jpeg_encoder::ColorType::Luma)
        .unwrap();
    assert_eq!(lj_image::jpeg::read(&original).unwrap().quant[0][0], 255);
    let image = stream(
        &format!(
            "/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray \
             /BitsPerComponent 8 /Filter /DCTDecode"
        ),
        &original,
    );
    let mut document = PdfDocument::open(&image_page(image, vec![])).unwrap();
    let report = document
        .apply_redactions(RedactionPlan {
            fill_color: Some(lj_pdf::targets::FillColor {
                r: 112,
                g: 112,
                b: 112,
            }),
            ..plan(0)
        })
        .unwrap();
    assert_eq!(
        (report.image_draws_masked, report.image_draws_removed),
        (0, 1)
    );
}

//! TIFF files written by libtiff, decoded to the pixels they were made from.

fn pixels(tiff: &[u8]) -> (u32, u32, png::ColorType, Vec<u8>) {
    let png = lj_image::tiff::to_png(tiff).expect("decodes");
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut out = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut out).unwrap();
    out.truncate(info.buffer_size());
    (info.width, info.height, info.color_type, out)
}

#[test]
fn an_lzw_strip_past_511_codes_with_horizontal_differencing() {
    let (width, height, colour, data) = pixels(include_bytes!("data/rgb-lzw-predictor.tif"));
    assert_eq!((width, height, colour), (97, 61, png::ColorType::Rgb));
    let expected: Vec<u8> = (0..61u32)
        .flat_map(|y| {
            (0..97u32).flat_map(move |x| {
                [(x * 7 + y * 3) % 256, (x * x + y) % 256, (x ^ y) * 5 % 256].map(|v| v as u8)
            })
        })
        .collect();
    assert_eq!(data, expected);
}

fn checks() -> Vec<u8> {
    (0..47u32)
        .flat_map(|y| (0..83u32).map(move |x| if (x / 5 + y / 3) % 3 != 0 { 255 } else { 0 }))
        .collect()
}

#[test]
fn a_group_4_fax_strip() {
    let (width, height, colour, data) = pixels(include_bytes!("data/bilevel-g4.tif"));
    assert_eq!((width, height, colour), (83, 47, png::ColorType::Grayscale));
    assert_eq!(data, checks());
}

#[test]
fn a_packbits_strip() {
    let (_, _, _, data) = pixels(include_bytes!("data/bilevel-packbits.tif"));
    assert_eq!(data, checks());
}

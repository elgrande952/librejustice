//! An image as a picture the browser decodes: a JPEG as it is stored, any
//! other image decoded and written as a PNG, its masks folded into an alpha
//! channel.

mod decode;

pub(crate) use decode::{dct, encoded};

use std::collections::HashMap;

use lj_draw::Rgb;
use lj_image::png;

use super::color::ColorSpace as Space;
use crate::interpret::ShownImage;
use crate::objects::{PdfDictionary, PdfError, PdfFile, PdfObject, PdfResult, PdfStream, PdfValue};
pub(crate) use decode::Codec;
pub(crate) use decode::Samples;

pub(crate) struct Picture {
    pub mime: &'static str,
    pub data: Vec<u8>,
    pub smooth: bool,
    /// A grey PNG whose luminance is the picture's alpha, for a picture whose
    /// format has none.
    pub alpha: Option<Vec<u8>>,
}

/// An inline image's dictionary with its abbreviated keys spelled out.
pub(crate) fn inline_dict(written: &PdfDictionary) -> PdfDictionary {
    written
        .iter()
        .map(|(key, value)| {
            let key = match key.as_str() {
                "W" => "Width",
                "H" => "Height",
                "BPC" => "BitsPerComponent",
                "CS" => "ColorSpace",
                "F" => "Filter",
                "DP" => "DecodeParms",
                "IM" => "ImageMask",
                "D" => "Decode",
                "I" => "Interpolate",
                other => other,
            };
            (key.to_string(), value.clone())
        })
        .collect()
}

pub(crate) fn is_stencil(file: &PdfFile, dict: &PdfDictionary) -> bool {
    value(file, dict, "ImageMask").and_then(PdfValue::as_bool) == Some(true)
}

/// `fill` paints a stencil mask; without it a stencil draws nothing.
pub(crate) fn picture(
    file: &PdfFile,
    image: &ShownImage<'_>,
    fill: Option<Rgb>,
) -> PdfResult<Option<Picture>> {
    let dict = image.dict;
    let smooth = value(file, dict, "Interpolate").and_then(PdfValue::as_bool) == Some(true);
    if is_stencil(file, dict) {
        let Some(color) = fill else {
            return Ok(None);
        };
        let samples = stencil(file, dict, image.data)?;
        let paints_zero = decode_array(file, dict).first() != Some(&1.0);
        let alphas = match paints_zero {
            true => [255, 0],
            false => [0, 255],
        };
        let colors = [[color.0, color.1, color.2]; 2];
        let data = png::encode(
            samples.width,
            samples.height,
            1,
            png::Kind::Palette(&colors, &alphas),
            &samples.data,
        );
        return Ok(Some(Picture {
            mime: "image/png",
            data,
            smooth,
            alpha: None,
        }));
    }

    let encoded = decode::encoded(file, dict, image.data)?;
    let named = dict
        .get("ColorSpace")
        .map(|space| Space::parse(file, space, Some(image.resources)))
        .transpose()?;
    let smask = stream_of(file, dict.get("SMask"));
    let mask = dict.get("Mask");
    let color_key: Option<Vec<i64>> = mask
        .and_then(|mask| file.resolve(mask).ok())
        .and_then(PdfValue::as_array)
        .map(|items| items.iter().filter_map(PdfValue::as_integer).collect());
    let own_mask = match (smask, stream_of(file, mask)) {
        (Some(smask), _) => Some(soft_mask(file, smask)?),
        (None, Some(mask)) if is_stencil(file, &mask.dict) => Some(explicit(file, mask)?),
        (None, Some(mask)) => Some(soft_mask(file, mask)?),
        (None, None) => None,
    };

    // A JPEG the browser shows as it is, its mask beside it.
    if let Some((Codec::Dct, parms)) = &encoded.codec
        && color_key.is_none()
        && !dict.contains_key("Decode")
        && !parms.contains_key("ColorTransform")
        && let Some(header) = decode::jpeg_header(&encoded.bytes)
        && match &named {
            Some(Space::Gray) | None => header.components == 1,
            Some(Space::Rgb) => header.components == 3,
            _ => false,
        }
    {
        return Ok(Some(Picture {
            mime: "image/jpeg",
            data: encoded.bytes,
            smooth,
            alpha: own_mask.map(|alpha| {
                png::encode(alpha.width, alpha.height, 8, png::Kind::Gray, &alpha.data)
            }),
        }));
    }

    let jpx_alpha = smask.is_none()
        && value(file, dict, "SMaskInData").and_then(PdfValue::as_integer) > Some(0);
    let (samples, space, decode) = colour_samples(file, dict, encoded, named, jpx_alpha)?;

    let mut alpha = own_mask.or_else(|| {
        samples.alpha.as_ref().map(|alpha| Alpha {
            width: samples.width,
            height: samples.height,
            data: alpha.clone(),
        })
    });
    if let Some(ranges) = color_key.filter(|ranges| ranges.len() >= 2 * samples.components) {
        alpha = Some(keyed(&samples, &ranges[..2 * samples.components]));
    }

    if alpha.is_none()
        && decode == space.default_decode(samples.bits)
        && let Some(data) = direct(&space, &samples)
    {
        return Ok(Some(Picture {
            mime: "image/png",
            data,
            smooth,
            alpha: None,
        }));
    }

    let rgb = to_rgb(&space, &decode, &samples);
    let data = match alpha {
        None => png::encode(samples.width, samples.height, 8, png::Kind::Rgb, &rgb),
        Some(alpha) => {
            let width = samples.width.max(alpha.width);
            let height = samples.height.max(alpha.height);
            decode::check_size(width, height)?;
            let rgb = resample(&rgb, 3, samples.width, samples.height, width, height);
            let alpha = resample(&alpha.data, 1, alpha.width, alpha.height, width, height);
            let rgba: Vec<u8> = rgb
                .as_chunks::<3>()
                .0
                .iter()
                .zip(&alpha)
                .flat_map(|(pixel, &a)| [pixel[0], pixel[1], pixel[2], a])
                .collect();
            png::encode(width, height, 8, png::Kind::Rgba, &rgba)
        }
    };
    Ok(Some(Picture {
        mime: "image/png",
        data,
        smooth,
        alpha: None,
    }))
}

/// An image's samples as the draw reads them, for the redaction that paints
/// some and stores them back. `space` is `None` for a stencil mask, whose
/// `decode` then says which sample paints.
pub(crate) struct Decoded {
    pub samples: Samples,
    pub space: Option<Space>,
    pub decode: Vec<f64>,
}

/// With `jpx_alpha`, a JPEG 2000 image keeps the alpha it carries.
pub(crate) fn decoded(
    file: &PdfFile,
    dict: &PdfDictionary,
    data: &[u8],
    resources: Option<&PdfDictionary>,
    jpx_alpha: bool,
) -> PdfResult<Decoded> {
    if is_stencil(file, dict) {
        let samples = stencil(file, dict, data)?;
        let decode = match decode_array(file, dict) {
            array if array.len() == 2 => array,
            _ => vec![0.0, 1.0],
        };
        return Ok(Decoded {
            samples,
            space: None,
            decode,
        });
    }
    let encoded = decode::encoded(file, dict, data)?;
    let named = dict
        .get("ColorSpace")
        .map(|space| Space::parse(file, space, resources))
        .transpose()?;
    let (samples, space, decode) = colour_samples(file, dict, encoded, named, jpx_alpha)?;
    Ok(Decoded {
        samples,
        space: Some(space),
        decode,
    })
}

/// The samples of a colour image, the space they are read in and the Decode
/// array that maps them.
fn colour_samples(
    file: &PdfFile,
    dict: &PdfDictionary,
    encoded: decode::Encoded,
    named: Option<Space>,
    jpx_alpha: bool,
) -> PdfResult<(Samples, Space, Vec<f64>)> {
    let width = dimension(file, dict, "Width");
    let height = dimension(file, dict, "Height");
    let bits = match value(file, dict, "BitsPerComponent").and_then(PdfValue::as_integer) {
        Some(bits @ (1 | 2 | 4 | 8 | 16)) => bits as u8,
        Some(bits) => {
            return Err(PdfError::Corrupt(format!(
                "image of {bits} bits per component"
            )));
        }
        None => 8,
    };
    let samples = match encoded.codec {
        None => {
            let (width, height) = (width?, height?);
            decode::check_size(width, height)?;
            let components = named.as_ref().map_or(1, Space::components);
            decode::raw(encoded.bytes, width, height, bits, components)?
        }
        Some((Codec::Dct, parms)) => decode::dct(&encoded.bytes, &parms)?,
        Some((Codec::Jpx, _)) => decode::jpx(
            &encoded.bytes,
            matches!(named, Some(Space::Indexed { .. })),
            jpx_alpha,
        )?,
        Some((Codec::Jbig2, parms)) => decode::jbig2(file, &encoded.bytes, &parms)?,
        Some((Codec::Ccitt, parms)) => decode::ccitt(&encoded.bytes, &parms, height?)?,
    };
    let (space, decode) = space_and_decode(file, dict, named, samples.components, samples.bits)?;
    Ok((samples, space, decode))
}

/// The space an image of `components` is read in, the one it names when
/// the counts agree, and the Decode array that maps its samples.
pub(crate) fn space_and_decode(
    file: &PdfFile,
    dict: &PdfDictionary,
    named: Option<Space>,
    components: usize,
    bits: u8,
) -> PdfResult<(Space, Vec<f64>)> {
    let space = match named {
        Some(space) if space.components() == components => space,
        _ => match components {
            1 => Space::Gray,
            4 => Space::Cmyk,
            3.. => Space::Rgb,
            n => {
                return Err(PdfError::Unsupported(format!("image of {n} components")));
            }
        },
    };
    let decode = match decode_array(file, dict) {
        array if array.len() == 2 * space.components() => array,
        _ => space.default_decode(bits),
    };
    Ok((space, decode))
}

struct Alpha {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

fn value<'a>(file: &'a PdfFile, dict: &'a PdfDictionary, key: &str) -> Option<&'a PdfValue> {
    dict.get(key).and_then(|v| file.resolve(v).ok())
}

fn dimension(file: &PdfFile, dict: &PdfDictionary, key: &str) -> PdfResult<u32> {
    value(file, dict, key)
        .and_then(PdfValue::as_integer)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| PdfError::Corrupt(format!("image without a {key}")))
}

fn decode_array(file: &PdfFile, dict: &PdfDictionary) -> Vec<f64> {
    value(file, dict, "Decode")
        .and_then(PdfValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| file.resolve(item).ok().and_then(PdfValue::as_number))
                .collect()
        })
        .unwrap_or_default()
}

fn stream_of<'a>(file: &'a PdfFile, value: Option<&PdfValue>) -> Option<&'a PdfStream> {
    match value? {
        PdfValue::Reference(object_ref) => match file.get_object(*object_ref).ok()? {
            PdfObject::Stream(stream) => Some(stream),
            PdfObject::Value(_) => None,
        },
        _ => None,
    }
}

/// One bit per pixel, as a stencil or an explicit mask stores it.
fn stencil(file: &PdfFile, dict: &PdfDictionary, data: &[u8]) -> PdfResult<Samples> {
    let encoded = decode::encoded(file, dict, data)?;
    match encoded.codec {
        None => {
            let width = dimension(file, dict, "Width")?;
            let height = dimension(file, dict, "Height")?;
            decode::check_size(width, height)?;
            decode::raw(encoded.bytes, width, height, 1, 1)
        }
        Some((Codec::Jbig2, parms)) => decode::jbig2(file, &encoded.bytes, &parms),
        Some((Codec::Ccitt, parms)) => {
            decode::ccitt(&encoded.bytes, &parms, dimension(file, dict, "Height")?)
        }
        Some((codec, _)) => Err(PdfError::Unsupported(format!("image mask in {codec:?}"))),
    }
}

/// A soft mask's grey, its Decode applied, as alpha.
fn soft_mask(file: &PdfFile, stream: &PdfStream) -> PdfResult<Alpha> {
    let dict = &stream.dict;
    let encoded = decode::encoded(file, dict, &stream.data)?;
    let bits = value(file, dict, "BitsPerComponent")
        .and_then(PdfValue::as_integer)
        .filter(|bits| matches!(bits, 1 | 2 | 4 | 8 | 16))
        .unwrap_or(8) as u8;
    let samples = match encoded.codec {
        None => {
            let width = dimension(file, dict, "Width")?;
            let height = dimension(file, dict, "Height")?;
            decode::check_size(width, height)?;
            decode::raw(encoded.bytes, width, height, bits, 1)?
        }
        Some((Codec::Dct, parms)) => decode::dct(&encoded.bytes, &parms)?,
        Some((Codec::Jpx, _)) => decode::jpx(&encoded.bytes, false, false)?,
        Some((Codec::Jbig2, parms)) => decode::jbig2(file, &encoded.bytes, &parms)?,
        Some((Codec::Ccitt, parms)) => {
            decode::ccitt(&encoded.bytes, &parms, dimension(file, dict, "Height")?)?
        }
    };
    if samples.components != 1 {
        return Err(PdfError::Corrupt(
            "soft mask of more than one component".into(),
        ));
    }
    let decode = match decode_array(file, dict) {
        array if array.len() == 2 => array,
        _ => vec![0.0, 1.0],
    };
    let grey = to_rgb(&Space::Gray, &decode, &samples);
    Ok(Alpha {
        width: samples.width,
        height: samples.height,
        data: grey
            .as_chunks::<3>()
            .0
            .iter()
            .map(|pixel| pixel[0])
            .collect(),
    })
}

/// An explicit mask: where it is 1, unless its Decode inverts it, the image
/// is not painted.
fn explicit(file: &PdfFile, stream: &PdfStream) -> PdfResult<Alpha> {
    let samples = stencil(file, &stream.dict, &stream.data)?;
    let paints_zero = decode_array(file, &stream.dict).first() != Some(&1.0);
    let data = decode::unpack(&samples)
        .map(|bit| match (bit == 0) == paints_zero {
            true => 255,
            false => 0,
        })
        .collect();
    Ok(Alpha {
        width: samples.width,
        height: samples.height,
        data,
    })
}

/// A colour key mask: a pixel whose every sample falls in its range is not
/// painted.
fn keyed(samples: &Samples, ranges: &[i64]) -> Alpha {
    let n = samples.components;
    let values: Vec<u16> = decode::unpack(samples).collect();
    let data = values
        .chunks_exact(n)
        .map(|pixel| {
            let inside = pixel.iter().enumerate().all(|(component, &v)| {
                (ranges[2 * component]..=ranges[2 * component + 1]).contains(&i64::from(v))
            });
            if inside { 0 } else { 255 }
        })
        .collect();
    Alpha {
        width: samples.width,
        height: samples.height,
        data,
    }
}

/// A PNG straight from the samples, when their space is one PNG has.
fn direct(space: &Space, samples: &Samples) -> Option<Vec<u8>> {
    let (width, height, bits) = (samples.width, samples.height, samples.bits);
    match space {
        Space::Gray => Some(png::encode(
            width,
            height,
            bits,
            png::Kind::Gray,
            &samples.data,
        )),
        Space::Rgb if bits >= 8 => Some(png::encode(
            width,
            height,
            bits,
            png::Kind::Rgb,
            &samples.data,
        )),
        Space::Indexed { hival, .. } if bits <= 8 => {
            let count = 1usize << bits;
            let colors: Vec<[u8; 3]> = (0..count)
                .map(|index| space.rgb_bytes(&[index.min(*hival) as f64]))
                .collect();
            let alphas = vec![255; count];
            Some(png::encode(
                width,
                height,
                bits,
                png::Kind::Palette(&colors, &alphas),
                &samples.data,
            ))
        }
        _ => None,
    }
}

/// Eight bit RGB, row after row, no padding.
fn to_rgb(space: &Space, decode: &[f64], samples: &Samples) -> Vec<u8> {
    let n = samples.components;
    let max = f64::from((1u32 << samples.bits) - 1);
    let map = |component: usize, sample: u16| {
        let (low, high) = (decode[2 * component], decode[2 * component + 1]);
        low + f64::from(sample) * (high - low) / max
    };
    if matches!(space, Space::Rgb) && samples.bits == 8 && decode == [0.0, 1.0, 0.0, 1.0, 0.0, 1.0]
    {
        return samples.data.clone();
    }
    let pixels = samples.width as usize * samples.height as usize;
    let mut out = Vec::with_capacity(pixels * 3);
    if n == 1 && samples.bits <= 8 {
        let table: Vec<[u8; 3]> = (0..=max as u16)
            .map(|sample| space.rgb_bytes(&[map(0, sample)]))
            .collect();
        for sample in decode::unpack(samples) {
            out.extend_from_slice(&table[usize::from(sample)]);
        }
        return out;
    }
    // A tint transform runs once per distinct pixel.
    let mut known: HashMap<Vec<u16>, [u8; 3]> = HashMap::new();
    let tinted = matches!(space, Space::Separation { .. });
    let mut raw = Vec::with_capacity(n);
    let mut values = vec![0.0; n];
    for sample in decode::unpack(samples) {
        values[raw.len()] = map(raw.len(), sample);
        raw.push(sample);
        if raw.len() == n {
            let rgb = match tinted {
                true => *known
                    .entry(raw.clone())
                    .or_insert_with(|| space.rgb_bytes(&values)),
                false => space.rgb_bytes(&values),
            };
            out.extend_from_slice(&rgb);
            raw.clear();
        }
    }
    out
}

/// Nearest neighbour, `channels` bytes a pixel.
fn resample(
    data: &[u8],
    channels: usize,
    from_w: u32,
    from_h: u32,
    to_w: u32,
    to_h: u32,
) -> Vec<u8> {
    if (from_w, from_h) == (to_w, to_h) {
        return data.to_vec();
    }
    // A side can reach 2^26 pixels, whose square overflows a 32-bit usize.
    let nearest =
        |at: u32, from: u32, to: u32| (u64::from(at) * u64::from(from) / u64::from(to)) as usize;
    let columns: Vec<usize> = (0..to_w).map(|x| nearest(x, from_w, to_w)).collect();
    let mut out = Vec::with_capacity(to_w as usize * to_h as usize * channels);
    for y in 0..to_h {
        let row = &data[nearest(y, from_h, to_h) * from_w as usize * channels..];
        for &x in &columns {
            out.extend_from_slice(&row[x * channels..(x + 1) * channels]);
        }
    }
    out
}

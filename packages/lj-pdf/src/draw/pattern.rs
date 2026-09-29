//! A pattern a fill or a stroke paints with.

use std::rc::Rc;

use lj_draw::Rgb;

use super::color::ColorSpace;
use super::function::{corrupt, number, numbers};
use super::shading::Shading;
use crate::graphics::Matrix;
use crate::objects::{ObjectRef, PdfFile, PdfObject, PdfResult, PdfValue};

#[derive(Debug)]
pub(crate) enum Pattern {
    Shading {
        shading: Rc<Shading>,
        /// The pattern's space to the default space of the content it is
        /// painted in.
        matrix: Matrix,
    },
    Tiling {
        /// The stream of the cell's content.
        source: ObjectRef,
        /// `[x0, y0, x1, y1]` the cell is clipped to, in the pattern's space.
        bbox: [f64; 4],
        /// The distance between two cells, across then up.
        step: [f64; 2],
        matrix: Matrix,
        /// An uncoloured cell paints in the colour `scn` gives with the
        /// pattern.
        coloured: bool,
    },
}

impl Pattern {
    pub fn parse(
        file: &PdfFile,
        value: &PdfValue,
        shading: impl FnOnce(&PdfValue) -> PdfResult<Rc<Shading>>,
    ) -> PdfResult<Pattern> {
        let dict = match value {
            PdfValue::Reference(object_ref) => match file.get_object(*object_ref)? {
                PdfObject::Stream(stream) => &stream.dict,
                PdfObject::Value(value) => file.resolve_dict(value)?,
            },
            value => file.resolve_dict(value)?,
        };
        let matrix = numbers(file, dict.get("Matrix"))
            .filter(|m| m.len() == 6)
            .map_or_else(Matrix::identity, |m| Matrix {
                a: m[0],
                b: m[1],
                c: m[2],
                d: m[3],
                e: m[4],
                f: m[5],
            });
        match dict
            .get("PatternType")
            .and_then(|v| number(file, v))
            .map(|n| n as i64)
        {
            Some(1) => {
                let PdfValue::Reference(source) = value else {
                    return Err(corrupt("tiling pattern is not a stream"));
                };
                let bbox = numbers(file, dict.get("BBox"))
                    .filter(|b| b.len() == 4)
                    .ok_or_else(|| corrupt("tiling pattern has no BBox"))?;
                let step = ["XStep", "YStep"].map(|key| {
                    dict.get(key)
                        .and_then(|v| number(file, v))
                        .map_or(0.0, f64::abs)
                });
                if step.iter().any(|s| *s < 1e-9 || !s.is_finite()) {
                    return Err(corrupt("tiling pattern step"));
                }
                Ok(Pattern::Tiling {
                    source: *source,
                    bbox: [
                        bbox[0].min(bbox[2]),
                        bbox[1].min(bbox[3]),
                        bbox[0].max(bbox[2]),
                        bbox[1].max(bbox[3]),
                    ],
                    step,
                    matrix,
                    coloured: dict.get("PaintType").and_then(|v| number(file, v)) != Some(2.0),
                })
            }
            Some(2) => Ok(Pattern::Shading {
                shading: shading(
                    dict.get("Shading")
                        .ok_or_else(|| corrupt("shading pattern has no Shading"))?,
                )?,
                matrix,
            }),
            _ => Err(corrupt("pattern type")),
        }
    }

    /// The flat colour that stands for the pattern; an uncoloured tiling one
    /// takes `components` in `under`.
    pub fn representative(&self, under: Option<&ColorSpace>, components: &[f64]) -> Option<Rgb> {
        match self {
            Pattern::Shading { shading, .. } => Some(shading.representative()),
            Pattern::Tiling {
                coloured: false, ..
            } => under.map(|space| space.rgb(components)),
            Pattern::Tiling { coloured: true, .. } => None,
        }
    }
}

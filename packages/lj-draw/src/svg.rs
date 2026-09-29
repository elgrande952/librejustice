use std::fmt::Write as _;

use crate::{
    Blend, Cap, Clip, Fill, Gradient, GradientKind, Group, Image, Item, Join, Matrix, Page, Paint,
    Recolor, Rgb, Segment, Shape, Stroke, Text,
};

/// Slant of a synthetic italic, tan(12 degrees).
const SLANT: f64 = 0.212_556_56;

fn fixed(value: f64, decimals: usize) -> String {
    let scale = 10f64.powi(decimals as i32);
    let rounded = (value * scale).round() / scale;
    let mut s = format!("{rounded:.decimals$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

fn num(value: f64) -> String {
    fixed(value, 3)
}

fn hex(color: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.0, color.1, color.2)
}

fn escape(c: char, out: &mut String) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        c if u32::from(c) < 0x20 => out.push(' '),
        '\u{FFFE}' | '\u{FFFF}' => out.push('\u{FFFD}'),
        c => out.push(c),
    }
}

fn base64(data: &[u8], out: &mut String) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = Vec::with_capacity(data.len().div_ceil(3) * 4);
    let (whole, rest) = data.as_chunks::<3>();
    for [a, b, c] in whole {
        let n = (u32::from(*a) << 16) | (u32::from(*b) << 8) | u32::from(*c);
        text.extend([18, 12, 6, 0].map(|shift| ALPHABET[(n >> shift & 63) as usize]));
    }
    if let [a, tail @ ..] = rest {
        let b = tail.first().copied().unwrap_or(0);
        let n = (u32::from(*a) << 16) | (u32::from(b) << 8);
        for k in 0..4 {
            text.push(match k <= rest.len() {
                true => ALPHABET[(n >> (18 - 6 * k) & 63) as usize],
                false => b'=',
            });
        }
    }
    out.push_str(std::str::from_utf8(&text).expect("base64 is ASCII"));
}

/// A glyph matrix scales font units by a hundredth or less: three decimals
/// would move its glyphs.
fn matrix(m: Matrix) -> String {
    format!(
        "matrix({} {} {} {} {} {})",
        fixed(m.a, 6),
        fixed(m.b, 6),
        fixed(m.c, 6),
        fixed(m.d, 6),
        num(m.e),
        num(m.f)
    )
}

fn transform(m: Matrix, out: &mut String) {
    if m != Matrix::IDENTITY {
        let _ = write!(out, r#" transform="{}""#, matrix(m));
    }
}

struct Writer<'a> {
    ids: &'a str,
    clips: usize,
    filters: usize,
    out: String,
}

impl Writer<'_> {
    fn paint(&mut self, name: &str, paint: Paint, opacity: f64) {
        match paint {
            Paint::Color(color) => {
                let _ = write!(self.out, r#" {name}="{}""#, hex(color));
            }
            Paint::Gradient(index) => {
                let _ = write!(self.out, r##" {name}="url(#{}d{index})""##, self.ids);
            }
            Paint::Pattern(index) => {
                let _ = write!(self.out, r##" {name}="url(#{}t{index})""##, self.ids);
            }
        }
        if opacity < 1.0 {
            let _ = write!(self.out, r#" {name}-opacity="{}""#, num(opacity.max(0.0)));
        }
    }

    fn fill(&mut self, fill: Option<&Fill>) {
        match fill {
            None => self.out.push_str(r#" fill="none""#),
            Some(fill) => {
                self.paint("fill", fill.paint, fill.opacity);
                if fill.even_odd {
                    self.out.push_str(r#" fill-rule="evenodd""#);
                }
            }
        }
    }

    fn stroke(&mut self, stroke: Option<&Stroke>) {
        let Some(stroke) = stroke else { return };
        self.paint("stroke", stroke.paint, stroke.opacity);
        let _ = write!(self.out, r#" stroke-width="{}""#, num(stroke.width));
        match stroke.cap {
            Cap::Butt => {}
            Cap::Round => self.out.push_str(r#" stroke-linecap="round""#),
            Cap::Square => self.out.push_str(r#" stroke-linecap="square""#),
        }
        match stroke.join {
            Join::Miter => {}
            Join::Round => self.out.push_str(r#" stroke-linejoin="round""#),
            Join::Bevel => self.out.push_str(r#" stroke-linejoin="bevel""#),
        }
        if stroke.join == Join::Miter && stroke.miter_limit != 4.0 {
            let _ = write!(
                self.out,
                r#" stroke-miterlimit="{}""#,
                num(stroke.miter_limit.max(1.0))
            );
        }
        if stroke.dash.iter().any(|d| *d > 0.0) {
            let dash: Vec<String> = stroke.dash.iter().map(|d| num(d.max(0.0))).collect();
            let _ = write!(self.out, r#" stroke-dasharray="{}""#, dash.join(" "));
            if stroke.dash_offset != 0.0 {
                let _ = write!(
                    self.out,
                    r#" stroke-dashoffset="{}""#,
                    num(stroke.dash_offset)
                );
            }
        }
    }

    fn segments(&mut self, segments: &[Segment]) {
        for (i, segment) in segments.iter().enumerate() {
            if i > 0 {
                self.out.push(' ');
            }
            let _ = match segment {
                Segment::Move([x, y]) => write!(self.out, "M{} {}", num(*x), num(*y)),
                Segment::Line([x, y]) => write!(self.out, "L{} {}", num(*x), num(*y)),
                Segment::Cubic([x1, y1], [x2, y2], [x, y]) => write!(
                    self.out,
                    "C{} {} {} {} {} {}",
                    num(*x1),
                    num(*y1),
                    num(*x2),
                    num(*y2),
                    num(*x),
                    num(*y)
                ),
                Segment::Close => write!(self.out, "Z"),
            };
        }
    }

    /// The element opening `shape`, its attributes left open.
    fn shape(&mut self, shape: &Shape) {
        let _ = match shape {
            Shape::Rect {
                x,
                y,
                width,
                height,
                radius,
            } => {
                let _ = write!(
                    self.out,
                    r#"<rect x="{}" y="{}" width="{}" height="{}""#,
                    num(*x),
                    num(*y),
                    num(*width),
                    num(*height)
                );
                match *radius > 0.0 {
                    true => write!(self.out, r#" rx="{}""#, num(*radius)),
                    false => Ok(()),
                }
            }
            Shape::Ellipse { cx, cy, rx, ry } => write!(
                self.out,
                r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}""#,
                num(*cx),
                num(*cy),
                num(*rx),
                num(*ry)
            ),
            Shape::Line { from, to } => write!(
                self.out,
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}""#,
                num(from[0]),
                num(from[1]),
                num(to[0]),
                num(to[1])
            ),
            Shape::Path(segments) => {
                self.out.push_str(r#"<path d=""#);
                self.segments(segments);
                self.out.push('"');
                Ok(())
            }
        };
    }

    fn text(&mut self, text: &Text) {
        let color = hex(text.color);
        if text.synthetic_italic {
            let _ = write!(
                self.out,
                r#"<g transform="matrix(1 0 {} 1 {} 0)">"#,
                num(-SLANT),
                num(SLANT * text.y)
            );
        }
        // One family per face, with no weight, style nor width to ask for: a
        // browser would synthesise them over a face that already carries them.
        let _ = write!(
            self.out,
            r#"<text y="{}" font-family="{}" font-size="{}" fill="{color}""#,
            num(text.y),
            text.family.replace('"', ""),
            num(text.size)
        );
        if !text.kerning {
            self.out.push_str(r#" font-kerning="none""#);
        }
        if text.letter_spacing != 0.0 {
            let _ = write!(
                self.out,
                r#" letter-spacing="{}""#,
                num(text.letter_spacing)
            );
        }
        if text.synthetic_bold {
            let _ = write!(
                self.out,
                r#" stroke="{color}" stroke-width="{}""#,
                num(text.size / 30.0)
            );
        }
        self.out.push_str(r#" xml:space="preserve">"#);
        let chars: Vec<(char, f64)> = text
            .text
            .chars()
            .zip(text.advances.iter().copied())
            .collect();
        let mut x = text.x;
        let mut open = false;
        let mut after_space = true;
        for (index, &(c, advance)) in chars.iter().enumerate() {
            let space = c.is_whitespace();
            if !space && after_space {
                if open {
                    self.out.push_str("</tspan>");
                }
                let _ = write!(self.out, r#"<tspan x="{}"#, num(x));
                if text.each_char {
                    let mut next = x + advance;
                    let word = chars[index + 1..]
                        .iter()
                        .take_while(|(c, _)| !c.is_whitespace());
                    for &(_, advance) in word {
                        let _ = write!(self.out, " {}", fixed(next, 2));
                        next += advance;
                    }
                }
                self.out.push_str(r#"">"#);
                open = true;
            }
            if open {
                escape(c, &mut self.out);
            }
            after_space = space;
            x += advance;
        }
        if open {
            self.out.push_str("</tspan>");
        }
        self.out.push_str("</text>");
        if text.synthetic_italic {
            self.out.push_str("</g>");
        }
    }

    fn recolor(&mut self, r: &Recolor) -> usize {
        self.filters += 1;
        let _ = write!(
            self.out,
            r#"<filter id="{}f{}" color-interpolation-filters="sRGB">"#,
            self.ids, self.filters
        );
        if let Some([r, g, b]) = r.grey {
            let grey = format!("{} {} {} 0 0", num(r), num(g), num(b));
            let _ = write!(
                self.out,
                r#"<feColorMatrix type="matrix" values="{grey} {grey} {grey} 0 0 0 1 0"/>"#
            );
        }
        let mut transfer = |function: String| {
            self.out.push_str("<feComponentTransfer>");
            for channel in ["R", "G", "B"] {
                let _ = write!(self.out, "<feFunc{channel} {function}/>");
            }
            self.out.push_str("</feComponentTransfer>");
        };
        if r.threshold {
            transfer(r#"type="discrete" tableValues="0 1""#.to_string());
        }
        if r.slope != 1.0 || r.intercept != 0.0 {
            transfer(format!(
                r#"type="linear" slope="{}" intercept="{}""#,
                num(r.slope),
                num(r.intercept)
            ));
        }
        self.out.push_str("</filter>");
        self.filters
    }

    fn image(&mut self, image: &Image) {
        let filter = image.recolor.as_ref().map(|r| self.recolor(r));
        let m = image.transform;
        // An upright picture keeps its rectangle: stretched from a unit square,
        // its scale misses 1 by a rounding, and resvg then resamples a picture
        // drawn pixel for pixel.
        if m.b == 0.0 && m.c == 0.0 && m.a > 0.0 && m.d > 0.0 {
            let _ = write!(
                self.out,
                r#"<image x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="none""#,
                num(m.e),
                num(m.f),
                num(m.a),
                num(m.d)
            );
        } else {
            self.out
                .push_str(r#"<image width="1" height="1" preserveAspectRatio="none""#);
            transform(m, &mut self.out);
        }
        if !image.smooth {
            self.out.push_str(r#" image-rendering="optimizeSpeed""#);
        }
        if let Some(filter) = filter {
            let _ = write!(self.out, r#" filter="url(#{}f{filter})""#, self.ids);
        }
        let _ = write!(self.out, r#" href="data:{};base64,"#, image.mime);
        base64(&image.data, &mut self.out);
        self.out.push_str(r#""/>"#);
    }

    fn clip(&mut self, clip: &Clip) -> usize {
        self.clips += 1;
        let _ = write!(self.out, r#"<clipPath id="{}c{}">"#, self.ids, self.clips);
        self.shape(&clip.shape);
        if clip.even_odd {
            self.out.push_str(r#" clip-rule="evenodd""#);
        }
        self.out.push_str("/></clipPath>");
        self.clips
    }

    fn group(&mut self, group: &Group) {
        let plain = group.transform == Matrix::IDENTITY
            && group.clip.is_none()
            && group.opacity >= 1.0
            && group.blend.is_none()
            && group.mask.is_none()
            && !group.isolated;
        if plain {
            self.items(&group.items);
            return;
        }
        let clip = group.clip.as_ref().map(|clip| self.clip(clip));
        self.out.push_str("<g");
        transform(group.transform, &mut self.out);
        if let Some(clip) = clip {
            let _ = write!(self.out, r#" clip-path="url(#{}c{clip})""#, self.ids);
        }
        if group.opacity < 1.0 {
            let _ = write!(self.out, r#" opacity="{}""#, num(group.opacity.max(0.0)));
        }
        if let Some(mask) = group.mask {
            let _ = write!(self.out, r#" mask="url(#{}m{mask})""#, self.ids);
        }
        match (group.blend, group.isolated) {
            (Some(blend), isolated) => {
                let _ = write!(self.out, r#" style="mix-blend-mode:{}"#, blend_name(blend));
                if isolated {
                    self.out.push_str(";isolation:isolate");
                }
                self.out.push('"');
            }
            (None, true) => self.out.push_str(r#" style="isolation:isolate""#),
            (None, false) => {}
        }
        self.out.push('>');
        self.items(&group.items);
        self.out.push_str("</g>");
    }

    fn items(&mut self, items: &[Item]) {
        for item in items {
            match item {
                Item::Shape {
                    shape,
                    fill,
                    stroke,
                } => {
                    self.shape(shape);
                    if !matches!(shape, Shape::Line { .. }) {
                        self.fill(fill.as_ref());
                    }
                    self.stroke(stroke.as_ref());
                    self.out.push_str("/>");
                }
                Item::Text(text) => self.text(text),
                Item::Glyph {
                    outline,
                    transform: m,
                    fill,
                    stroke,
                } => {
                    let _ = write!(self.out, r##"<use href="#{}g{outline}""##, self.ids);
                    transform(*m, &mut self.out);
                    self.fill(fill.as_ref());
                    self.stroke(stroke.as_ref());
                    self.out.push_str("/>");
                }
                Item::Image(image) => self.image(image),
                Item::Group(group) => self.group(group),
            }
        }
    }

    fn gradient(&mut self, index: usize, gradient: &Gradient) {
        let (tag, geometry) = match gradient.kind {
            GradientKind::Linear { from, to } => (
                "linearGradient",
                format!(
                    r#"x1="{}" y1="{}" x2="{}" y2="{}""#,
                    num(from[0]),
                    num(from[1]),
                    num(to[0]),
                    num(to[1])
                ),
            ),
            GradientKind::Radial {
                from,
                from_radius,
                to,
                to_radius,
            } => (
                "radialGradient",
                format!(
                    r#"fx="{}" fy="{}" fr="{}" cx="{}" cy="{}" r="{}""#,
                    num(from[0]),
                    num(from[1]),
                    num(from_radius),
                    num(to[0]),
                    num(to[1]),
                    num(to_radius)
                ),
            ),
        };
        let _ = write!(
            self.out,
            r#"<{tag} id="{}d{index}" gradientUnits="userSpaceOnUse" {geometry}"#,
            self.ids
        );
        if gradient.transform != Matrix::IDENTITY {
            let _ = write!(
                self.out,
                r#" gradientTransform="{}""#,
                matrix(gradient.transform)
            );
        }
        self.out.push('>');
        for stop in &gradient.stops {
            let _ = write!(
                self.out,
                r#"<stop offset="{}" stop-color="{}""#,
                num(stop.offset.clamp(0.0, 1.0)),
                hex(stop.color)
            );
            if stop.opacity < 1.0 {
                let _ = write!(
                    self.out,
                    r#" stop-opacity="{}""#,
                    num(stop.opacity.max(0.0))
                );
            }
            self.out.push_str("/>");
        }
        let _ = write!(self.out, "</{tag}>");
    }
}

fn blend_name(blend: Blend) -> &'static str {
    match blend {
        Blend::Multiply => "multiply",
        Blend::Screen => "screen",
        Blend::Overlay => "overlay",
        Blend::Darken => "darken",
        Blend::Lighten => "lighten",
        Blend::ColorDodge => "color-dodge",
        Blend::ColorBurn => "color-burn",
        Blend::HardLight => "hard-light",
        Blend::SoftLight => "soft-light",
        Blend::Difference => "difference",
        Blend::Exclusion => "exclusion",
        Blend::Hue => "hue",
        Blend::Saturation => "saturation",
        Blend::Color => "color",
        Blend::Luminosity => "luminosity",
    }
}

/// `page` as a standalone SVG in points on a white sheet. Every identifier it
/// defines starts with `ids`, so that pages set in one document do not reach
/// each other's definitions.
pub fn svg(page: &Page, ids: &str) -> String {
    let (w, h) = (num(page.width), num(page.height));
    let mut writer = Writer {
        ids,
        clips: 0,
        filters: 0,
        out: format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><rect width="{w}" height="{h}" fill="#ffffff"/>"##
        ),
    };
    let defined = !page.outlines.is_empty()
        || !page.gradients.is_empty()
        || !page.masks.is_empty()
        || !page.patterns.is_empty();
    if defined {
        writer.out.push_str("<defs>");
        for (index, outline) in page.outlines.iter().enumerate() {
            let _ = write!(writer.out, r#"<path id="{ids}g{index}" d=""#);
            writer.segments(outline);
            writer.out.push_str(r#""/>"#);
        }
        for (index, gradient) in page.gradients.iter().enumerate() {
            writer.gradient(index, gradient);
        }
        for (index, mask) in page.masks.iter().enumerate() {
            let _ = write!(
                writer.out,
                r#"<mask id="{ids}m{index}" maskUnits="userSpaceOnUse" x="-100000" y="-100000" width="200000" height="200000""#
            );
            if !mask.luminosity {
                writer.out.push_str(r#" mask-type="alpha""#);
            }
            writer.out.push('>');
            writer.items(&mask.items);
            writer.out.push_str("</mask>");
        }
        for (index, pattern) in page.patterns.iter().enumerate() {
            let _ = write!(
                writer.out,
                r#"<pattern id="{ids}t{index}" patternUnits="userSpaceOnUse" width="{}" height="{}""#,
                num(pattern.width),
                num(pattern.height)
            );
            if pattern.transform != Matrix::IDENTITY {
                let _ = write!(
                    writer.out,
                    r#" patternTransform="{}""#,
                    matrix(pattern.transform)
                );
            }
            writer.out.push('>');
            writer.items(&pattern.items);
            writer.out.push_str("</pattern>");
        }
        writer.out.push_str("</defs>");
    }
    writer.items(&page.items);
    writer.out.push_str("</svg>");
    writer.out
}

#[cfg(test)]
mod tests {
    use super::{base64, fixed};

    #[test]
    fn base64_pads_the_last_group() {
        for (data, expected) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foobar", "Zm9vYmFy"),
        ] {
            let mut out = String::new();
            base64(data, &mut out);
            assert_eq!(out, expected);
        }
    }

    #[test]
    fn numbers_drop_trailing_zeros_and_negative_zero() {
        assert_eq!(fixed(12.0, 3), "12");
        assert_eq!(fixed(0.1234, 3), "0.123");
        assert_eq!(fixed(-0.0001, 3), "0");
        assert_eq!(fixed(1500.0, 3), "1500");
    }
}

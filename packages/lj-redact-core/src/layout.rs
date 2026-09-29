//! The words of one page with their boxes, grouped in lines, which is what a
//! viewer lays over a drawn page so the text on it can be selected.

use crate::Anchor;

/// A box in page space: `[x0, y0, x1, y1]`, with `x0 <= x1` and `y0 <= y1`.
pub type Bounds = [f64; 4];

pub struct Word {
    pub text: String,
    pub bounds: Bounds,
}

pub struct Line {
    pub words: Vec<Word>,
}

pub(crate) fn bounds(quad: &[[f64; 2]; 4]) -> Bounds {
    let xs = quad.iter().map(|p| p[0]);
    let ys = quad.iter().map(|p| p[1]);
    [
        xs.clone().fold(f64::INFINITY, f64::min),
        ys.clone().fold(f64::INFINITY, f64::min),
        xs.fold(f64::NEG_INFINITY, f64::max),
        ys.fold(f64::NEG_INFINITY, f64::max),
    ]
}

fn height(b: &Bounds) -> f64 {
    b[3] - b[1]
}

/// Two glyphs sit on one baseline when their boxes overlap over half the
/// smaller height.
pub(crate) fn same_baseline(previous: &Bounds, next: &Bounds) -> bool {
    let overlap = previous[3].min(next[3]) - previous[1].max(next[1]);
    let smaller = height(previous).min(height(next)).max(f64::EPSILON);
    overlap >= 0.5 * smaller
}

/// A superscript or a subscript set right after the glyph before it, as
/// `M` then `me`: the boxes touch in height, and the pen did not jump.
pub(crate) fn raised(previous: &Bounds, next: &Bounds) -> bool {
    let overlap = previous[3].min(next[3]) - previous[1].max(next[1]);
    overlap > 0.0 && next[0] >= previous[0] && !apart(previous, next)
}

/// Two glyphs share a line when they sit on one baseline and the second does
/// not jump back left of the first.
fn same_line(previous: &Bounds, next: &Bounds) -> bool {
    let smaller = height(previous).min(height(next)).max(f64::EPSILON);
    same_baseline(previous, next) && next[0] >= previous[0] - smaller
}

/// The pen moved on one baseline by more than a tenth of the glyph height,
/// the word break pdftotext reads (`minWordBreakSpace`): a justified line
/// sets its word spaces by position well under the gap of [`apart`].
pub(crate) fn word_space(previous: &Bounds, next: &Bounds) -> bool {
    same_baseline(previous, next)
        && next[0] - previous[2] > 0.1 * height(previous).max(height(next))
}

/// Many PDFs place words apart without any space glyph between them.
pub(crate) fn apart(previous: &Bounds, next: &Bounds) -> bool {
    next[0] - previous[2] > 0.25 * height(previous).max(height(next))
}

/// The lines of words of `page`, from glyphs in page order as
/// [`crate::pdf::glyphs`] gives them.
pub fn page_lines(glyphs: &[(char, Anchor)], page: usize) -> Vec<Line> {
    let on = |(_, anchor): &(char, Anchor)| anchor.pdf().map(|(on, _)| on);
    let start = glyphs.partition_point(|g| on(g).is_some_and(|on| on < page));
    let end = start + glyphs[start..].partition_point(|g| on(g).is_some_and(|on| on == page));
    let glyphs = &glyphs[start..end];
    let mut lines: Vec<Line> = Vec::new();
    let mut current: Vec<Word> = Vec::new();
    let mut word: Option<Word> = None;
    let mut last: Option<Bounds> = None;

    let flush_word = |word: &mut Option<Word>, current: &mut Vec<Word>| {
        if let Some(done) = word.take() {
            current.push(done);
        }
    };
    let flush_line = |current: &mut Vec<Word>, lines: &mut Vec<Line>| {
        if !current.is_empty() {
            lines.push(Line {
                words: std::mem::take(current),
            });
        }
    };

    for (character, anchor) in glyphs {
        let Some((on, quad)) = anchor.pdf() else {
            continue;
        };
        if on != page {
            continue;
        }
        if *character == '\n' {
            flush_word(&mut word, &mut current);
            flush_line(&mut current, &mut lines);
            last = None;
            continue;
        }
        if character.is_whitespace() {
            flush_word(&mut word, &mut current);
            continue;
        }
        let glyph = bounds(&quad);
        if let Some(previous) = last {
            if !same_line(&previous, &glyph) {
                flush_word(&mut word, &mut current);
                flush_line(&mut current, &mut lines);
            } else if apart(&previous, &glyph) {
                flush_word(&mut word, &mut current);
            }
        }
        match word.as_mut() {
            Some(open) => {
                open.text.push(*character);
                open.bounds = [
                    open.bounds[0].min(glyph[0]),
                    open.bounds[1].min(glyph[1]),
                    open.bounds[2].max(glyph[2]),
                    open.bounds[3].max(glyph[3]),
                ];
            }
            None => {
                word = Some(Word {
                    text: character.to_string(),
                    bounds: glyph,
                })
            }
        }
        last = Some(glyph);
    }
    flush_word(&mut word, &mut current);
    flush_line(&mut current, &mut lines);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(c: char, x: f64, y: f64, page: usize) -> (char, Anchor) {
        let (w, h) = (5.0, 10.0);
        (
            c,
            Anchor::PdfGlyph {
                page,
                quad: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            },
        )
    }

    fn text(lines: &[Line]) -> Vec<Vec<&str>> {
        lines
            .iter()
            .map(|l| l.words.iter().map(|w| w.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn words_split_on_spaces_on_gaps_and_lines_on_the_baseline() {
        let mut glyphs = Vec::new();
        for (i, c) in "Me".chars().enumerate() {
            glyphs.push(glyph(c, 10.0 + 5.0 * i as f64, 100.0, 0));
        }
        glyphs.push(glyph(' ', 20.0, 100.0, 0));
        for (i, c) in "Dupont".chars().enumerate() {
            glyphs.push(glyph(c, 25.0 + 5.0 * i as f64, 100.0, 0));
        }
        // No space glyph, only a gap: still a new word.
        glyphs.push(glyph('X', 80.0, 100.0, 0));
        // Same x again, lower baseline: a new line without any '\n'.
        glyphs.push(glyph('a', 10.0, 80.0, 0));
        glyphs.push(glyph('\n', 15.0, 80.0, 0));
        glyphs.push(glyph('b', 10.0, 60.0, 0));
        // Another page is left out.
        glyphs.push(glyph('z', 10.0, 60.0, 1));

        let lines = page_lines(&glyphs, 0);
        assert_eq!(
            text(&lines),
            vec![vec!["Me", "Dupont", "X"], vec!["a"], vec!["b"]]
        );
        assert_eq!(lines[0].words[1].bounds, [25.0, 100.0, 55.0, 110.0]);
    }
}

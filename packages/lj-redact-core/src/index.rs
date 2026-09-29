//! Folded text that still knows where every character came from.

use std::ops::Range;

use crate::fold::{fold_char_before, fold_query, Spaces};

/// Where one source character sits in the document it came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// A glyph drawn on a PDF page, as the four corners of its quad in page
    /// space. The quad and not a box, because the engine matches a glyph by the
    /// quad its own extractor produced.
    #[cfg(feature = "pdf")]
    PdfGlyph { page: usize, quad: [[f64; 2]; 4] },
    /// A character of a Word document, as `lj-office` reads and edits it.
    #[cfg(feature = "office")]
    Office(lj_office::Anchor),
}

impl Anchor {
    /// The page and the quad of a PDF glyph.
    #[cfg(feature = "pdf")]
    pub fn pdf(&self) -> Option<(usize, [[f64; 2]; 4])> {
        match *self {
            Self::PdfGlyph { page, quad } => Some((page, quad)),
            #[cfg(feature = "office")]
            Self::Office(_) => None,
        }
    }

    /// The character of a Word document.
    #[cfg(feature = "office")]
    pub fn office(&self) -> Option<lj_office::Anchor> {
        match *self {
            Self::Office(anchor) => Some(anchor),
            #[cfg(feature = "pdf")]
            Self::PdfGlyph { .. } => None,
        }
    }
}

/// Folded characters, one anchor each, pointing at the source character the
/// folded one came from.
pub struct TextIndex {
    chars: Vec<char>,
    anchors: Vec<Anchor>,
    /// Index of the source character each folded one came from. A fold can emit
    /// two characters for one glyph, so this is not the identity.
    sources: Vec<usize>,
    spaces: Spaces,
}

impl TextIndex {
    /// Fold a source feed of `(character, anchor)` into a searchable index.
    pub fn build<I>(source: I, spaces: Spaces) -> Self
    where
        I: IntoIterator<Item = (char, Anchor)>,
    {
        let mut chars = Vec::new();
        let mut anchors = Vec::new();
        let mut sources = Vec::new();
        let mut folded = Vec::new();
        let mut previous_space = true;
        let mut source = source.into_iter().enumerate().peekable();
        while let Some((at, (c, anchor))) = source.next() {
            folded.clear();
            fold_char_before(c, source.peek().map(|(_, (n, _))| *n), &mut folded);
            for f in &folded {
                let space = *f == ' ';
                let drop = match spaces {
                    Spaces::Strip => space,
                    Spaces::Collapse => space && previous_space,
                };
                previous_space = space;
                if !drop {
                    chars.push(*f);
                    anchors.push(anchor);
                    sources.push(at);
                }
            }
        }
        if chars.last() == Some(&' ') {
            chars.pop();
            anchors.pop();
            sources.pop();
        }
        Self {
            chars,
            anchors,
            sources,
            spaces,
        }
    }

    /// The folded text, for a caller that wants to read or display it.
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Character ranges where the folded `query` occurs.
    pub fn find(&self, query: &str) -> Vec<Range<usize>> {
        let needle = fold_query(query, self.spaces);
        if needle.is_empty() || needle.len() > self.chars.len() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        let last = self.chars.len() - needle.len();
        let mut start = 0;
        while start <= last {
            if self.chars[start..start + needle.len()] == needle[..] {
                hits.push(start..start + needle.len());
                start += needle.len();
            } else {
                start += 1;
            }
        }
        hits
    }

    /// The anchors of the source characters covered by `range`.
    pub fn anchors(&self, range: Range<usize>) -> &[Anchor] {
        &self.anchors[range]
    }

    /// Where `range` sits in the source feed, for a caller that displays the
    /// document as its own characters rather than as folded ones.
    pub fn source(&self, range: Range<usize>) -> Range<usize> {
        match (
            self.sources.get(range.start),
            self.sources.get(range.end - 1),
        ) {
            (Some(first), Some(last)) => *first..*last + 1,
            _ => 0..0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tests::{glyphs, position};

    fn x_positions(index: &TextIndex, hit: Range<usize>) -> Vec<f64> {
        index
            .anchors(hit)
            .iter()
            .map(|a| position(a) as f64)
            .collect()
    }

    #[test]
    fn a_match_points_back_at_the_exact_source_glyphs() {
        let index = TextIndex::build(glyphs("Maitre Dupont ici"), Spaces::Collapse);
        let hits = index.find("dupont");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            x_positions(&index, hits[0].clone()),
            vec![7.0, 8.0, 9.0, 10.0, 11.0, 12.0]
        );
    }

    #[test]
    fn an_accented_name_matches_its_unaccented_query_without_shifting_anchors() {
        let index = TextIndex::build(glyphs("affaire Noel close"), Spaces::Collapse);
        let hits = index.find("noel");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            x_positions(&index, hits[0].clone()),
            vec![8.0, 9.0, 10.0, 11.0]
        );
    }

    #[test]
    fn a_ligature_folds_to_two_characters_both_anchored_to_the_one_glyph() {
        let index = TextIndex::build(glyphs("le \u{fb01}ls"), Spaces::Collapse);
        let hits = index.find("fils");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            x_positions(&index, hits[0].clone()),
            vec![3.0, 3.0, 4.0, 5.0]
        );
    }

    #[test]
    fn collapsing_a_run_of_spaces_keeps_the_anchors_aligned() {
        let index = TextIndex::build(glyphs("a   Dupont"), Spaces::Collapse);
        assert_eq!(index.text(), "a dupont");
        let hits = index.find("dupont");
        assert_eq!(
            x_positions(&index, hits[0].clone()),
            vec![4.0, 5.0, 6.0, 7.0, 8.0, 9.0]
        );
    }

    #[test]
    fn stripping_spaces_finds_a_name_the_producer_let_spaced() {
        let index = TextIndex::build(glyphs("D U P O N T"), Spaces::Strip);
        let hits = index.find("Dupont");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            x_positions(&index, hits[0].clone()),
            vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]
        );
    }

    #[test]
    fn every_occurrence_is_returned_not_only_the_first() {
        let index = TextIndex::build(glyphs("Dupont contre Dupont"), Spaces::Collapse);
        assert_eq!(index.find("dupont").len(), 2);
    }

    #[test]
    fn an_empty_or_oversized_query_matches_nothing() {
        let index = TextIndex::build(glyphs("court"), Spaces::Collapse);
        assert!(index.find("").is_empty());
        assert!(index.find("much longer than the source").is_empty());
    }
}

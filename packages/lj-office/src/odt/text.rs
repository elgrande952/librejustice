//! The document as plain text, laid out the way LibreOffice exports it: one line
//! per paragraph, list labels kept, note citations in the line and note bodies,
//! frames, headers and footers left out.

use super::model::*;

pub fn plain_text(doc: &Document) -> String {
    let mut out = String::new();
    blocks(&doc.body, &mut out);
    out
}

fn blocks(blocks_: &[Block], out: &mut String) {
    for b in blocks_ {
        match b {
            Block::Paragraph(p) => paragraph(p, out),
            Block::Table(t) => {
                for row in &t.rows {
                    for cell in row.cells.iter().filter(|c| !c.covered) {
                        blocks(&cell.blocks, out);
                    }
                }
            }
            Block::Section(s) => blocks(&s.blocks, out),
            Block::SoftPageBreak => {}
        }
    }
}

fn paragraph(p: &Paragraph, out: &mut String) {
    if let Some(label) = &p.label {
        // LibreOffice writes a bullet by level, whatever the character.
        if label.bullet {
            out.push(['\u{2022}', '\u{25e6}', '\u{25aa}'][(label.level as usize - 1) % 3]);
        } else {
            out.push_str(&label.text);
        }
        out.push(' ');
    }
    for inline in &p.inlines {
        match inline {
            Inline::Text(run) => out.push_str(&run.text),
            Inline::Tab { .. } => out.push('\t'),
            Inline::LineBreak { .. } => out.push('\n'),
            Inline::Field { run, .. } => out.push_str(&run.text),
            Inline::NoteRef { citation, .. } => out.push_str(citation),
            Inline::Frame(_) | Inline::SoftPageBreak => {}
        }
    }
    out.push('\n');
}

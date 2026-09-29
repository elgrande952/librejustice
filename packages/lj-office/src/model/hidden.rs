//! Paragraphs whose mark is hidden, laid out as LibreOffice lays them out.

use super::{Block, Inline, Paragraph};

/// Joins each paragraph whose mark is hidden to the next, with the
/// properties of the first that shows something; one with no next paragraph
/// goes when it shows nothing.
pub(crate) fn join_hidden_marks(blocks: &mut Vec<Block>) {
    if !blocks.iter().any(carries_hidden_mark) {
        return;
    }
    let mut out = Vec::with_capacity(blocks.len());
    let mut open: Option<Box<Paragraph>> = None;
    for block in blocks.drain(..) {
        match block {
            Block::Paragraph(mut paragraph) => {
                if let Some(mut first) = open.take() {
                    if shows(&first) {
                        first.inlines.append(&mut paragraph.inlines);
                        first.mark = paragraph.mark;
                        paragraph = first;
                    } else {
                        paragraph.inlines.splice(0..0, first.inlines);
                    }
                }
                if paragraph.mark.hidden {
                    open = Some(paragraph);
                } else {
                    out.push(Block::Paragraph(paragraph));
                }
            }
            Block::Table(mut table) => {
                close(&mut out, open.take());
                for cell in table.rows.iter_mut().flat_map(|r| r.cells.iter_mut()) {
                    join_hidden_marks(&mut cell.blocks);
                }
                out.push(Block::Table(table));
            }
        }
    }
    match open {
        Some(last) if out.is_empty() => out.push(Block::Paragraph(last)),
        last => close(&mut out, last),
    }
    *blocks = out;
}

fn carries_hidden_mark(block: &Block) -> bool {
    match block {
        Block::Paragraph(p) => p.mark.hidden,
        Block::Table(t) => t
            .rows
            .iter()
            .flat_map(|r| &r.cells)
            .any(|c| c.blocks.iter().any(carries_hidden_mark)),
    }
}

fn close(out: &mut Vec<Block>, paragraph: Option<Box<Paragraph>>) {
    let Some(paragraph) = paragraph else {
        return;
    };
    if shows(&paragraph) {
        out.push(Block::Paragraph(paragraph));
    }
}

fn shows(paragraph: &Paragraph) -> bool {
    paragraph.inlines.iter().any(|inline| match inline {
        Inline::Text(t) => !t.props.hidden && !t.text.is_empty(),
        Inline::Char(_, props)
        | Inline::Tab(props)
        | Inline::Field(_, props)
        | Inline::Note(_, props) => !props.hidden,
        Inline::Break(_) | Inline::Drawing(_) => true,
    })
}

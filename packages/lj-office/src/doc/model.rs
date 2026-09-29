//! What the reader makes of a `.doc`: stories of paragraphs and tables, each
//! character still addressed by its CP.

use std::ops::Range;

use super::props::{Chp, Pap, Sep, Tap};
use super::sttb::Font;
use super::text::Story;

pub struct Document {
    pub main: Vec<Block>,
    pub sections: Vec<Section>,
    /// The header story cut by PlcfHdd: six separators, then six stories per
    /// section (even header, odd header, even footer, odd footer, first
    /// header, first footer). An empty entry inherits the previous
    /// section's.
    pub headers: Vec<Vec<Block>>,
    pub footnotes: Vec<Note>,
    pub endnotes: Vec<Note>,
    pub comments: Vec<Note>,
    pub textboxes: Vec<Vec<Block>>,
    pub header_textboxes: Vec<Vec<Block>>,
    pub fonts: Vec<Font>,
    pub pictures: Vec<Picture>,
    /// What the reader skipped or could not make sense of.
    pub warnings: Vec<String>,
}

pub struct Section {
    /// Main-story CPs the section covers, its section mark included.
    pub cp: Range<u32>,
    pub sep: Sep,
}

pub struct Note {
    /// Where the reference mark sits in the main story.
    pub reference: u32,
    /// Author of a comment: index into the comment owner table.
    pub author: Option<String>,
    pub blocks: Vec<Block>,
}

pub enum Block {
    Paragraph(Box<Paragraph>),
    Table(Table),
}

#[derive(Default)]
pub struct Paragraph {
    pub story: Story,
    /// CPs of the paragraph, its end mark included.
    pub cp: Range<u32>,
    pub pap: Pap,
    /// The character properties of the paragraph mark, which a list label and
    /// an empty paragraph's height follow.
    pub mark: Chp,
    pub label: Option<Label>,
    pub runs: Vec<Run>,
}

/// A list label: its text as the level's number text makes it, follow
/// character included, and the properties it is drawn with.
#[derive(Clone)]
pub struct Label {
    pub text: String,
    pub chp: Chp,
    /// The level is a bullet rather than a number.
    pub bullet: bool,
}

pub struct Run {
    /// First CP of the run. Each character covers one CP, except one outside
    /// the BMP, which covers two.
    pub cp: u32,
    pub chp: Chp,
    pub content: Content,
}

pub enum Content {
    Text(String),
    Tab,
    LineBreak,
    PageBreak,
    ColumnBreak,
    /// Index into `Document::pictures`.
    Picture(usize),
    /// A floating shape anchored here (sprmCFSpec 0x08).
    Shape,
    /// An embedded OLE object: its storage `_<id>` under ObjectPool.
    Object(u32),
    /// An automatically numbered note reference: its mark, the number in
    /// the section's or the document's format.
    NoteReference(String),
    CommentReference,
    /// A field's result follows until the matching `FieldEnd`; the
    /// instruction is what the field computes (`PAGE`, `HYPERLINK "…"`).
    FieldStart(String),
    FieldEnd,
    /// The entry a drop-down form field shows, which the text does not carry.
    Choice(String),
}

pub struct Table {
    pub depth: i32,
    pub rows: Vec<Row>,
}

pub struct Row {
    pub tap: Tap,
    pub cells: Vec<Vec<Block>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Emf,
    Wmf,
    Pict,
    Dib,
    Tiff,
}

pub struct Picture {
    /// Offset of the PICF in the Data stream.
    pub location: u32,
    /// Displayed size in twips, after scaling and cropping.
    pub width: u32,
    pub height: u32,
    /// Crop from each edge, in twips: left, top, right, bottom.
    pub crop: [i32; 4],
    pub format: Option<ImageFormat>,
    /// The image file as it would be saved: PNG, JPEG, or the decompressed
    /// metafile.
    pub bytes: Vec<u8>,
    /// The simple OfficeArt properties of the picture's shape.
    pub props: Vec<(u16, u32)>,
}

impl Paragraph {
    /// Move the paragraph out, leaving an empty one.
    pub fn take(&mut self) -> Paragraph {
        std::mem::take(self)
    }
}

impl Block {
    pub fn paragraphs<'a>(&'a self, out: &mut Vec<&'a Paragraph>) {
        match self {
            Block::Paragraph(p) => out.push(p),
            Block::Table(t) => {
                for row in &t.rows {
                    for cell in &row.cells {
                        for b in cell {
                            b.paragraphs(out);
                        }
                    }
                }
            }
        }
    }
}

use super::*;
use crate::model::{Block, Inline};

const PAGE: &str = concat!(
    "\u{FEFF}<html xmlns:o='urn:schemas-microsoft-com:office:office' xmlns:w='urn:schemas-microsoft-com:office:word' xmlns='http://www.w3.org/TR/REC-html40'>",
    "<head><meta http-equiv=Content-Type content=\"text/html; charset=utf-8\"><meta name=author content=\"Jeanne Roux\"><title>Dossier Lambert</title>",
    "<!--[if gte mso 9]><xml><o:DocumentProperties><o:Author>Jeanne Roux</o:Author><o:Pages>1</o:Pages></o:DocumentProperties></xml><![endif]-->",
    "<style><!-- p.MsoNormal {margin:0cm; font-size:11.0pt; font-family:\"Calibri\",sans-serif;} @page WordSection1 {size:595.3pt 841.9pt; margin:72pt 72pt 72pt 72pt;} --></style></head>",
    "<body><div class=WordSection1>",
    "<h1 style='text-align:center'>Compte rendu</h1>",
    "<p class=MsoNormal>Reçu   M.\n<strong>Paul Lambert</strong>&nbsp;&amp; <em>associés</em>,<br>le 3 mars.</p>",
    "<ul><li>premier point<li>second <u>point</u></ul>",
    "<ol><li>un</li><li>deux</li></ol>",
    "<table border=1 width=\"100%\"><tr><td>Nom</td><td style='background-color:#ffeb9c'>Lambert</td></tr><tr><td colspan=2>fin</td></tr></table>",
    "<p class=MsoNormal><span style='display:none'>cache</span>visible</p>",
    "</div></body></html>",
);

fn texts(blocks: &[Block], out: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Paragraph(p) => {
                let mut line = p
                    .label
                    .as_ref()
                    .map_or(String::new(), |l| format!("{} ", l.text));
                for inline in &p.inlines {
                    match inline {
                        Inline::Text(t) => line.push_str(&t.text),
                        Inline::Break(_) => line.push('|'),
                        _ => {}
                    }
                }
                out.push(line);
            }
            Block::Table(t) => {
                for cell in t.rows.iter().flat_map(|r| &r.cells) {
                    texts(&cell.blocks, out);
                }
            }
        }
    }
}

#[test]
fn a_word_web_page_reads_as_paragraphs_lists_and_a_table() {
    assert_eq!(
        crate::Format::of(PAGE.as_bytes()).unwrap(),
        crate::Format::Html
    );
    let document = crate::open(PAGE.as_bytes()).unwrap();
    let mut lines = Vec::new();
    texts(&document.sections[0].blocks, &mut lines);
    assert_eq!(
        lines,
        [
            "Compte rendu",
            "Reçu M. Paul Lambert\u{A0}& associés,|le 3 mars.",
            "\u{2022} premier point",
            "\u{2022} second point",
            "1. un",
            "2. deux",
            "Nom",
            "Lambert",
            "fin",
            "cachevisible",
        ]
    );
    let Some(Block::Table(table)) = document.sections[0].blocks.get(6) else {
        panic!("the table after the lists");
    };
    assert_eq!(table.grid.len(), 2);
    assert!((table.grid.iter().sum::<f64>() - (595.3 - 144.0)).abs() < 0.01);
    assert_eq!(table.rows[1].cells[0].span, 2);
}

#[test]
fn every_character_is_anchored_where_the_splice_finds_it_title_and_author_included() {
    let package = HtmlPackage::open(PAGE.as_bytes()).unwrap();
    let feed = package.characters();
    let text: String = feed.iter().map(|(c, _)| *c).collect();
    assert!(text.contains("Dossier Lambert\n"), "{text:?}");
    assert!(text.contains("Jeanne Roux\n"), "{text:?}");
    for (c, anchor) in &feed {
        let Anchor::XmlText {
            part: 0,
            node,
            offset,
        } = *anchor
        else {
            panic!("{anchor:?}");
        };
        let at = package.text(node).chars().nth(offset as usize);
        match c {
            ' ' | '\n' => assert!(at.is_some()),
            c => assert_eq!(at, Some(*c)),
        }
    }
    let views = package.views();
    let hidden: String = feed
        .iter()
        .zip(&views)
        .filter(|(_, v)| **v & 0b10 == 0)
        .map(|((c, _), _)| *c)
        .collect();
    assert_eq!(hidden, "cache");
}

#[test]
fn a_replacement_rewrites_its_text_node_alone_and_the_authors_go() {
    let bytes = PAGE.replace("charset=utf-8", "charset=windows-1252");
    let (encoded, _, _) = encoding_rs::WINDOWS_1252.encode(bytes.trim_start_matches('\u{FEFF}'));
    let mut package = HtmlPackage::open(&encoded).unwrap();
    let feed = package.characters();
    let text: String = feed.iter().map(|(c, _)| *c).collect();
    let at = text[..text.find("Paul Lambert").unwrap()].chars().count();
    let anchors: Vec<Anchor> = feed[at..at + "Paul Lambert".len()]
        .iter()
        .map(|(_, a)| *a)
        .collect();
    let removed = package
        .replace(&[Replacement {
            anchors,
            text: "[PERSONNE 1] <é>".into(),
        }])
        .unwrap();
    assert_eq!(removed, 12);
    package.clear_authors();
    let saved = package.save().unwrap();
    let (decoded, _) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(&saved);
    assert!(
        decoded.contains("<strong>[PERSONNE 1] &lt;é&gt;</strong>"),
        "{decoded}"
    );
    assert!(decoded.contains("<title></title>") && decoded.contains("<o:Author></o:Author>"));
    assert!(
        decoded.contains("<meta name=author content=\"\">"),
        "{decoded}"
    );
    assert!(decoded.contains("Reçu   M.\n<strong>"), "{decoded}");
    let reread = HtmlPackage::open(&saved).unwrap();
    let text: String = reread.characters().iter().map(|(c, _)| *c).collect();
    assert!(
        !text.contains("Paul") && !text.contains("Roux") && !text.contains("Dossier"),
        "{text}"
    );
    assert!(text.contains("Nom\nLambert"), "{text}");
}

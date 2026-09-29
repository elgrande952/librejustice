use super::package::{write, Entry};
use super::OdtPackage;
use crate::{Anchor, Replacement};

const NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0""#;

fn odt(body: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {NS} office:version="1.3"><office:body><office:text>{body}</office:text></office:body></office:document-content>"#
    );
    let manifest = r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.text"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="Thumbnails/thumbnail.png" manifest:media-type="image/png"/></manifest:manifest>"#;
    let mut entries = vec![
        Entry {
            name: "content.xml".into(),
            data: content.into_bytes(),
            deflated: true,
        },
        Entry {
            name: "mimetype".into(),
            data: b"application/vnd.oasis.opendocument.text".to_vec(),
            deflated: true,
        },
        Entry {
            name: "META-INF/manifest.xml".into(),
            data: manifest.as_bytes().to_vec(),
            deflated: true,
        },
    ];
    for (name, data) in extra {
        entries.push(Entry {
            name: name.to_string(),
            data: data.to_vec(),
            deflated: true,
        });
    }
    let refs: Vec<(&Entry, Option<Vec<u8>>)> = entries.iter().map(|e| (e, None)).collect();
    write(&refs).unwrap()
}

#[test]
fn whitespace_collapses_as_writer_reads_it() {
    let body = r#"<text:p>  a  <text:s text:c="2"/> b<text:tab/> c <text:span> d</text:span><text:line-break/> e  </text:p>"#;
    assert_eq!(
        super::read_text(&odt(body, &[])).unwrap(),
        "a    b\t c d\n e \n"
    );
}

fn anchors_of(feed: &[(char, Anchor)], at: usize, len: usize) -> Vec<Anchor> {
    feed[at..at + len].iter().map(|(_, a)| *a).collect()
}

#[test]
fn a_replacement_lands_where_its_first_character_stood_even_in_a_space_element() {
    let input = odt(
        r#"<text:p>Signé <text:span>Ma</text:span>rtin, a<text:s text:c="3"/>b</text:p>"#,
        &[],
    );
    let mut package = OdtPackage::open(&input).unwrap();
    let feed = package.characters();
    let text: String = feed.iter().map(|(c, _)| *c).collect();
    let martin = text.chars().position(|c| c == 'M').unwrap();
    let space = text.chars().position(|c| c == ',').unwrap() + 4;
    let replacements = [
        Replacement {
            anchors: anchors_of(&feed, martin, 6),
            text: "[A]".into(),
        },
        Replacement {
            anchors: anchors_of(&feed, space, 1),
            text: "<X>".into(),
        },
    ];
    assert_eq!(package.replace(&replacements).unwrap(), 7);
    assert_eq!(
        super::read_text(&package.save().unwrap()).unwrap(),
        "Signé [A], a <X> b\n"
    );
}

#[test]
fn a_page_frame_skips_a_hidden_section_for_the_next_shown_paragraph() {
    let body = r#"<draw:frame xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" text:anchor-type="page" text:anchor-page-number="1" svg:width="3cm" svg:height="1cm"><draw:text-box><text:p>logo</text:p></draw:text-box></draw:frame><text:section text:name="S" text:display="none"><text:p>hidden</text:p></text:section><text:p>shown</text:p>"#;
    let document = crate::open(&odt(body, &[])).unwrap();
    let drawings: Vec<(bool, usize)> = document.sections[0]
        .blocks
        .iter()
        .filter_map(|b| match b {
            crate::model::Block::Paragraph(p) => Some((
                p.hidden_section,
                p.inlines
                    .iter()
                    .filter(|i| matches!(i, crate::model::Inline::Drawing(_)))
                    .count(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(drawings, [(true, 0), (false, 1)]);
}

//! An `.odt`: the needles give way to their labels in the text, and leave the
//! bytes of every entry no XML reader walks, nested packages included.

use anyhow::Result;
use lj_office::odt::OdtPackage;

use super::{anchored, byte_patterns, erase_bytes, found_in, replacements, survivors};
use crate::labels::Labels;
use crate::Outcome;

/// Replace every occurrence of `needles` by its label, empty the author
/// metadata, drop the thumbnails, saved versions and signatures, then verify
/// on the bytes produced that none survives, in a part or in any other entry.
pub(super) fn redact(bytes: &[u8], needles: &[String], labels: &Labels) -> Result<Outcome> {
    let mut package = OdtPackage::open(bytes)?;
    let characters = anchored(package.characters());
    let views = lj_office::view_positions(&package.views()?);
    let source_chars = characters.len();
    let (found, replacements) = replacements(&[characters], &views, needles, labels)?;

    let removed = package.replace(&replacements)?;
    package.clear_authors();
    package.drop_thumbnails();
    package.rewrite_binaries(|data| {
        needles
            .iter()
            .map(|needle| erase_bytes(data, needle))
            .sum::<usize>()
            > 0
    });
    let produced = package.save()?;
    let leaks = leaks(&produced, needles)?;
    Ok(Outcome {
        bytes: produced,
        found,
        removed,
        leaks,
        source_chars,
    })
}

/// Needles a reader still finds in the text or in the bytes of an entry no
/// XML reader walks.
pub(super) fn leaks(bytes: &[u8], needles: &[String]) -> Result<Vec<String>> {
    let package = OdtPackage::open(bytes)?;
    let views = lj_office::view_positions(&package.views()?);
    let mut leaks = survivors(&anchored(package.characters()), &views, needles);
    leaks.extend(binary_leaks(&package, needles));
    Ok(leaks)
}

/// Needles readable in an entry no XML reader walks, read as UTF-8, UTF-16LE
/// at both alignments and Latin-1, case folded, each named with its entry.
fn binary_leaks(package: &OdtPackage, needles: &[String]) -> Vec<String> {
    let patterns: Vec<(usize, Vec<u8>)> = needles
        .iter()
        .enumerate()
        .flat_map(|(i, n)| byte_patterns(n).into_iter().map(move |(p, _)| (i, p)))
        .collect();
    let mut leaks = Vec::new();
    for (name, data) in package.binaries() {
        for i in found_in(data, &patterns) {
            leaks.push(format!("{name}: {}", needles[i]));
        }
    }
    leaks
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use lj_office::odt::package::{write, Entry};
    use lj_office::odt::read_text;

    const NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0""#;

    fn redact_all(bytes: &[u8], needles: &[&str]) -> crate::Outcome {
        let (texts, labels) = super::super::erasing(needles);
        super::redact(bytes, &texts, &labels).unwrap()
    }

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
    fn a_name_is_removed_wherever_a_reader_or_the_package_carries_it() {
        let body = concat!(
            r#"<text:tracked-changes><text:changed-region text:id="c1"><text:deletion><office:change-info><dc:creator>Paul Dupont</dc:creator></office:change-info><text:p>Dup</text:p></text:deletion></text:changed-region></text:tracked-changes>"#,
            r#"<text:p>Signé <text:span>Ma</text:span>rtin<office:annotation><dc:creator>Martin</dc:creator><text:p>vu par Martin</text:p></office:annotation> et <text:change text:change-id="c1"/>ont.</text:p>"#,
            r#"<text:p text:style-name="Martin">Note<text:note text:id="n1" text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p>M. MARTIN</text:p></text:note-body></text:note></text:p>"#,
        );
        let meta = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><office:document-meta {NS}><office:meta><dc:title>Dossier Martin</dc:title><meta:initial-creator>Jeanne</meta:initial-creator></office:meta></office:document-meta>"#
        );
        let input = odt(
            body,
            &[
                ("meta.xml", meta.as_bytes()),
                ("Thumbnails/thumbnail.png", b"\x89PNG Martin"),
            ],
        );
        let outcome = redact_all(&input, &["Martin", "Dupont"]);
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);

        let mut archive = zip::ZipArchive::new(Cursor::new(&outcome.bytes)).unwrap();
        let first = archive.by_index_raw(0).unwrap();
        assert_eq!(
            (first.name(), first.compression()),
            ("mimetype", zip::CompressionMethod::Stored)
        );
        drop(first);
        let names: Vec<String> = archive.file_names().map(str::to_string).collect();
        assert!(
            !names.iter().any(|n| n.starts_with("Thumbnails/")),
            "{names:?}"
        );
        for name in &names {
            let mut data = String::new();
            archive
                .by_name(name)
                .unwrap()
                .read_to_string(&mut data)
                .unwrap();
            let lower = data.to_lowercase();
            assert!(
                !lower.contains("martin") && !lower.contains("dupont"),
                "{name}: {data}"
            );
            assert!(!lower.contains("jeanne"), "{name} keeps the author: {data}");
        }
        assert_eq!(read_text(&outcome.bytes).unwrap(), "Signé et .\nNote1\n");
    }

    #[test]
    fn saved_versions_and_signatures_leave_the_package() {
        let body = r#"<text:p xml:id="p1">Signé par Martin</text:p>"#;
        let rdf = r#"<?xml version="1.0" encoding="utf-8"?><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="../content.xml#p1"><ns0:value xmlns:ns0="urn:bails:loext:paragraph:classification:">interne</ns0:value></rdf:Description><rdf:Description rdf:about="../content.xml#p1"><ns0:digest xmlns:ns0="urn:bails:loext:paragraph:signature:1:">4D617274696E</ns0:digest></rdf:Description></rdf:RDF>"#;
        let input = odt(
            body,
            &[
                ("Versions/Version1", b"PK\x03\x04 Martin"),
                ("VersionList.xml", b"<VL:version-list/>"),
                (
                    "META-INF/documentsignatures.xml",
                    b"<document-signatures>CN=Martin</document-signatures>",
                ),
                ("tscp/bails.rdf", rdf.as_bytes()),
            ],
        );
        let outcome = redact_all(&input, &["Martin"]);
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        let mut archive = zip::ZipArchive::new(Cursor::new(&outcome.bytes)).unwrap();
        let names: Vec<String> = archive.file_names().map(str::to_string).collect();
        for gone in [
            "Versions/Version1",
            "VersionList.xml",
            "META-INF/documentsignatures.xml",
        ] {
            assert!(!names.iter().any(|n| n == gone), "{gone} kept: {names:?}");
        }
        let mut rdf = String::new();
        archive
            .by_name("tscp/bails.rdf")
            .unwrap()
            .read_to_string(&mut rdf)
            .unwrap();
        assert!(rdf.contains("interne") && !rdf.contains("digest"), "{rdf}");
    }

    #[test]
    fn a_part_opening_on_a_byte_order_mark_is_spliced_where_its_text_is() {
        let input = odt(r#"<text:p>Signé par Martin ce jour</text:p>"#, &[]);
        let mut archive = zip::ZipArchive::new(Cursor::new(&input)).unwrap();
        let mut entries = Vec::new();
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            if file.name() == "content.xml" {
                data.splice(0..0, *b"\xEF\xBB\xBF");
            }
            entries.push(Entry {
                name: file.name().to_string(),
                data,
                deflated: true,
            });
        }
        let refs: Vec<(&Entry, Option<Vec<u8>>)> = entries.iter().map(|e| (e, None)).collect();
        let outcome = redact_all(&write(&refs).unwrap(), &["Martin"]);
        assert_eq!(read_text(&outcome.bytes).unwrap(), "Signé par ce jour\n");
    }

    #[test]
    fn a_template_path_an_author_property_and_a_picture_lose_their_names() {
        let meta = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><office:document-meta {NS} xmlns:xlink="http://www.w3.org/1999/xlink"><office:meta><meta:template xlink:type="simple" xlink:href="../../../Profils/jmartin/Modeles/Lettre.ott" xlink:title="Lettre"/><meta:user-defined meta:name="_AuthorEmail">jm@cabinet.fr</meta:user-defined><meta:user-defined meta:name="Dossier">RG 21/00042</meta:user-defined></office:meta></office:document-meta>"#
        );
        let jpeg = b"\xFF\xD8\xFF\xE1\x00\x20Exif\x00\x00Artist: Jean Martin\x00\xFF\xD9";
        let input = odt(
            r#"<text:p>Jean Martin</text:p>"#,
            &[("meta.xml", meta.as_bytes()), ("Pictures/photo.jpg", jpeg)],
        );
        let outcome = redact_all(&input, &["Martin"]);
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        let mut archive = zip::ZipArchive::new(Cursor::new(&outcome.bytes)).unwrap();
        let mut meta = String::new();
        archive
            .by_name("meta.xml")
            .unwrap()
            .read_to_string(&mut meta)
            .unwrap();
        for kept in [
            r#"xlink:href="Lettre.ott""#,
            r#"meta:name="_AuthorEmail"></meta:user-defined>"#,
            "RG 21/00042",
        ] {
            assert!(meta.contains(kept), "{kept} missing from {meta}");
        }
        let mut picture = Vec::new();
        archive
            .by_name("Pictures/photo.jpg")
            .unwrap()
            .read_to_end(&mut picture)
            .unwrap();
        assert_eq!(picture.len(), jpeg.len());
        assert!(picture.ends_with(b"Artist: Jean XXXXXX\x00\xFF\xD9"));
    }

    #[test]
    fn a_field_showing_an_emptied_author_is_fixed_to_the_name_it_showed() {
        let input = odt(
            r#"<text:p><text:initial-creator>Paul Durand</text:initial-creator>, <text:user-defined text:name="_AuthorEmail" text:fixed="false">pd@cabinet.fr</text:user-defined>, <text:user-defined text:name="Dossier">RG 21/00042</text:user-defined>, <text:creator/></text:p>"#,
            &[],
        );
        let outcome = redact_all(&input, &["Martin"]);
        let mut archive = zip::ZipArchive::new(Cursor::new(&outcome.bytes)).unwrap();
        let mut content = String::new();
        archive
            .by_name("content.xml")
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        for kept in [
            r#"<text:initial-creator text:fixed="true">Paul Durand</text:initial-creator>"#,
            r#"<text:user-defined text:name="_AuthorEmail" text:fixed="true">pd@cabinet.fr<"#,
            r#"<text:user-defined text:name="Dossier">RG"#,
            r#"<text:creator text:fixed="true"/>"#,
        ] {
            assert!(content.contains(kept), "{kept} missing from {content}");
        }
    }
}

//! A `.docx`, `.xlsx` or `.pptx`: the needles give way to their labels in the
//! text and the prose attributes, and leave the bytes of every entry no XML
//! reader walks.

use std::collections::BTreeSet;

use anyhow::Result;
use lj_office::ooxml::OoxmlPackage;

use super::{anchored, byte_patterns, erase_bytes, found_in, replacements, survivors};
use crate::labels::Labels;
use crate::Outcome;

/// Replace every occurrence of `needles` by its label, empty the author
/// metadata, drop the thumbnails and signatures, then verify on the bytes
/// produced that none survives.
pub(super) fn redact(bytes: &[u8], needles: &[String], labels: &Labels) -> Result<Outcome> {
    let mut package = OoxmlPackage::open(bytes)?;
    let feed = anchored(package.characters());
    let views = lj_office::view_positions(&package.views());
    let source_chars = feed.len();
    let (found, replacements) = replacements(&[feed], &views, needles, labels)?;

    let removed = package.replace(&replacements)?;
    package.clear_authors();
    package.drop_thumbnails();
    package.rewrite_binaries(&mut |data| {
        for needle in needles {
            erase_bytes(data, needle);
        }
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

/// Needles a reader still finds in the text, the prose attributes or the
/// bytes of an entry no XML reader walks.
pub(super) fn leaks(bytes: &[u8], needles: &[String]) -> Result<Vec<String>> {
    let package = OoxmlPackage::open(bytes)?;
    let views = lj_office::view_positions(&package.views());
    let mut leaks = survivors(&anchored(package.characters()), &views, needles);
    for leak in binary_leaks(&package, needles) {
        if !leaks.contains(&leak) {
            leaks.push(leak);
        }
    }
    Ok(leaks)
}

/// Needles readable in an entry no XML reader walks, a picture's metadata or
/// an OLE object's stream, read as UTF-8, UTF-16 and Latin-1.
fn binary_leaks(package: &OoxmlPackage, needles: &[String]) -> Vec<String> {
    let patterns: Vec<(usize, Vec<u8>)> = needles
        .iter()
        .enumerate()
        .flat_map(|(i, n)| byte_patterns(n).into_iter().map(move |(p, _)| (i, p)))
        .collect();
    let mut found = BTreeSet::new();
    for (_, data) in package.binaries() {
        found.extend(found_in(data, &patterns));
    }
    found.into_iter().map(|i| needles[i].clone()).collect()
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;
    use crate::{Spaces, TextIndex};

    fn package(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn docx(body: &str) -> Vec<u8> {
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="urn:w"><w:body><w:p>{body}</w:p></w:body></w:document>"#
        );
        package(&[
            ("[Content_Types].xml", b"<Types/>".to_vec()),
            ("_rels/.rels", b"<Relationships/>".to_vec()),
            ("word/document.xml", document.into_bytes()),
        ])
    }

    fn text_of(bytes: &[u8]) -> String {
        let package = OoxmlPackage::open(bytes).unwrap();
        TextIndex::build(anchored(package.characters()), Spaces::Collapse).text()
    }

    fn redact_all(bytes: &[u8], needles: &[&str]) -> Outcome {
        let (texts, labels) = super::super::erasing(needles);
        super::redact(bytes, &texts, &labels).unwrap()
    }

    fn redact(bytes: &[u8], needle: &str) -> Vec<u8> {
        redact_all(bytes, &[needle]).bytes
    }

    fn entry(bytes: &[u8], name: &str) -> Option<String> {
        let package = OoxmlPackage::open(bytes).unwrap();
        package
            .entry(name)
            .map(|data| String::from_utf8_lossy(data).into_owned())
    }

    #[test]
    fn a_name_split_across_two_runs_is_found_and_removed() {
        let source = docx("<w:r><w:t>Maitre Du</w:t></w:r><w:r><w:t>pont plaide</w:t></w:r>");
        assert_eq!(text_of(&source), "maitre dupont plaide");
        let out = redact(&source, "Dupont");
        assert_eq!(text_of(&out), "maitre plaide");
    }

    #[test]
    fn what_was_not_named_survives_untouched() {
        let source = docx("<w:r><w:t>Vu la requete de Dupont; greffier Martin</w:t></w:r>");
        let out = redact(&source, "dupont");
        assert_eq!(text_of(&out), "vu la requete de ; greffier martin");
    }

    #[test]
    fn a_redacted_part_stays_a_readable_package() {
        let source = docx("<w:r><w:t>Dupont</w:t></w:r>");
        let out = redact(&source, "Dupont");
        let archive = zip::ZipArchive::new(Cursor::new(&out)).unwrap();
        assert_eq!(archive.len(), 3);
        let package = OoxmlPackage::open(&out).unwrap();
        assert!(package.entry("word/document.xml").is_some());
    }

    #[test]
    fn a_name_carrying_an_escaped_character_is_removed_whole() {
        let source = docx("<w:r><w:t>Dupont &amp; Fils SARL</w:t></w:r>");
        assert_eq!(text_of(&source), "dupont & fils sarl");
        let out = redact(&source, "Dupont & Fils");
        assert_eq!(text_of(&out), "sarl");
    }

    #[test]
    fn a_run_left_ending_on_a_blank_keeps_it() {
        let source = docx("<w:r><w:t>situer les Dupont</w:t></w:r><w:r><w:t>. Du</w:t></w:r>");
        let out = redact(&source, "Dupont");
        let document = entry(&out, "word/document.xml").unwrap();
        assert!(
            document.contains(r#"<w:t xml:space="preserve">situer les </w:t>"#),
            "{document}"
        );
    }

    #[test]
    fn a_part_opening_on_a_byte_order_mark_is_spliced_where_its_text_is() {
        let document = "\u{feff}<?xml version=\"1.0\"?><w:document xmlns:w=\"urn:w\"><w:body><w:p><w:r><w:t>Maitre Dupont plaide</w:t></w:r></w:p></w:body></w:document>";
        let source = package(&[("word/document.xml", document.as_bytes().to_vec())]);
        let out = redact(&source, "Dupont");
        assert_eq!(text_of(&out), "maitre plaide");
    }

    #[test]
    fn a_name_in_a_link_an_alternative_text_or_a_field_code_goes_and_the_link_stays() {
        let document = r#"<w:document xmlns:w="urn:w" xmlns:wp="urn:wp" xmlns:r="urn:r"><w:body><w:p><w:hyperlink r:id="rId5" w:tooltip="Ecrire a Jean Dupont"><w:r><w:t>ecrire</w:t></w:r></w:hyperlink><w:fldSimple w:instr=" MERGEFIELD Dupont "/><wp:docPr id="1" name="Photo Dupont.jpg" descr="Jean Dupont a l audience"/><w:bookmarkStart w:id="0" w:name="_Toc1"/></w:p></w:body></w:document>"#;
        let rels = r#"<Relationships xmlns="urn:rel"><Relationship Id="rId5" Type="urn:hyperlink" Target="mailto:jean.dupont@example.org" TargetMode="External"/><Relationship Id="rId6" Type="urn:styles" Target="styles.xml"/></Relationships>"#;
        let source = package(&[
            ("word/document.xml", document.as_bytes().to_vec()),
            ("word/_rels/document.xml.rels", rels.as_bytes().to_vec()),
        ]);
        let outcome = redact_all(&source, &["Dupont"]);
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        let document = entry(&outcome.bytes, "word/document.xml").unwrap();
        for kept in [
            r#"w:tooltip="Ecrire a Jean ""#,
            r#"w:instr=" MERGEFIELD  ""#,
            r#"name="Photo .jpg""#,
            r#"descr="Jean  a l audience""#,
            r#"w:name="_Toc1""#,
        ] {
            assert!(document.contains(kept), "{kept} missing from {document}");
        }
        let rels = entry(&outcome.bytes, "word/_rels/document.xml.rels").unwrap();
        assert!(
            rels.contains(r#"Target="mailto:jean.@example.org""#),
            "{rels}"
        );
        assert!(rels.contains(r#"Target="styles.xml""#), "{rels}");
    }

    #[test]
    fn authors_are_emptied_and_the_thumbnail_and_signature_go_with_what_points_at_them() {
        let core = r#"<cp:coreProperties xmlns:cp="urn:cp" xmlns:dc="urn:dc"><dc:title>Bail</dc:title><dc:creator>Jean Martin</dc:creator><cp:lastModifiedBy>Paul Durand</cp:lastModifiedBy></cp:coreProperties>"#;
        let comments = r#"<w:comments xmlns:w="urn:w"><w:comment w:id="0" w:author="Paul Durand" w:initials="PD"><w:p><w:r><w:t>a revoir</w:t></w:r></w:p></w:comment></w:comments>"#;
        let types = r#"<Types xmlns="urn:ct"><Default Extension="jpeg" ContentType="image/jpeg"/><Override PartName="/docProps/core.xml" ContentType="core"/><Override PartName="/docProps/thumbnail.jpeg" ContentType="image/jpeg"/><Override PartName="/_xmlsignatures/sig1.xml" ContentType="sig"/></Types>"#;
        let rels = r#"<Relationships xmlns="urn:rel"><Relationship Id="rId2" Type="urn:thumbnail" Target="docProps/thumbnail.jpeg"/><Relationship Id="rId1" Type="urn:core" Target="docProps/core.xml"/><Relationship Id="rId3" Type="urn:origin" Target="_xmlsignatures/origin.sigs"/></Relationships>"#;
        let source = package(&[
            ("[Content_Types].xml", types.as_bytes().to_vec()),
            ("_rels/.rels", rels.as_bytes().to_vec()),
            ("docProps/core.xml", core.as_bytes().to_vec()),
            ("docProps/thumbnail.jpeg", b"\xFF\xD8\xFF".to_vec()),
            ("word/comments.xml", comments.as_bytes().to_vec()),
            ("_xmlsignatures/origin.sigs", Vec::new()),
            (
                "_xmlsignatures/sig1.xml",
                b"<Signature>CN=Paul Durand</Signature>".to_vec(),
            ),
        ]);
        let out = redact(&source, "introuvable");
        for gone in [
            "docProps/thumbnail.jpeg",
            "_xmlsignatures/origin.sigs",
            "_xmlsignatures/sig1.xml",
        ] {
            assert!(entry(&out, gone).is_none(), "{gone} kept");
        }
        let core = entry(&out, "docProps/core.xml").unwrap();
        assert!(core.contains("<dc:title>Bail</dc:title><dc:creator></dc:creator><cp:lastModifiedBy></cp:lastModifiedBy>"), "{core}");
        let comments = entry(&out, "word/comments.xml").unwrap();
        assert!(
            comments.contains(r#"w:author="" w:initials="""#),
            "{comments}"
        );
        assert!(comments.contains("a revoir"), "{comments}");
        let types = entry(&out, "[Content_Types].xml").unwrap();
        assert!(
            !types.contains("thumbnail") && !types.contains("sig1"),
            "{types}"
        );
        assert!(types.contains("/docProps/core.xml"), "{types}");
        let rels = entry(&out, "_rels/.rels").unwrap();
        assert!(
            !rels.contains("thumbnail") && !rels.contains("origin"),
            "{rels}"
        );
        assert!(rels.contains("docProps/core.xml"), "{rels}");
    }

    #[test]
    fn a_document_embedded_in_the_package_is_redacted_too() {
        let inner = docx("<w:r><w:t>Annexe signee par Dupont</w:t></w:r>");
        let document = r#"<w:document xmlns:w="urn:w"><w:body><w:p><w:r><w:t>Voir annexe de Dupont</w:t></w:r></w:p></w:body></w:document>"#;
        let source = package(&[
            ("word/document.xml", document.as_bytes().to_vec()),
            ("word/embeddings/Document1.docx", inner),
        ]);
        let outcome = redact_all(&source, &["Dupont"]);
        assert_eq!(outcome.found, 2);
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        let package = OoxmlPackage::open(&outcome.bytes).unwrap();
        let inner = package.entry("word/embeddings/Document1.docx").unwrap();
        assert_eq!(text_of(inner), "annexe signee par");
    }

    #[test]
    fn a_template_path_a_task_a_smart_tag_a_custom_author_and_a_picture_lose_their_names() {
        let document = r#"<w:document xmlns:w="urn:w"><w:body><w:p><w:smartTag w:element="PersonName"><w:smartTagPr><w:attr w:name="ProductID" w:val="Jean Martin"/></w:smartTagPr><w:r><w:t>Jean Martin</w:t></w:r></w:smartTag></w:p></w:body></w:document>"#;
        let settings = r#"<Relationships xmlns="urn:rel"><Relationship Id="rId1" Type="urn:r/attachedTemplate" Target="file:///C:\Profils\jmartin\Modeles\Lettre.dotx" TargetMode="External"/></Relationships>"#;
        let custom = r#"<Properties xmlns="urn:custom" xmlns:vt="urn:vt"><property pid="2" name="_AuthorEmail"><vt:lpwstr>jm@cabinet.fr</vt:lpwstr></property><property pid="3" name="Dossier"><vt:lpwstr>RG 21/00042</vt:lpwstr></property></Properties>"#;
        let tasks = r#"<t:Tasks xmlns:t="urn:t"><t:Task><t:Event><t:Attribution userId="S::jm@cabinet.fr" userProvider="AD" userName="Jean"/><t:SetTitle title="Relire avec Martin"/></t:Event></t:Task></t:Tasks>"#;
        let jpeg = b"\xFF\xD8\xFF\xE1\x00\x20Exif\x00\x00Artist: Jean Martin\x00\xFF\xD9";
        let source = package(&[
            ("word/document.xml", document.as_bytes().to_vec()),
            ("word/_rels/settings.xml.rels", settings.as_bytes().to_vec()),
            ("docProps/custom.xml", custom.as_bytes().to_vec()),
            (
                "word/documenttasks/documenttasks1.xml",
                tasks.as_bytes().to_vec(),
            ),
            ("word/media/image1.jpeg", jpeg.to_vec()),
        ]);
        let outcome = redact_all(&source, &["Martin"]);
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        let expect = |name: &str, kept: &[&str]| {
            let part = entry(&outcome.bytes, name).unwrap();
            for k in kept {
                assert!(part.contains(k), "{k} missing from {part}");
            }
        };
        expect("word/document.xml", &[r#"w:val="Jean ""#, ">Jean </w:t>"]);
        expect("word/_rels/settings.xml.rels", &[r#"Target="Lettre.dotx""#]);
        expect(
            "docProps/custom.xml",
            &["<vt:lpwstr></vt:lpwstr>", "RG 21/00042"],
        );
        expect(
            "word/documenttasks/documenttasks1.xml",
            &[
                r#"userId="" userProvider="AD" userName="""#,
                r#"title="Relire avec ""#,
            ],
        );
        let package = OoxmlPackage::open(&outcome.bytes).unwrap();
        let picture = package.entry("word/media/image1.jpeg").unwrap();
        assert_eq!(picture.len(), jpeg.len());
        assert!(picture.ends_with(b"Artist: Jean XXXXXX\x00\xFF\xD9"));
    }
}

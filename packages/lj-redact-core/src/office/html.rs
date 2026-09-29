//! A web page: the needles give way to their labels in the text, and leave
//! the markup around it, attributes, comments and styles, as many `X`.

use std::collections::BTreeSet;

use anyhow::Result;
use lj_office::html::HtmlPackage;

use super::{anchored, byte_patterns, erase_bytes, found_in, replacements, survivors};
use crate::labels::Labels;
use crate::Outcome;

/// Replace every occurrence of `needles` by its label, empty the title and
/// the author metadata, then verify on the bytes produced that none survives.
pub(super) fn redact(bytes: &[u8], needles: &[String], labels: &Labels) -> Result<Outcome> {
    let mut package = HtmlPackage::open(bytes)?;
    let feed = anchored(package.characters());
    let views = lj_office::view_positions(&package.views());
    let source_chars = feed.len();
    let (found, replacements) = replacements(&[feed], &views, needles, labels)?;

    let removed = package.replace(&replacements)?;
    package.clear_authors();
    package.rewrite_markup(&mut |data| {
        for needle in needles {
            erase_bytes(data, needle);
        }
    })?;
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

/// Needles a reader still finds in the text or in the markup.
pub(super) fn leaks(bytes: &[u8], needles: &[String]) -> Result<Vec<String>> {
    let package = HtmlPackage::open(bytes)?;
    let views = lj_office::view_positions(&package.views());
    let mut leaks = survivors(&anchored(package.characters()), &views, needles);
    let patterns: Vec<(usize, Vec<u8>)> = needles
        .iter()
        .enumerate()
        .flat_map(|(i, n)| byte_patterns(n).into_iter().map(move |(p, _)| (i, p)))
        .collect();
    let mut found = BTreeSet::new();
    for markup in package.markup_text() {
        found.extend(found_in(markup.as_bytes(), &patterns));
    }
    for i in found {
        if !leaks.contains(&needles[i]) {
            leaks.push(needles[i].clone());
        }
    }
    Ok(leaks)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_name_leaves_the_text_the_attributes_and_the_properties_of_a_web_page() {
        let page = concat!(
            "\u{FEFF}<html xmlns:o='urn:schemas-microsoft-com:office:office'><head><title>Dossier Martin</title>",
            "<!--[if gte mso 9]><xml><o:DocumentProperties><o:Author>Jeanne</o:Author></o:DocumentProperties></xml><![endif]--></head>",
            "<body><p>Vu <b>Mar</b>tin, <a href='mailto:martin@exemple.fr' title='M. Martin'>écrire</a>.</p>",
            "<table><tr><td>Témoin</td><td>MARTIN</td></tr></table></body></html>",
        );
        let (texts, labels) = super::super::erasing(&["Martin"]);
        let outcome = super::redact(page.as_bytes(), &texts, &labels).unwrap();
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        assert_eq!(outcome.found, 3);
        let produced = String::from_utf8(outcome.bytes.clone()).unwrap();
        assert!(!produced.to_lowercase().contains("martin"), "{produced}");
        assert!(!produced.contains("Jeanne"), "{produced}");
        assert!(produced.starts_with("\u{FEFF}<html"), "{produced}");
        let text = lj_office::open(&outcome.bytes).unwrap().text();
        assert_eq!(text, "Vu , écrire.\nTémoin\n\n");
    }
}

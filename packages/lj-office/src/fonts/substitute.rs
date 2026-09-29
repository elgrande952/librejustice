//! What an Office family is drawn with. LibreOffice is given the same table
//! through fontconfig, so that both lay a document out on one file.

/// Office family, lowercase, and the vendored family drawn in its place.
const TABLE: &[(&str, &str)] = &[
    ("calibri", "Carlito"),
    ("calibri light", "Carlito"),
    ("cambria", "Caladea"),
    ("cambria math", "Caladea"),
    ("arial", "Liberation Sans"),
    ("helvetica", "Liberation Sans"),
    ("arial narrow", "Liberation Sans Narrow"),
    ("times new roman", "Liberation Serif"),
    ("times", "Liberation Serif"),
    ("nimbus roman", "Liberation Serif"),
    ("nimbus roman no9 l", "Liberation Serif"),
    ("nimbus sans", "Liberation Sans"),
    ("nimbus sans l", "Liberation Sans"),
    ("nimbus sans narrow", "Liberation Sans Narrow"),
    ("nimbus mono ps", "Liberation Mono"),
    ("nimbus mono l", "Liberation Mono"),
    ("courier new", "Liberation Mono"),
    ("courier", "Liberation Mono"),
    ("consolas", "Liberation Mono"),
    ("georgia", "Gelasio"),
    ("segoe ui", "Selawik"),
    ("segoe ui light", "Selawik Light"),
    ("segoe ui semilight", "Selawik Semilight"),
    ("segoe ui semibold", "Selawik Semibold"),
    ("verdana", "DejaVu Sans"),
    ("trebuchet ms", "DejaVu Sans Condensed"),
    ("candara", "Carlito"),
    ("corbel", "Carlito"),
    ("constantia", "Caladea"),
    ("garamond", "EB Garamond"),
    ("century gothic", "URW Gothic"),
    ("book antiqua", "P052"),
    ("palatino linotype", "P052"),
    ("palatino", "P052"),
    ("bookman old style", "URW Bookman"),
    ("century schoolbook", "C059"),
    ("century", "C059"),
    ("aptos", "PT Sans"),
    ("aptos display", "Carlito"),
    ("aptos light", "Selawik Light"),
    ("aptos narrow", "Carlito"),
    ("aptos serif", "Gelasio"),
    ("aptos mono", "Liberation Mono"),
    ("marianne", "Noto Sans"),
    ("bitstream vera sans", "DejaVu Sans"),
    ("starsymbol", "OpenSymbol"),
    ("sans-serif", "Noto Sans"),
    ("monospace", "DejaVu Sans Mono"),
];

/// Windows script variants of a family, drawn as the family itself.
const SCRIPT_SUFFIXES: &[&str] = &[" cyr", " ce", " tur", " greek", " baltic", " (arabic)"];

/// The family a font name asks for, as LibreOffice reads it: the first of a
/// list, unquoted, without a Windows script suffix.
pub fn requested(name: &str) -> &str {
    let first = name.split([',', ';']).next().unwrap_or(name);
    let first = first.trim().trim_matches(['\'', '"']).trim();
    let lower = first.to_ascii_lowercase();
    SCRIPT_SUFFIXES
        .iter()
        .find(|suffix| lower.len() > suffix.len() && lower.ends_with(*suffix))
        .map_or(first, |suffix| &first[..first.len() - suffix.len()])
}

/// The vendored family `family` is drawn with, when the table names one.
pub fn substitute(family: &str) -> Option<&'static str> {
    let key = family.trim().to_lowercase();
    TABLE
        .iter()
        .find(|(from, _)| *from == key)
        .map(|(_, to)| *to)
}

/// Where a character its own face lacks is drawn, whatever family it started
/// from, as LibreOffice does.
pub const GLYPH_FALLBACK: &str = "DejaVu Sans";

#[cfg(test)]
mod tests {
    use super::requested;

    /// The names LibreOffice 26.2 draws as the family they start with.
    #[test]
    fn a_font_name_asks_for_its_first_family_without_script_suffix() {
        assert_eq!(requested("Arial;Arial"), "Arial");
        assert_eq!(requested("Calibri,Italic"), "Calibri");
        assert_eq!(requested("OpenSymbol, 'Arial Unicode MS'"), "OpenSymbol");
        assert_eq!(requested("Times New Roman CYR"), "Times New Roman");
        assert_eq!(requested("Arial CE"), "Arial");
        assert_eq!(requested("Arial (Arabic)"), "Arial");
        assert_eq!(requested("Arial Cyrillic"), "Arial Cyrillic");
        assert_eq!(requested("Arial Western"), "Arial Western");
    }
}

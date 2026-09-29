//! Curated bank of pure-procedure articles, keyed by text catalogue slug and
//! article key. Read side only: the stock stays faithful to the extraction, the
//! filters keep accepting these articles and the facets keep counting them.
//!
//! The guiding principles of the trial (CPC 4/5/9/12/14/15/16, 122) are
//! deliberately absent: a decision that cites them is arguing about them.

/// `(text slug, procedural article keys)`.
const PROCEDURAL: &[(&str, &[&str])] = &[
    (
        "code-de-procedure-civile",
        &[
            // costs and expenses
            "695", "696", "699", "700", // form and delivery of the judgment
            "450", "451", "452", "453", "454", "455", "456", "457", "458", "459", "462", "463",
            "464", "465", "466", "472", "473", // case management
            "446-1", "446-2", "446-3", "446-4", "763", "776", "778", "779", "780", "785", "786",
            "787", "788", "789", "790", "799", "800", "802", "803", "804", "805", "807", "808",
            "945-1", // provisional enforcement
            "514", "515", "517", "521", "524",
            // appeal tracks and form of the submissions
            "905", "905-1", "905-2", "906", "907", "908", "909", "910", "911", "912", "913", "914",
            "916", "954", "960", "961", "963", // discontinuance and lapse
            "384", "385", "394", "395", "399", // cassation procedure
            "627", "974", "978", "979", "982", "1009-1", "1010", "1011", "1014", "1015", "1018",
            "1022", "1026", "1031-1",
        ],
    ),
    (
        "code-de-procedure-penale",
        &[
            // form of the judgment and procedure of the pourvoi
            "567", "567-1-1", "568", "584", "585", "585-1", "586", "590", "591", "592", "593",
            "594", "598", "609-1", "612", "614", "615", "802",
        ],
    ),
    (
        "code-de-l-organisation-judiciaire",
        &["l131-6", "l131-6-1", "l431-3", "l431-4", "l432-1", "r431-5"],
    ),
    // costs, the administrative twin of article 700 CPC
    ("code-de-justice-administrative", &["l761-1"]),
    // legal aid
    (
        "loi-n-91-647-du-10-juillet-1991-relative-a-l-aide-juridique",
        &["20", "24", "37", "75"],
    ),
];

/// Whether the bank holds `(slug, num_key)`, both taken verbatim from
/// `legal_text.slug` and `legal_citation.ref_num_key`.
pub fn is_procedural(slug: &str, num_key: &str) -> bool {
    PROCEDURAL
        .iter()
        .find(|(text, _)| *text == slug)
        .is_some_and(|(_, nums)| nums.contains(&num_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bank_holds_the_costs_formula_and_not_the_grounds() {
        assert!(is_procedural("code-de-procedure-civile", "700"));
        assert!(is_procedural("code-de-justice-administrative", "l761-1"));
        assert!(!is_procedural("code-civil", "1240"));
        assert!(!is_procedural("code-de-procedure-civile", "9"));
        assert!(!is_procedural("code-de-procedure-civile", "122"));
        assert!(!is_procedural("code-civil", "700"));
    }

    #[test]
    fn every_key_is_written_as_the_catalogue_writes_it() {
        for (slug, nums) in PROCEDURAL {
            assert_eq!(*slug, lj_fold::slug(slug, '-'), "slug {slug}");
            for num in *nums {
                assert_eq!(*num, crate::article_key::article_key(num), "{slug} {num}");
            }
        }
    }
}

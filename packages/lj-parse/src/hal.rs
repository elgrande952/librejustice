//! Attribution of a HAL deposit to the cassation dockets it comments.
//!
//! Read from the deposits, not from the decisions: one request covers a
//! thousand deposits, the other direction cost one request per decision.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

/// What a HAL deposit contributes to a decision's commentary bundle.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HalDeposit {
    pub hal_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub abstract_text: Option<String>,
    pub keywords: Vec<String>,
    pub authors: Vec<String>,
    pub journal: Option<String>,
    /// Verbatim from the deposit: HAL takes them as free text, so `page_s`
    /// holds `407-422` as readily as `act. 843` or `1 et suivantes`.
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub pages: Option<String>,
    pub year: Option<i32>,
    pub uri: String,
    /// `file` when full text is deposited, `notice` otherwise.
    pub submit_type: String,
    pub domains: Vec<String>,
}

impl HalDeposit {
    fn haystack(&self) -> String {
        let mut parts = vec![self.title.clone()];
        parts.extend(self.subtitle.clone());
        parts.extend(self.abstract_text.clone());
        parts.extend(self.keywords.iter().cloned());
        parts.join(" \n ")
    }

    /// Of 145 accepted matches, 143 were classified `1.shs.droit` and the 2
    /// others were the campaign's only false positives.
    pub fn is_law(&self) -> bool {
        self.domains.iter().any(|d| d == "1.shs.droit")
    }
}

/// The cassation docket shape, `NN-NN.NNN`, as deposits write it. The leading
/// hyphen is required and `/` is excluded: a court of appeal roll number
/// carries the same digits (`21/16169` against `21-16.169`) and matched 5
/// deposits wrongly over the measurement campaign.
static DOCKET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{2})\s?[-\u{2011}]\s?(\d{2})\s?[.\-\u{00a0}\s]?\s?(\d{3})")
        .expect("docket regex")
});

/// Docket numbers cited by a deposit, normalised to `NN-NN.NNN`, in order of
/// appearance and deduplicated.
///
/// The `regex` crate has no look-around, so the digit boundaries the pattern
/// cannot express are checked on each match.
pub fn extract_dockets(deposit: &HalDeposit) -> Vec<String> {
    let hay = deposit.haystack();
    let mut out: Vec<String> = Vec::new();
    for c in DOCKET.captures_iter(&hay) {
        let m = c.get(0).expect("group 0");
        let before = hay[..m.start()].chars().next_back();
        let after = hay[m.end()..].chars().next();
        if before.is_some_and(|c| c.is_ascii_digit()) || after.is_some_and(|c| c.is_ascii_digit()) {
            continue;
        }
        let docket = format!("{}-{}.{}", &c[1], &c[2], &c[3]);
        if !out.contains(&docket) {
            out.push(docket);
        }
    }
    out
}

/// Law numbers cited in the deposit's title, `AAAA-NNN` or `AA-NNN`.
///
/// Title and subtitle only: 2 175 deposits name a law somewhere, 728 name it
/// in their title, and only those are about the law rather than merely citing
/// it. Same precision guard as the docket anchor.
pub fn extract_law_numbers(deposit: &HalDeposit) -> Vec<String> {
    let mut hay = deposit.title.clone();
    if let Some(sub) = &deposit.subtitle {
        hay.push_str(" \n ");
        hay.push_str(sub);
    }
    let mut out: Vec<String> = Vec::new();
    for c in LAW_NUMBER.captures_iter(&hay) {
        let num = c[1].to_string();
        if !out.contains(&num) {
            out.push(num);
        }
    }
    out
}

static LAW_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)loi\s+(?:n[°os]\s?)?((?:19|20)?\d{2}-\d{1,4})(?:\D|$)").expect("law regex")
});

fn tidy(field: &Option<String>) -> Option<String> {
    let s = field
        .as_ref()?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!s.is_empty()).then_some(s)
}

/// Assembles the outbound link of one deposit; no body is stored.
pub fn build_hal_source_fields(deposit: &HalDeposit) -> Value {
    json!({
        "commentaires": [{
            "kind": "note",
            "title": deposit.title,
            "author": (!deposit.authors.is_empty()).then(|| deposit.authors.join(", ")),
            "publisher": deposit.journal,
            "volume": tidy(&deposit.volume),
            "issue": tidy(&deposit.issue),
            "pages": tidy(&deposit.pages),
            "date": deposit.year.map(|y| y.to_string()),
            "url": deposit.uri,
            "access": if deposit.submit_type == "file" { "libre" } else { "abonnes" },
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deposit(field: &str, value: &str) -> HalDeposit {
        let mut d = HalDeposit {
            hal_id: "hal-1".into(),
            uri: "https://hal.science/hal-1".into(),
            submit_type: "notice".into(),
            domains: vec!["1.shs.droit".into()],
            ..Default::default()
        };
        match field {
            "title" => d.title = value.into(),
            "subtitle" => d.subtitle = Some(value.into()),
            "abstract" => d.abstract_text = Some(value.into()),
            _ => d.keywords = vec![value.into()],
        }
        d
    }

    #[test]
    fn separators_normalise_to_one_form() {
        for form in ["17-17.319", "17-17319", "17 - 17.319", "17-17 319"] {
            assert_eq!(
                extract_dockets(&deposit("title", &format!("Note sous {form}"))),
                vec!["17-17.319".to_string()],
                "{form} should normalise"
            );
        }
    }

    #[test]
    fn court_of_appeal_roll_number_is_not_a_docket() {
        assert!(extract_dockets(&deposit("title", "CA Paris, 21/16169")).is_empty());
    }

    #[test]
    fn a_longer_number_is_not_sliced_into_a_docket() {
        assert!(extract_dockets(&deposit("title", "n° 17-17.3190")).is_empty());
        assert!(extract_dockets(&deposit("title", "n° 917-17.319")).is_empty());
    }

    #[test]
    fn all_four_fields_are_read() {
        for field in ["title", "subtitle", "abstract", "keywords"] {
            assert_eq!(
                extract_dockets(&deposit(field, "pourvoi numéro 12-27.299")),
                vec!["12-27.299".to_string()],
                "{field} should be read"
            );
        }
    }

    #[test]
    fn several_dockets_come_back_deduplicated() {
        let mut d = deposit("title", "Cass. com., n° 20-15.488 et n° 20-10.147");
        d.abstract_text = Some("retour sur 20-15.488".into());
        assert_eq!(extract_dockets(&d), vec!["20-15.488", "20-10.147"]);
    }

    #[test]
    fn a_deposit_naming_nothing_yields_nothing() {
        assert!(extract_dockets(&deposit("title", "Le droit des contrats en 2026")).is_empty());
    }

    #[test]
    fn bundle_separates_deposit_from_reference() {
        let mut open = deposit("title", "Note sous Civ. 1re");
        open.submit_type = "file".into();
        open.journal = Some("RTDCiv".into());
        open.authors = vec!["P. Jourdain".into()];
        open.year = Some(2018);
        let v = build_hal_source_fields(&open);
        let c = &v["commentaires"][0];
        assert_eq!(c["access"], "libre");
        assert_eq!(c["publisher"], "RTDCiv");
        assert_eq!(c["author"], "P. Jourdain");
        assert_eq!(c["date"], "2018");
        assert_eq!(c["kind"], "note");

        let notice = deposit("title", "Obs.");
        assert_eq!(
            build_hal_source_fields(&notice)["commentaires"][0]["access"],
            "abonnes"
        );
    }

    #[test]
    fn the_reference_travels_verbatim_and_absent_stays_absent() {
        let mut d = deposit("title", "Note");
        d.volume = Some("  vol.  134 ".into());
        d.pages = Some("act. 843".into());
        let c = &build_hal_source_fields(&d)["commentaires"][0];
        assert_eq!(c["volume"], "vol. 134");
        assert_eq!(c["pages"], "act. 843");
        assert!(c["issue"].is_null());
    }

    #[test]
    fn domain_filter_drops_non_law() {
        let mut astro = deposit("title", "n° 12-27.299");
        astro.domains = vec!["1.phys.astr".into()];
        assert!(!astro.is_law());
        assert!(deposit("title", "x").is_law());
    }

    #[test]
    fn law_number_is_read_from_the_title_only() {
        let mut d = deposit("title", "Les aspects médico-sociaux de la loi n° 2014-873");
        assert_eq!(extract_law_numbers(&d), vec!["2014-873".to_string()]);
        d.abstract_text = Some("la loi n° 99-123 est citée".into());
        assert_eq!(extract_law_numbers(&d), vec!["2014-873".to_string()]);
    }

    #[test]
    fn law_number_accepts_the_old_two_digit_form() {
        assert_eq!(
            extract_law_numbers(&deposit("title", "Retour sur la loi n° 52-1322")),
            vec!["52-1322".to_string()]
        );
        assert_eq!(
            extract_law_numbers(&deposit("subtitle", "commentaire de la loi 98-389")),
            vec!["98-389".to_string()]
        );
    }

    #[test]
    fn a_title_naming_no_law_yields_nothing() {
        assert!(extract_law_numbers(&deposit("title", "Note sous Civ. 1re, 17-17.319")).is_empty());
    }
}

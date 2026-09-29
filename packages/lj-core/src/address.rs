//! How a decision is designated when the corpus may not hold it yet.
//!
//! An absent component WIDENS the designation instead of breaking it: a
//! cassation names the court and the date of the judgment it attacks, never its
//! docket, so the address goes out without one and the parties settle it later.
//! Whether an address designates one decision and only one is never its own
//! claim; the resolution decides, against the corpus of the moment.

/// The address of a decision that is cited, attacked or remitted.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CaseAddress {
    /// A `jurisdiction` referential code (`ta_rennes`, `ca_nancy`, `cc`).
    pub court: Option<String>,
    /// As written (`19/00207`, `20LY00096`): folding to a key is the join's
    /// business, never storage's.
    pub docket: Option<String>,
    /// ISO `YYYY-MM-DD`.
    pub date: Option<String>,
}

impl CaseAddress {
    pub fn new(court: &str, docket: &str, date: &str) -> Self {
        Self {
            court: non_empty(court),
            docket: non_empty(&written_docket(docket)),
            date: non_empty(date),
        }
    }

    pub fn court_and_date(court: &str, date: &str) -> Self {
        Self {
            court: non_empty(court),
            docket: None,
            date: non_empty(date),
        }
    }

    pub fn designates(&self) -> bool {
        self.court.is_some() || self.docket.is_some() || self.date.is_some()
    }

    /// Same target at the grain both addresses reach: a component absent on
    /// either side does not tell them apart. Serves the bench, whose oracle
    /// carries the docket where the text path does not.
    pub fn same_target_as(&self, other: &Self) -> bool {
        let agrees = |a: &Option<String>, b: &Option<String>| match (a, b) {
            (Some(x), Some(y)) => x == y,
            _ => true,
        };
        self.designates()
            && other.designates()
            && agrees(&self.court, &other.court)
            && agrees(&self.docket, &other.docket)
            && agrees(&self.date, &other.date)
    }
}

/// The number as the source writes it, minus what the sentence lends it: the
/// closing full stop the capture swallows, and the space a typesetter slipped
/// after the slash. Separators stay, they are how the number reads.
fn written_docket(raw: &str) -> String {
    raw.trim_matches(|c: char| !c.is_alphanumeric())
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sentence_punctuation_is_not_part_of_the_number() {
        let a = |d: &str| CaseAddress::new("tj_paris", d, "").docket.unwrap();
        assert_eq!(a("09/03244."), "09/03244");
        assert_eq!(a("09/ 29"), "09/29");
        assert_eq!(a("(21/04532)"), "21/04532");
        assert_eq!(a("C-561/19"), "C-561/19");
        assert_eq!(CaseAddress::new("cc", " . ", "").docket, None);
    }

    #[test]
    fn an_empty_component_is_not_written() {
        let a = CaseAddress::new("ta_rennes", "", "2025-10-15");
        assert_eq!(a.court.as_deref(), Some("ta_rennes"));
        assert_eq!(a.docket, None);
        assert!(a.designates());
        assert!(!CaseAddress::new("", " ", "").designates());
    }

    #[test]
    fn an_absent_docket_does_not_tell_targets_apart() {
        let registry = CaseAddress::new("ca_nancy", "19/00207", "2020-12-08");
        let text = CaseAddress::court_and_date("ca_nancy", "2020-12-08");
        assert!(registry.same_target_as(&text));
        assert!(text.same_target_as(&registry));
    }

    #[test]
    fn a_component_present_on_both_sides_tells_them_apart() {
        let a = CaseAddress::new("ca_nancy", "19/00207", "2020-12-08");
        assert!(!a.same_target_as(&CaseAddress::new("ca_nancy", "19/00208", "2020-12-08")));
        assert!(!a.same_target_as(&CaseAddress::new("ca_paris", "19/00207", "2020-12-08")));
    }

    #[test]
    fn the_empty_address_resembles_nothing() {
        let empty = CaseAddress::default();
        assert!(!empty.same_target_as(&empty));
        assert!(!empty.same_target_as(&CaseAddress::new("cc", "20-22.085", "2022-03-24")));
    }
}

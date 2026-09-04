//! Chronology steps the corpus does not hold. A `decision_links` row whose
//! target stays pending (`target_decision_id IS NULL`) leaves nothing but its
//! key, written by the ingest in the `canonical_ref` grammar: it is read back
//! here into a named step, which the page renders without a link.

use crate::referential::Referential;

/// One chronology step read back from a pending key.
pub struct PendingStep {
    pub label: String,
    pub date: String,
    pub docket: Option<String>,
}

/// `None` when the court component matches no referential unit, which is what
/// a name the extraction mis-cut looks like ("tribunal administratif de nimes
/// auquel la requete"), and when the key is `caa|<rg>|<date>`, whose court
/// lives in the request-number prefix and nowhere else in the key.
pub fn pending_step(target_ref: &str, refs: &Referential) -> Option<PendingStep> {
    let (court, docket, date) = match *target_ref.split('|').collect::<Vec<_>>().as_slice() {
        ["ca-parties", court, date] => (court, None, date),
        ["caa", _, _] => return None,
        [court, docket, date] => (court, Some(docket), date),
        [_, court, docket, date] => (court, Some(docket), date),
        _ => return None,
    };
    Some(PendingStep {
        label: refs.court_by_key(court)?.to_string(),
        date: date.to_string(),
        docket: docket.map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_store::repository::JurisdictionRow;

    fn refs() -> Referential {
        let rows = [
            (
                "ca_aix_en_provence",
                "ca_aix_provence",
                "CA",
                "Cour d'appel d'Aix-en-Provence",
            ),
            (
                "tj_le_havre",
                "tj76351",
                "TJ",
                "Tribunal judiciaire du Havre",
            ),
            (
                "tcom_paris",
                "tcom7501",
                "TCOM",
                "Tribunal de commerce de Paris",
            ),
            ("ta_lyon", "ta_lyon", "TA", "Tribunal administratif de Lyon"),
            ("cc", "cc", "CC", "Cour de cassation"),
        ];
        Referential::new(
            Vec::new(),
            rows.iter()
                .map(
                    |(code, source_code, jurisdiction_type, label)| JurisdictionRow {
                        code: (*code).to_string(),
                        source_code: (*source_code).to_string(),
                        jurisdiction_type: (*jurisdiction_type).to_string(),
                        city: None,
                        label: (*label).to_string(),
                    },
                )
                .collect(),
            Vec::new(),
        )
    }

    #[test]
    fn every_key_family_names_its_court() {
        let refs = refs();
        let step = |key: &str| pending_step(key, &refs).map(|s| (s.label, s.date, s.docket));
        assert_eq!(
            step("tribunal administratif de lyon|0907255|2012-03-27"),
            Some((
                "Tribunal administratif de Lyon".to_string(),
                "2012-03-27".to_string(),
                Some("0907255".to_string())
            ))
        );
        assert_eq!(
            step("ca-parties|ca_aix_provence|2021-03-03"),
            Some((
                "Cour d'appel d'Aix-en-Provence".to_string(),
                "2021-03-03".to_string(),
                None
            ))
        );
        assert_eq!(
            step("tj|tj76351|24 01880|2024-06-30").map(|s| s.0),
            Some("Tribunal judiciaire du Havre".to_string())
        );
        assert_eq!(
            step("tcom|7501|2023064061|2024-01-19").map(|s| s.0),
            Some("Tribunal de commerce de Paris".to_string())
        );
        assert_eq!(
            step("cc|04-15.329|2006-11-16").map(|s| s.0),
            Some("Cour de cassation".to_string())
        );
    }

    #[test]
    fn a_court_the_referential_does_not_hold_yields_no_step() {
        let refs = refs();
        let absent = [
            "tribunal administratif de nimes auquel la requete|1900612|2021-05-04",
            "caa|02ly02109|2003-03-31",
            "tj|tj99999|24 01880|2024-06-30",
        ];
        for key in absent {
            assert!(pending_step(key, &refs).is_none(), "{key}");
        }
    }
}

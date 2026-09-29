//! French dates as tender engines and court decisions write them.

use jiff::civil::Date;

const MOIS: [&str; 12] = [
    "janvier",
    "fevrier",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "decembre",
];

/// A month name, full (« septembre ») or abbreviated (« sept. », « Sept »), folded.
fn mois(nom: &str) -> Option<i8> {
    let n = lj_fold::fold(nom);
    let n = n.trim_end_matches('.');
    if n.len() < 3 {
        return None;
    }
    MOIS.iter()
        .position(|m| m.starts_with(n) || (n.len() >= 4 && n.starts_with(&m[..4.min(m.len())])))
        .map(|i| i as i8 + 1)
}

/// A date in the formats the tender engines serve: `12/09/2026`, `12/09/26`,
/// `2026-09-12` or an ISO datetime, `1er août 2026`, `01 Sept. 2026`. `None`
/// when unreadable.
pub fn parse_date_fr(s: &str) -> Option<Date> {
    let s = s.trim();
    if let Ok(d) = s.get(..10).unwrap_or_default().parse::<Date>() {
        return Some(d);
    }
    let slash: Vec<&str> = s.split('/').collect();
    if let [j, m, a] = slash[..] {
        let (j, m) = (j.trim().parse::<i8>().ok()?, m.trim().parse::<i8>().ok()?);
        let mut a = a.trim().parse::<i16>().ok()?;
        if a < 100 {
            a += 2000;
        }
        return Date::new(a, m, j).ok();
    }
    let mots: Vec<&str> = s.split_whitespace().collect();
    if let [j, m, a] = mots[..] {
        let j = j.trim_end_matches("er").parse::<i8>().ok()?;
        let m = mois(m)?;
        let a = a.parse::<i16>().ok()?;
        return Date::new(a, m, j).ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_the_engines_serve() {
        let d = |s: &str| parse_date_fr(s).map(|d| d.to_string());
        assert_eq!(d("12/09/2026"), Some("2026-09-12".into()));
        assert_eq!(d("12/09/26"), Some("2026-09-12".into()));
        assert_eq!(d("2026-09-12"), Some("2026-09-12".into()));
        assert_eq!(d("2026-09-12T17:00:00+02:00"), Some("2026-09-12".into()));
        assert_eq!(d("1er août 2026"), Some("2026-08-01".into()));
        assert_eq!(d("01 Sept. 2026"), Some("2026-09-01".into()));
        assert_eq!(d("12 septembre 2026"), Some("2026-09-12".into()));
        assert_eq!(d("31/02/2026"), None);
        assert_eq!(d("bientôt"), None);
        assert_eq!(d(&"\u{2014}".repeat(13)), None);
        assert_eq!(d("é"), None);
    }
}

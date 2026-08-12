//! Validation des dates de recherche (frontière HTTP + MCP).

/// Échec de validation d'une date `dateFrom`/`dateTo` : format ET plage
/// (`1678-01-01` … `2262-01-01`) validés AVANT le SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DateError {
    /// Format non parsable — `error` est le détail verbatim, recopié dans
    /// `msg`/`ctx.error`.
    Parse(&'static str),
    /// Antérieure à la borne basse représentable par Tantivy (1678-01-01).
    TooEarly,
    /// Postérieure à la borne haute (2262-01-01).
    TooLate,
}

/// Bornes basse/haute des dates : la fenêtre que Tantivy indexe en i64
/// nanosecondes.
pub(crate) const DATE_GE: &str = "1678-01-01";
pub(crate) const DATE_LE: &str = "2262-01-01";

/// Parse une date `dateFrom`/`dateTo` : strict `YYYY-MM-DD` puis plage
/// [1678-01-01, 2262-01-01].
pub(crate) fn parse_search_date(s: &str) -> std::result::Result<chrono::NaiveDate, DateError> {
    let d = parse_iso_strict(s).map_err(DateError::Parse)?;
    // `ge`/`le` sont des constantes valides → `unwrap` sûr.
    let ge = chrono::NaiveDate::from_ymd_opt(1678, 1, 1).unwrap();
    let le = chrono::NaiveDate::from_ymd_opt(2262, 1, 1).unwrap();
    if d < ge {
        Err(DateError::TooEarly)
    } else if d > le {
        Err(DateError::TooLate)
    } else {
        Ok(d)
    }
}

/// `YYYY-MM-DD` strict : année 4 chiffres, `-`, mois 2 chiffres, `-`, jour
/// 2 chiffres, longueur exacte.
fn parse_iso_strict(s: &str) -> std::result::Result<chrono::NaiveDate, &'static str> {
    let b = s.as_bytes();
    if b.len() < 4 {
        return Err("input is too short");
    }
    if b[..4].iter().any(|c| !c.is_ascii_digit()) {
        return Err("invalid character in year");
    }
    if b.len() < 5 || b[4] != b'-' {
        return Err("invalid date separator, expected `-`");
    }
    if b.len() < 7 || !b[5].is_ascii_digit() || !b[6].is_ascii_digit() {
        return Err("input is too short");
    }
    if b.len() < 8 || b[7] != b'-' {
        return Err("invalid date separator, expected `-`");
    }
    if b.len() < 10 || !b[8].is_ascii_digit() || !b[9].is_ascii_digit() {
        return Err("input is too short");
    }
    if b.len() != 10 {
        return Err("trailing characters");
    }
    let year: i32 = s[0..4].parse().unwrap();
    let month: u32 = s[5..7].parse().unwrap();
    let day: u32 = s[8..10].parse().unwrap();
    if !(1..=12).contains(&month) {
        return Err("month value is outside expected range of 1-12");
    }
    chrono::NaiveDate::from_ymd_opt(year, month, day).ok_or("day value is outside expected range")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_search_date_parse_errors_match_speedate() {
        use DateError::Parse;
        assert_eq!(
            parse_search_date("not-a-date"),
            Err(Parse("invalid character in year"))
        );
        assert_eq!(
            parse_search_date("2024-13-01"),
            Err(Parse("month value is outside expected range of 1-12"))
        );
        assert_eq!(
            parse_search_date("2024-99-99"),
            Err(Parse("month value is outside expected range of 1-12"))
        );
        assert_eq!(
            parse_search_date("0000-00-00"),
            Err(Parse("month value is outside expected range of 1-12"))
        );
        assert_eq!(
            parse_search_date("2024-02-30"),
            Err(Parse("day value is outside expected range"))
        );
        assert_eq!(
            parse_search_date("999999999-01-01"),
            Err(Parse("invalid date separator, expected `-`"))
        );
        assert_eq!(
            parse_search_date("2024-1-1"),
            Err(Parse("input is too short"))
        );
    }

    #[test]
    fn parse_search_date_range_and_valid() {
        assert_eq!(parse_search_date("1600-01-01"), Err(DateError::TooEarly));
        assert_eq!(parse_search_date("2300-01-01"), Err(DateError::TooLate));
        assert!(parse_search_date("2024-01-01").is_ok());
        assert!(parse_search_date("1678-01-01").is_ok());
        assert!(parse_search_date("2262-01-01").is_ok());
        assert_eq!(parse_search_date("1677-12-31"), Err(DateError::TooEarly));
        assert_eq!(parse_search_date("2262-01-02"), Err(DateError::TooLate));
    }
}

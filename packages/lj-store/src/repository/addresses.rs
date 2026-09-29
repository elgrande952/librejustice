//! Transitional filling of the three address columns, on the four tables that
//! designate a target. The corpus still carries the old `target_ref` key: this
//! module converts it row by row, for as long as the re-extraction takes to go
//! over the corpus. It leaves with the column.
//!
//! The cursor is the `ctid`: three of the four tables have no single-column
//! primary key, and the pass reads the corpus once, in physical order.

use chrono::NaiveDate;

use super::DecisionRepository;
use crate::error::Result;

/// The four tables that designate a pending target.
pub const ADDRESSED_TABLES: [&str; 4] = [
    "decision_links",
    "case_citation",
    "text_case_citation",
    "decision_renvoi",
];

/// A row to convert: its `ctid`, the old key, and the address of the target
/// when that target is already resolved, which wins over the key.
pub struct LegacyRow {
    pub ctid: String,
    pub key: Option<String>,
    pub target: Option<(Option<String>, Option<String>, Option<NaiveDate>)>,
}

impl DecisionRepository<'_> {
    /// The rows without an address, in physical order, after `ctid`. An
    /// address the gazetteer cannot name still fills the docket or the date, so
    /// the emptiness of all three columns is what says a row is left to do: an
    /// UPDATE moves the row further down the heap, where a narrower predicate
    /// would meet it again and rewrite it for ever. `decision_renvoi` already
    /// carries its date, which is why the date is out of its predicate.
    pub async fn legacy_addresses(
        &self,
        table: &str,
        after: &str,
        lot: i64,
    ) -> Result<Vec<LegacyRow>> {
        let empty = if table == "decision_renvoi" {
            "c.target_court IS NULL AND c.target_docket IS NULL"
        } else {
            "c.target_court IS NULL AND c.target_docket IS NULL AND c.target_date IS NULL"
        };
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT c.ctid::text, c.target_ref, d.jurisdiction_code, \
                            d.docket_numbers[1], d.date_lecture \
                     FROM {table} c \
                     LEFT JOIN decisions d ON d.id = c.target_decision_id \
                     WHERE c.ctid > $1::text::tid AND {empty} \
                     ORDER BY c.ctid LIMIT $2"
                ),
                &[&after, &lot],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| LegacyRow {
                ctid: r.get(0),
                key: r.get(1),
                target: r
                    .get::<_, Option<String>>(2)
                    .map(|code| (Some(code), r.get(3), r.get(4))),
            })
            .collect())
    }

    /// Writes the converted addresses. `decision_renvoi` already carries its
    /// date, which the renvoi key does not hold, so the key never erases it.
    pub async fn set_addresses(
        &self,
        table: &str,
        ctids: &[String],
        courts: &[Option<String>],
        dockets: &[Option<String>],
        dates: &[Option<NaiveDate>],
    ) -> Result<u64> {
        let date = if table == "decision_renvoi" {
            "coalesce(c.target_date, s.j)"
        } else {
            "s.j"
        };
        let n = self
            .conn
            .execute(
                &format!(
                    "UPDATE {table} c \
                     SET target_court = s.c, target_docket = s.n, target_date = {date} \
                     FROM unnest($1::text[], $2::text[], $3::text[], $4::date[]) \
                          AS s(t, c, n, j) \
                     WHERE c.ctid = s.t::tid"
                ),
                &[&ctids, &courts, &dockets, &dates],
            )
            .await?;
        Ok(n)
    }
}

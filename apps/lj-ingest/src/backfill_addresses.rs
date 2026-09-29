//! Fills the three address columns from the old `target_ref` key, for as long
//! as the re-extraction takes to go over the corpus. Two sources, in this
//! order: the target when it is already resolved, which carries its own
//! coordinates; the old key otherwise, read back through the same door as the
//! extraction (`lj_extract::chrono::court_from_label`).
//!
//! Single-pass command, replayable: it only touches a row whose address is
//! still empty.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_core::address::CaseAddress;
use lj_extract::chrono::{court_from_label, ChronoSnapshot};

use crate::config::Settings;

const LOT: i64 = 20_000;

pub async fn run() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let client = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let jurisdictions = lj_store::repository::DecisionRepository::new(&client)
        .load_jurisdictions()
        .await?;
    let locations: HashMap<String, String> = jurisdictions
        .iter()
        .map(|j| (normalized(&j.source_code), j.code.clone()))
        .collect();
    let snap = ChronoSnapshot::new(
        jurisdictions
            .into_iter()
            .filter_map(|j| Some((j.code, j.jurisdiction_type, j.city?))),
    );

    let repo = lj_store::repository::DecisionRepository::new(&client);
    for table in lj_store::repository::ADDRESSED_TABLES {
        let (from_target, from_key) = fill(&repo, table, &snap, &locations).await?;
        tracing::info!(
            table,
            from_target,
            from_key,
            "addresses filled, from the resolved target then from the old key"
        );
    }
    Ok(())
}

/// One physical pass over the table. A resolved row takes its address from its
/// target, which is the fact; a pending row takes it from its old key.
async fn fill(
    repo: &lj_store::repository::DecisionRepository<'_>,
    table: &str,
    snap: &ChronoSnapshot,
    locations: &HashMap<String, String>,
) -> Result<(u64, u64)> {
    let (mut from_target, mut from_key) = (0u64, 0u64);
    let mut after = String::from("(0,0)");
    loop {
        let rows = repo.legacy_addresses(table, &after, LOT).await?;
        let Some(last) = rows.last() else {
            return Ok((from_target, from_key));
        };
        after = last.ctid.clone();
        let mut ctids: Vec<String> = Vec::with_capacity(rows.len());
        let mut courts: Vec<Option<String>> = Vec::with_capacity(rows.len());
        let mut dockets: Vec<Option<String>> = Vec::with_capacity(rows.len());
        let mut dates: Vec<Option<NaiveDate>> = Vec::with_capacity(rows.len());
        for row in &rows {
            let (court, docket, date) = match &row.target {
                Some((court, docket, date)) => {
                    from_target += 1;
                    (court.clone(), docket.clone(), *date)
                }
                None => {
                    let address =
                        address_of_key(row.key.as_deref().unwrap_or_default(), snap, locations);
                    if !address.designates() {
                        continue;
                    }
                    from_key += 1;
                    let date = address
                        .date
                        .and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok());
                    (address.court, address.docket, date)
                }
            };
            ctids.push(row.ctid.clone());
            courts.push(court);
            dockets.push(docket);
            dates.push(date);
        }
        if !ctids.is_empty() {
            repo.set_addresses(table, &ctids, &courts, &dockets, &dates)
                .await?;
        }
    }
}

/// The address an old key carries, whatever its family. A court nothing names
/// stays empty: an incomplete address widens, where an unresolvable string
/// would have said nothing at all.
fn address_of_key(
    key: &str,
    snap: &ChronoSnapshot,
    locations: &HashMap<String, String>,
) -> CaseAddress {
    let parts: Vec<&str> = key.split('|').collect();
    let date = |s: &str| s.len() == 10 && s.as_bytes()[4] == b'-';
    match parts.as_slice() {
        [famille, loc, rg, jour] if matches!(*famille, "ca" | "tj" | "tcom") => CaseAddress::new(
            locations.get(&normalized(loc)).map_or("", String::as_str),
            rg,
            jour,
        ),
        ["ca-parties", court, jour] => CaseAddress::court_and_date(court, jour),
        ["caa", rg, jour] if date(jour) => CaseAddress::new("", rg, jour),
        [famille, code, num] if matches!(*famille, "rg" | "af") => CaseAddress::new(code, num, ""),
        ["cc", pourvoi, jour] if date(jour) => CaseAddress::new("cc", pourvoi, jour),
        [nom, rg, jour] if date(jour) => {
            CaseAddress::new(&court_from_label(nom, snap).unwrap_or_default(), rg, jour)
        }
        [cour, num] => CaseAddress::new(cour, num, ""),
        _ => CaseAddress::default(),
    }
}

/// The graphy under which a key carries a location code: lowercase, and
/// punctuation reduced to a space, as `normalize_component` used to write it.
fn normalized(s: &str) -> String {
    let folded: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> ChronoSnapshot {
        ChronoSnapshot::new(vec![
            ("ta_rennes".into(), "TA".into(), "Rennes".into()),
            ("caa_lyon".into(), "CAA".into(), "Lyon".into()),
            ("ta_guadeloupe".into(), "TA".into(), "Guadeloupe".into()),
        ])
    }

    fn locations() -> std::collections::HashMap<String, String> {
        [("tj 75056", "tj_paris"), ("ca toulouse", "ca_toulouse")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// The seven families, plus the two chronology keys, land on the address
    /// the re-extraction will write.
    #[test]
    fn every_old_family_reads_as_an_address() {
        let (s, l) = (snap(), locations());
        let a = |k: &str| {
            let a = address_of_key(k, &s, &l);
            let c = |o: Option<String>| o.unwrap_or_default();
            format!("{}|{}|{}", c(a.court), c(a.docket), c(a.date))
        };
        assert_eq!(a("cc|1823954"), "cc|1823954|");
        assert_eq!(a("af|ta_rennes|2504655"), "ta_rennes|2504655|");
        assert_eq!(a("rg|ca_paris|21/04532"), "ca_paris|21/04532|");
        assert_eq!(a("rg||8703"), "|8703|");
        assert_eq!(a("cjue|c-561/19"), "cjue|c-561/19|");
        assert_eq!(a("caa|20ly00096|2021-12-14"), "|20ly00096|2021-12-14");
        assert_eq!(a("cc|04-15.329|2006-11-16"), "cc|04-15.329|2006-11-16");
        assert_eq!(
            a("ca-parties|ca_aix_en_provence|2021-03-03"),
            "ca_aix_en_provence||2021-03-03"
        );
        assert_eq!(
            a("tj|tj 75056|17 59899|2018-04-05"),
            "tj_paris|1759899|2018-04-05"
        );
        assert_eq!(
            a("tribunal administratif de rennes|2504655|2025-10-15"),
            "ta_rennes|2504655|2025-10-15"
        );
        assert_eq!(
            a("tribunal administratif de la guadeloupe|1900468|2019-07-04"),
            "ta_guadeloupe|1900468|2019-07-04"
        );
        assert_eq!(a("poitier|1900468|2019-07-04"), "|1900468|2019-07-04");
    }
}

//! What a text says of its own applicability, read in its articles: the
//! overseas counters that extend or set aside articles of the same code, and
//! the Alsace-Moselle clauses that set a text or an article aside. Events, in
//! memory, for the compiler; nothing is stored.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;

use lj_norm::applicability::{Event, EventKind, UNDATED_PLACEHOLDER};
use lj_norm::compteurs::{
    compteur_has_table, parse_compteur, parse_compteur_table, self_scope, CounterEvent, SelfGrain,
};
use lj_store::repository::VersionForCompile;

use super::scope_compteurs::{resolve_event_targets, VersionsByKey};

/// Territories a counter names, and the place each one is.
const COUNTER_PLACES: &[(&str, &str)] = &[
    ("Polynésie française", "geo:PF"),
    ("Nouvelle-Calédonie", "geo:NC"),
    ("Wallis-et-Futuna", "geo:WF"),
    ("Terres australes et antarctiques françaises", "geo:TF"),
    ("Saint-Martin", "geo:MF"),
    ("Saint-Barthélemy", "geo:BL"),
    ("Saint-Pierre-et-Miquelon", "geo:PM"),
    ("Mayotte", "geo:YT"),
];

const ALSACE: &str = "geo:FR-ALS";

static RE_PIVOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)applicables?").unwrap());
static RE_ALSACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Bas-Rhin|Haut-Rhin").unwrap());

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EventStats {
    pub counters: usize,
    pub foreign_owner: usize,
    pub dangling_targets: usize,
    pub alsace_curation: usize,
}

fn to_jiff(d: NaiveDate) -> jiff::civil::Date {
    use chrono::Datelike;
    jiff::civil::Date::new(d.year() as i16, d.month() as i8, d.day() as i8).expect("valid date")
}

/// A version whose text spoke during its window: the counter as it stood then,
/// on the wording then in force. Markers and stillborns carry no window.
fn speaks(v: &VersionForCompile) -> bool {
    matches!(v.status.as_str(), "VIGUEUR" | "MODIFIE")
        && to_jiff(v.date_debut) < UNDATED_PLACEHOLDER
        && v.texte.is_some()
}

fn event(
    kind: EventKind,
    place: &str,
    version_start: NaiveDate,
    v: &VersionForCompile,
    source: String,
    frozen: bool,
) -> Event {
    Event {
        kind,
        place: place.to_string(),
        version_start: Some(to_jiff(version_start)),
        frozen,
        window_start: to_jiff(v.date_debut),
        window_end: v.date_fin.map(to_jiff),
        source,
    }
}

/// Events of one text, keyed by the article they name.
pub fn events_of_text(
    versions: &[VersionForCompile],
) -> (BTreeMap<String, Vec<Event>>, EventStats) {
    let mut by_key: VersionsByKey = BTreeMap::new();
    for v in versions {
        by_key
            .entry(v.num_key.clone())
            .or_default()
            .push((v.date_debut, v.date_fin));
    }
    let mut out: BTreeMap<String, Vec<Event>> = BTreeMap::new();
    let mut stats = EventStats::default();
    let push = |out: &mut BTreeMap<String, Vec<Event>>, key: String, ev: Event| {
        out.entry(key).or_default().push(ev);
    };

    for v in versions.iter().filter(|v| speaks(v)) {
        let texte = v.texte.as_deref().expect("a speaking version has a text");
        if !RE_PIVOT.is_match(texte) {
            continue;
        }
        let ident = format!("{}#{}", v.text_uid, v.num_key);

        let named_places: Vec<&str> = COUNTER_PLACES
            .iter()
            .filter(|(name, _)| texte.contains(name))
            .map(|(_, place)| *place)
            .collect();
        if !named_places.is_empty() {
            let mut events = parse_compteur(texte);
            if compteur_has_table(texte) {
                if let Some(table) = parse_compteur_table(texte) {
                    events.extend(table.events);
                }
            }
            if !events.is_empty() {
                stats.counters += 1;
                for place in named_places {
                    counter_events(&events, place, v, &ident, &by_key, &mut stats, &mut out);
                }
            }
        }

        if !RE_ALSACE.is_match(texte) {
            continue;
        }
        let self_clause =
            self_scope(texte).filter(|(_, scopes, _)| scopes.iter().any(|s| s == ALSACE));
        if let Some((grain, _, negated)) = self_clause {
            match (grain, negated) {
                (SelfGrain::Text, true) => {
                    for (key, vlist) in &by_key {
                        let Some((start, _)) = vlist
                            .iter()
                            .filter(|(d, f)| {
                                *d <= v.date_debut && f.is_none_or(|f| f > v.date_debut)
                            })
                            .max()
                        else {
                            continue;
                        };
                        push(
                            &mut out,
                            key.clone(),
                            event(
                                EventKind::Exclude,
                                ALSACE,
                                *start,
                                v,
                                format!("clause {ident}"),
                                false,
                            ),
                        );
                    }
                }
                (SelfGrain::Article, true) => push(
                    &mut out,
                    v.num_key.clone(),
                    event(
                        EventKind::Exclude,
                        ALSACE,
                        v.date_debut,
                        v,
                        format!("clause {ident}"),
                        false,
                    ),
                ),
                (_, false) | (SelfGrain::Division, true) => stats.alsace_curation += 1,
            }
            continue;
        }
        let negated: Vec<CounterEvent> = parse_compteur(texte)
            .into_iter()
            .filter(|e| e.scopes.iter().any(|s| s == ALSACE))
            .collect();
        if negated.iter().any(|e| !e.negated) {
            stats.alsace_curation += 1;
        }
        for ev in negated
            .into_iter()
            .filter(|e| e.negated && !e.foreign_owner)
        {
            let resolved = resolve_event_targets(&ev, &by_key, v.date_debut);
            stats.dangling_targets += resolved.miss_anchor + resolved.miss_current;
            for (key, version_start) in resolved.keys {
                push(
                    &mut out,
                    key,
                    event(
                        EventKind::Exclude,
                        ALSACE,
                        version_start,
                        v,
                        format!("clause {ident}"),
                        false,
                    ),
                );
            }
        }
    }
    (out, stats)
}

/// The events of one counter for one named place. A target pinned by an
/// explicit wording anchor outranks the same target caught by an umbrella
/// range of the same counter.
fn counter_events(
    events: &[CounterEvent],
    place: &str,
    v: &VersionForCompile,
    ident: &str,
    by_key: &VersionsByKey,
    stats: &mut EventStats,
    out: &mut BTreeMap<String, Vec<Event>>,
) {
    let named: BTreeSet<&str> = events
        .iter()
        .flat_map(|e| e.scopes.iter().map(String::as_str))
        .collect();
    let inherit_ok = named.len() == 1 && named.contains(place);
    let mut local: Vec<(String, Event, bool)> = Vec::new();
    for ev in events {
        if ev.foreign_owner {
            stats.foreign_owner += 1;
            continue;
        }
        let explicit = ev.scopes.iter().any(|s| s == place);
        if !(explicit || ev.scopes.is_empty() && inherit_ok) {
            continue;
        }
        let resolved = resolve_event_targets(ev, by_key, v.date_debut);
        stats.dangling_targets += resolved.miss_anchor + resolved.miss_current;
        let mut source = format!("compteur {ident}");
        let prose: Vec<String> = ev
            .exceptions
            .iter()
            .map(|e| e.prose.clone())
            .chain(ev.reserves.iter().cloned())
            .collect();
        if !prose.is_empty() {
            let detail: String = prose.join(" ; ").chars().take(400).collect();
            source.push_str(" ; ");
            source.push_str(&detail);
        }
        let kind = if ev.negated {
            EventKind::Exclude
        } else {
            EventKind::Extend
        };
        let frozen = ev.redaction_date.is_some();
        for (key, version_start) in resolved.keys {
            local.push((
                key,
                event(kind, place, version_start, v, source.clone(), frozen),
                frozen,
            ));
        }
    }
    let pinned: BTreeSet<String> = local
        .iter()
        .filter(|(_, _, anchored)| *anchored)
        .map(|(k, _, _)| k.clone())
        .collect();
    for (key, ev, anchored) in local {
        if !anchored && pinned.contains(&key) {
            continue;
        }
        out.entry(key).or_default().push(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(
        num_key: &str,
        start: &str,
        end: Option<&str>,
        status: &str,
        texte: &str,
    ) -> VersionForCompile {
        VersionForCompile {
            text_uid: "LEGITEXT000000000001".to_string(),
            num: num_key.to_uppercase(),
            num_key: num_key.to_string(),
            date_debut: start.parse().unwrap(),
            date_fin: end.map(|d| d.parse().unwrap()),
            status: status.to_string(),
            source: "legifrance".to_string(),
            translation: "officiel".to_string(),
            content_checksum: 0,
            texte: Some(texte.to_string()),
            texte_original: None,
            lang_original: None,
        }
    }

    #[test]
    fn a_self_clause_on_another_territory_does_not_hide_the_alsace_exclusion() {
        let texte = "Le présent décret est applicable en Polynésie française.\n\
                     Les articles L. 218-3 et L. 218-4 ne sont pas applicables dans les départements du Bas-Rhin, du Haut-Rhin et de la Moselle.";
        let versions = [
            version("l218-1", "2010-01-01", None, "VIGUEUR", texte),
            version("l218-3", "2010-01-01", None, "VIGUEUR", "Texte."),
            version("l218-4", "2010-01-01", None, "VIGUEUR", "Texte."),
        ];
        let (events, _) = events_of_text(&versions);
        let excluded: Vec<&String> = events
            .iter()
            .filter(|(_, evs)| {
                evs.iter()
                    .any(|e| e.kind == EventKind::Exclude && e.place == ALSACE)
            })
            .map(|(k, _)| k)
            .collect();
        assert_eq!(excluded, vec!["l218-3", "l218-4"]);
    }

    #[test]
    fn every_speaking_version_of_the_carrier_opens_its_own_window() {
        let counter = "Les articles L. 100-1 à L. 100-2 sont applicables en Nouvelle-Calédonie.";
        let versions = [
            version(
                "l612-1",
                "2015-01-01",
                Some("2019-12-31"),
                "MODIFIE",
                counter,
            ),
            version("l612-1", "2020-01-01", None, "VIGUEUR", counter),
            version("l100-1", "2010-01-01", None, "VIGUEUR", "Texte."),
            version("l100-2", "2010-01-01", None, "VIGUEUR", "Texte."),
        ];
        let (events, stats) = events_of_text(&versions);
        let nc: Vec<&Event> = events["l100-1"]
            .iter()
            .filter(|e| e.place == "geo:NC")
            .collect();
        assert_eq!(nc.len(), 2);
        assert_eq!(nc[0].window_start, "2015-01-01".parse().unwrap());
        assert_eq!(nc[0].window_end, Some("2019-12-31".parse().unwrap()));
        assert_eq!(nc[1].window_start, "2020-01-01".parse().unwrap());
        assert_eq!(nc[1].window_end, None);
        assert_eq!(stats.counters, 2);
    }
}

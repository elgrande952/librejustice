//! Filtres SQL (pre-filter facettes scalaires + filtres tableau tantivy/SQL).
//!
//! Les filtres référentiels (solution, voie, office, domaine, instance,
//! publication, juridiction fine) ciblent les colonnes `_uid`/`jurisdiction_code`
//! de `decisions`. Côté jambe ANN (`decision_chunks`, qui ne porte pas ces
//! colonnes), ils passent par un `EXISTS` sur `decisions`.

use serde_json::Value;
use tokio_postgres::types::ToSql;

use lj_dtos::{Domain, JurisdictionType, SearchRequest};

/// Param dynamique : on accumule `Box<dyn ToSql>` et on génère les `$n`.
/// `Send` requis pour que les futures de handler axum restent `Send`.
pub(crate) type Params = Vec<Box<dyn ToSql + Sync + Send>>;

pub(crate) fn as_refs(params: &Params) -> Vec<&(dyn ToSql + Sync)> {
    params
        .iter()
        .map(|b| b.as_ref() as &(dyn ToSql + Sync))
        .collect()
}

pub(crate) fn jt_codes(types: &[JurisdictionType]) -> Vec<String> {
    types.iter().map(|t| jt_to_str(*t).to_string()).collect()
}

/// Un tableau de filtre vide n'est pas un filtre : `= ANY('{}')` ne matche rien,
/// donc `{"jurisdiction_type": []}` rendrait zéro résultat sans erreur.
/// [`nonempty`] pose la même règle pour les filtres de chaînes.
pub(crate) fn present<T>(v: &Option<Vec<T>>) -> Option<&[T]> {
    v.as_deref().filter(|s| !s.is_empty())
}

pub(crate) fn jt_to_str(t: JurisdictionType) -> &'static str {
    match t {
        JurisdictionType::Ta => "TA",
        JurisdictionType::Caa => "CAA",
        JurisdictionType::Ce => "CE",
        JurisdictionType::Constit => "CONSTIT",
        JurisdictionType::Tc => "TC",
        JurisdictionType::Cc => "CC",
        JurisdictionType::Ca => "CA",
        JurisdictionType::Tj => "TJ",
        JurisdictionType::Tcom => "TCOM",
        JurisdictionType::Cedh => "CEDH",
        JurisdictionType::Cjue => "CJUE",
        JurisdictionType::Cnda => "CNDA",
        JurisdictionType::Cnil => "CNIL",
    }
}

/// Sérialise un enum schéma vers son code SCREAMING (via serde_json).
fn enum_code<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Uids complets `facet_value` d'une sélection d'enums : `prefix:<CODE>`.
pub(crate) fn enum_uids<T: serde::Serialize>(prefix: &str, vs: &[T]) -> Vec<String> {
    vs.iter()
        .map(|v| format!("{prefix}:{}", enum_code(v)))
        .collect()
}

/// Expansion domaine : une racine sélectionnée matche elle-même et toutes ses
/// feuilles (codes préfixés `<RACINE>_` dans `Domain::ALL`) ; une racine sans
/// feuille (FISCAL, EUROPEEN…) ou une feuille matche elle seule.
pub(crate) fn legal_domain_uids(selected: &[Domain]) -> Vec<String> {
    let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for d in selected {
        let code = enum_code(d);
        let leaf_prefix = format!("{code}_");
        out.insert(format!("legal_domain:{code}"));
        for other in Domain::ALL {
            let oc = enum_code(&other);
            if oc.starts_with(&leaf_prefix) {
                out.insert(format!("legal_domain:{oc}"));
            }
        }
    }
    out.into_iter().collect()
}

/// Ajoute une clause `sql` (avec `$?` substitué par le prochain placeholder) +
/// son param.
fn add_clause(
    clauses: &mut Vec<String>,
    params: &mut Params,
    idx: &mut usize,
    sql: &str,
    p: Box<dyn ToSql + Sync + Send>,
) {
    clauses.push(sql.replace("$?", &format!("${idx}")));
    params.push(p);
    *idx += 1;
}

/// Table ciblée par le filtre : `decisions` (alias `d`, toutes colonnes
/// locales) côté jambes BM25, ou `decision_chunks` (alias `c`) côté ANN — les
/// colonnes référentielles `_uid`/`jurisdiction_code` n'existent que sur
/// `decisions`, elles y sont atteintes par `EXISTS`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterTable {
    Decisions,
    Chunks,
}

/// Construit le filtre facettes scalaire d'une jambe. Renvoie le fragment SQL
/// (préfixé ` AND `), en remplissant `params` à partir de `next_idx`.
pub(crate) fn build_facet_filter(
    req: &SearchRequest,
    next_idx: &mut usize,
    params: &mut Params,
    arrays: ArrayFilters,
    table: FilterTable,
) -> String {
    let alias = match table {
        FilterTable::Decisions => "d",
        FilterTable::Chunks => "c",
    };
    let mut clauses: Vec<String> = Vec::new();
    // Colonnes présentes sur les deux tables (dénormalisées côté chunks).
    if let Some(types) = present(&req.jurisdiction_type) {
        add_clause(
            &mut clauses,
            params,
            next_idx,
            &format!("{alias}.jurisdiction_type = ANY($?)"),
            Box::new(jt_codes(types)),
        );
    }
    if arrays == ArrayFilters::AsSql {
        for (col, values) in array_filter_values(req) {
            add_clause(
                &mut clauses,
                params,
                next_idx,
                &format!("{alias}.{col} && $?::text[]"),
                Box::new(values),
            );
        }
        // `publication_codes` est dénormalisée sur les deux tables : clause directe
        // sur l'alias local, aucun `EXISTS` vers `decisions`. Côté jambes BM25
        // (`Tantivy`), le filtre est composé DANS le `@@@` par
        // [`build_array_filter_queries`].
        if let Some(v) = nonempty(&req.publication) {
            if let Some(clause) = publication_clause(alias, &v) {
                clauses.push(clause);
            }
        }
    }
    if let Some(d) = &req.date_from {
        add_clause(
            &mut clauses,
            params,
            next_idx,
            &format!("{alias}.date_lecture >= $?::text::date"),
            Box::new(d.clone()),
        );
    }
    if let Some(d) = &req.date_to {
        add_clause(
            &mut clauses,
            params,
            next_idx,
            &format!("{alias}.date_lecture <= $?::text::date"),
            Box::new(d.clone()),
        );
    }
    // Colonnes référentielles, sur `decisions` uniquement.
    let mut ref_clauses: Vec<String> = Vec::new();
    let ref_alias = match table {
        FilterTable::Decisions => "d",
        FilterTable::Chunks => "fd",
    };
    if let Some(v) = present(&req.solution) {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.solution_uid = ANY($?)"),
            Box::new(enum_uids("solution", v)),
        );
    }
    if let Some(v) = present(&req.procedure) {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.procedure_uid = ANY($?)"),
            Box::new(enum_uids("procedure", v)),
        );
    }
    if let Some(v) = present(&req.office) {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.office_uid = ANY($?)"),
            Box::new(enum_uids("office", v)),
        );
    }
    if let Some(v) = present(&req.legal_domain) {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.legal_domain_uid = ANY($?)"),
            Box::new(legal_domain_uids(v)),
        );
    }
    if let Some(v) = nonempty(&req.jurisdiction_code) {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.jurisdiction_code = ANY($?)"),
            Box::new(v),
        );
    }
    // Seat: an overlap of paths, the GIN serving the SQL leg. The BM25 legs
    // pass it as a `term_set` inside the `@@@`, where the multi-valued fast
    // field answers without touching the table.
    if arrays == ArrayFilters::AsSql {
        if let Some(v) = nonempty(&req.seat) {
            add_clause(
                &mut ref_clauses,
                params,
                next_idx,
                &format!("{ref_alias}.seat_paths && $?::text[]"),
                Box::new(v),
            );
        }
    }
    if !ref_clauses.is_empty() {
        match table {
            FilterTable::Decisions => clauses.extend(ref_clauses),
            FilterTable::Chunks => clauses.push(format!(
                "EXISTS (SELECT 1 FROM decisions fd WHERE fd.id = c.decision_id AND {})",
                ref_clauses.join(" AND ")
            )),
        }
    }
    if clauses.is_empty() {
        String::new()
    } else {
        format!(" AND {}", clauses.join(" AND "))
    }
}

/// Sort des filtres tableau (`legal_instrument`, `legal_article` composite)
/// dans `build_facet_filter`.
///
/// - Jambes BM25 (`@@@` sur `decisions_bm25`) : `Tantivy` — le term/term_set
///   passe par l'index inversé. Les filtres sont composés dans la requête via
///   [`compose_tantivy_query`].
/// - Jambe ANN (pas de `@@@`) : `AsSql` — `&&` évalué par le prefilter
///   VectorChord sur le heap.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrayFilters {
    AsSql,
    Tantivy,
}

/// `(colonne, valeurs)` des filtres tableau actifs (sémantique `&&` = overlap).
fn array_filter_values(req: &SearchRequest) -> Vec<(&'static str, Vec<String>)> {
    [
        ("legal_instruments", nonempty(&req.legal_instrument)),
        ("legal_article_composite", nonempty(&req.legal_article)),
    ]
    .into_iter()
    .filter_map(|(col, v)| v.map(|values| (col, values)))
    .collect()
}

/// Codes source des valeurs de publication sélectionnées, dédupliqués. Ce sont
/// des constantes du référentiel compilé, jamais de l'entrée utilisateur (la
/// frontière la valide en 422) : inlinées, pas de paramètre.
fn publication_selected_codes(keys: &[String]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for key in keys {
        for code in lj_core::publication::publication_codes_of(key) {
            if !out.contains(code) {
                out.push(code);
            }
        }
    }
    out
}

/// Clause SQL de publication : appartenance multiple, l'union des codes des
/// valeurs cochées. `None` si la sélection est vide.
fn publication_clause(alias: &str, keys: &[String]) -> Option<String> {
    let codes = publication_selected_codes(keys);
    if codes.is_empty() {
        return None;
    }
    let list: Vec<String> = codes.iter().map(|c| format!("'{c}'")).collect();
    Some(format!(
        "{alias}.publication_codes && ARRAY[{}]",
        list.join(",")
    ))
}

/// Même filtre en requête tantivy, pour les jambes BM25. `publication_codes` est
/// un champ `pdb.literal` de `decisions_bm25` : termes exacts, pas de
/// lowercasing (`r` != `R`, `C+` intact).
fn publication_tantivy(keys: &[String]) -> Option<String> {
    let terms: Vec<String> = publication_selected_codes(keys)
        .iter()
        .map(|c| format!("paradedb.term('publication_codes', '{c}')"))
        .collect();
    match terms.len() {
        0 => None,
        1 => terms.into_iter().next(),
        _ => Some(format!(
            "paradedb.term_set(terms => ARRAY[{}])",
            terms.join(", ")
        )),
    }
}

/// Requêtes tantivy des filtres tableau, `paradedb.term` (1 valeur) ou
/// `paradedb.term_set` (overlap = ANY), plus le filtre de publication
/// ([`publication_tantivy`]). À AND-er dans le `@@@` via [`compose_tantivy_query`].
pub(crate) fn build_array_filter_queries(
    req: &SearchRequest,
    next_idx: &mut usize,
    params: &mut Params,
) -> Vec<String> {
    let mut out = Vec::new();
    for (col, values) in array_filter_values(req) {
        let terms: Vec<String> = values
            .into_iter()
            .map(|v| {
                params.push(Box::new(v) as Box<dyn ToSql + Sync + Send>);
                let expr = format!("paradedb.term('{col}', ${next_idx}::text)");
                *next_idx += 1;
                expr
            })
            .collect();
        out.push(if terms.len() == 1 {
            terms.into_iter().next().expect("non vide")
        } else {
            format!("paradedb.term_set(terms => ARRAY[{}])", terms.join(", "))
        });
    }
    if let Some(v) = nonempty(&req.seat) {
        out.push(term_any("seat_paths", &v, next_idx, params));
    }
    if let Some(v) = &req.publication {
        if let Some(expr) = publication_tantivy(v) {
            out.push(expr);
        }
    }
    out
}

/// `paradedb.term` d'une valeur, `term_set` de plusieurs.
pub(crate) fn term_any(
    field: &str,
    values: &[String],
    next_idx: &mut usize,
    params: &mut Params,
) -> String {
    let terms: Vec<String> = values
        .iter()
        .map(|v| {
            params.push(Box::new(v.clone()) as Box<dyn ToSql + Sync + Send>);
            let expr = format!("paradedb.term('{field}', ${next_idx}::text)");
            *next_idx += 1;
            expr
        })
        .collect();
    if terms.len() == 1 {
        terms.into_iter().next().expect("non vide")
    } else {
        format!("paradedb.term_set(terms => ARRAY[{}])", terms.join(", "))
    }
}

/// Compose l'expression `@@@` d'une jambe BM25 : la requête primaire seule,
/// ou `paradedb.boolean(must => …)` avec les filtres tableau in-index.
pub(crate) fn compose_tantivy_query(primary: &str, filters: Vec<String>) -> String {
    if filters.is_empty() {
        primary.to_string()
    } else {
        format!(
            "paradedb.boolean(must => ARRAY[{primary}, {}])",
            filters.join(", ")
        )
    }
}

pub(crate) fn nonempty(v: &Option<Vec<String>>) -> Option<Vec<String>> {
    let filtered: Vec<String> = v
        .as_ref()
        .map(|x| x.iter().filter(|s| !s.is_empty()).cloned().collect())
        .unwrap_or_default();
    if filtered.is_empty() {
        None
    } else {
        Some(filtered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domaine_root_expands_to_leaves() {
        let uids = legal_domain_uids(&[Domain::Civil]);
        assert!(uids.contains(&"legal_domain:CIVIL".to_string()));
        assert!(uids.contains(&"legal_domain:CIVIL_DROIT_LOCATIF".to_string()));
        assert_eq!(uids.len(), 16);
        assert!(uids.iter().all(|u| u.starts_with("legal_domain:CIVIL")));
        assert_eq!(
            legal_domain_uids(&[Domain::Fiscal]),
            vec!["legal_domain:FISCAL".to_string()]
        );
        assert_eq!(
            legal_domain_uids(&[Domain::SocialDroitTravail]),
            vec!["legal_domain:SOCIAL_DROIT_TRAVAIL".to_string()]
        );
    }

    #[test]
    fn empty_filter_arrays_are_no_filter_at_all() {
        let req: SearchRequest = serde_json::from_value(serde_json::json!({
            "query": "x",
            "jurisdictionType": [],
            "solution": [],
            "procedure": [],
            "office": [],
            "legalDomain": [],
            "publication": [],
            "jurisdictionCode": [],
        }))
        .unwrap();
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let frag = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::AsSql,
            FilterTable::Decisions,
        );
        assert!(frag.is_empty(), "{frag}");
        assert!(params.is_empty());
    }

    /// The seat filters by an overlap of paths: the BM25 legs pass it inside
    /// the `@@@`, the ANN leg through the array, never both.
    #[test]
    fn seat_filter_recouvre_les_chemins_selon_la_jambe() {
        let req: SearchRequest = serde_json::from_value(serde_json::json!({
            "query": "x",
            "seat": ["CC.SOCIALE", "CA.PARIS.C1"],
        }))
        .unwrap();

        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let bm25 = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::Tantivy,
            FilterTable::Decisions,
        );
        assert_eq!(bm25, "", "le chemin part dans le `@@@`, pas en SQL");
        let index_query = build_array_filter_queries(&req, &mut idx, &mut params);
        assert_eq!(index_query.len(), 1);
        assert!(
            index_query[0].contains("paradedb.term_set"),
            "{index_query:?}"
        );
        assert!(
            index_query[0].contains("paradedb.term('seat_paths', $1::text)"),
            "{index_query:?}"
        );
        assert_eq!(params.len(), 2);

        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let ann = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::AsSql,
            FilterTable::Chunks,
        );
        assert!(ann.contains("fd.seat_paths && $1::text[]"), "{ann}");
        assert!(ann.contains("EXISTS (SELECT 1 FROM decisions fd"), "{ann}");
        assert_eq!(params.len(), 1);
    }

    /// Appartenance multiple : la sélection est l'union des codes cochés, sans
    /// exclusion d'un palier par un autre.
    #[test]
    fn publication_filter_routes_by_leg_on_source_codes() {
        let req: SearchRequest = serde_json::from_value(serde_json::json!({
            "query": "x",
            "publication": ["PUBLIE_RAPPORT", "LETTRE_CHAMBRE"],
        }))
        .unwrap();
        // Jambes BM25 : composé DANS le `@@@` (index inversé), absent du fragment SQL.
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let frag = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::Tantivy,
            FilterTable::Decisions,
        );
        assert!(!frag.contains("publication_codes"));
        let queries = build_array_filter_queries(&req, &mut idx, &mut params);
        assert_eq!(queries.len(), 1);
        assert_eq!(
            queries[0],
            "paradedb.term_set(terms => ARRAY[paradedb.term('publication_codes', 'r'), \
             paradedb.term('publication_codes', 'l')])"
        );
        assert!(params.is_empty());

        // Jambe ANN : clause SQL directe sur la colonne dénormalisée du chunk,
        // aucun `EXISTS` vers `decisions`.
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let frag = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::AsSql,
            FilterTable::Chunks,
        );
        assert!(frag.contains("c.publication_codes && ARRAY['r','l']"));
        assert!(!frag.contains("EXISTS"));
        assert!(params.is_empty());
    }

    #[test]
    fn chunk_filter_reaches_uid_columns_via_exists() {
        let req: SearchRequest = serde_json::from_value(serde_json::json!({
            "query": "x",
            "solution": ["REJET"],
            "publication": ["PUBLIE_BULLETIN"],
            "jurisdictionCode": ["tj76351"],
        }))
        .unwrap();
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let frag = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::AsSql,
            FilterTable::Chunks,
        );
        assert!(frag.contains("EXISTS (SELECT 1 FROM decisions fd WHERE fd.id = c.decision_id"));
        assert!(frag.contains("fd.solution_uid = ANY($1)"));
        assert!(frag.contains("fd.jurisdiction_code = ANY($2)"));
        // La publication ne passe plus par l'`EXISTS` : `publication_codes` est
        // dénormalisée sur le chunk.
        assert!(frag.contains("c.publication_codes && ARRAY['b']"));
        assert_eq!(params.len(), 2);

        // Même requête côté decisions : clauses directes sur `d`.
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let frag = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::Tantivy,
            FilterTable::Decisions,
        );
        assert!(!frag.contains("EXISTS"));
        assert!(frag.contains("d.solution_uid = ANY($1)"));
    }
}

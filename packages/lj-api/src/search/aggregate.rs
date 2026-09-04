//! Total et facettes exacts, comptés dans `decisions_bm25` par un seul
//! `paradedb.aggregate` plutôt que sur la page ramenée, qui n'en donnait qu'une
//! borne inférieure : 26 juridictions sur 376 sur `responsabilite
//! contractuelle`, les 350 absentes étant des filtres valides injoignables.
//!
//! Piège de l'appel : il ne prend qu'un prédicat, sans clause SQL. Un filtre qui
//! ne s'y traduit pas fait porter les décomptes sur un ensemble plus large que
//! les résultats rendus.

use deadpool_postgres::Client;
use serde_json::Value;
use tokio_postgres::types::ToSql;

use lj_dtos::SearchRequest;

use super::filters::{
    as_refs, build_array_filter_queries, enum_uids, jt_codes, legal_domain_uids, nonempty, present,
    term_any, Params,
};
use super::hydrate::FacetCounters;

/// Aggregation tree. The seat axis is flat: a path carries its own hierarchy,
/// so one `terms` renders the 2,362 nodes of the corpus where the
/// type/court/chamber nest asked for three.
///
/// Sizes set with margin on the real vocabulary (2,362 paths, 43 domains, 14
/// solutions, 9 offices, 13 procedures, 32 codes): `sum_other_doc_count` is 0
/// everywhere, citations excepted, truncated by nature. The seat cap is global
/// now, no longer nested under a court: at 300 it would truncate the tree with
/// no error and no log, measured at 684 ms at 4,000.
fn agg() -> &'static str {
    r#"{
  "seat": {"terms": {"field": "seat_paths", "size": 4000}},
  "dom": {"terms": {"field": "legal_domain_uid", "size": 100}},
  "sol": {"terms": {"field": "solution_uid", "size": 50}},
  "off": {"terms": {"field": "office_uid", "size": 30}},
  "pro": {"terms": {"field": "procedure_uid", "size": 30}},
  "pub": {"terms": {"field": "publication_codes", "size": 50}},
  "jour": {"date_histogram": {"field": "date_lecture", "fixed_interval": "1d", "min_doc_count": 1}},
  "inst": {"terms": {"field": "legal_instruments", "size": 30}},
  "art": {"terms": {"field": "legal_article_composite", "size": 300}},
  "total": {"value_count": {"field": "id"}}
}"#
}

/// Plage de dates en `daterange`, `date_to` inclusive comme le filtre SQL.
fn date_range(req: &SearchRequest, next_idx: &mut usize, params: &mut Params) -> Option<String> {
    let literal = match (req.date_from.as_deref(), req.date_to.as_deref()) {
        (None, None) => return None,
        (Some(f), Some(t)) => format!("[{f},{t}]"),
        (Some(f), None) => format!("[{f},)"),
        (None, Some(t)) => format!("(,{t}]"),
    };
    params.push(Box::new(literal) as Box<dyn ToSql + Sync + Send>);
    let expr = format!("paradedb.range('date_lecture', ${next_idx}::text::daterange)");
    *next_idx += 1;
    Some(expr)
}

/// La question, en union des deux jambes lexicales. Sans le titre, une décision
/// que seul son nom d'usage désigne serait servie par la jambe titre et comptée
/// zéro : la facette afficherait 0 sous un résultat visible.
fn question(req: &SearchRequest, next_idx: &mut usize, params: &mut Params) -> Option<String> {
    if req.query.trim().is_empty() {
        return None;
    }
    let boolean = super::query::is_boolean_query(&req.query);
    let body = if boolean {
        super::query::translate_boolean(&req.query)
    } else {
        super::query::phrase_combo_parse(&req.query)
    };
    let bi = *next_idx;
    params.push(Box::new(body) as Box<dyn ToSql + Sync + Send>);
    let title_expr = if boolean {
        params
            .push(Box::new(super::query::translate_boolean(&req.query))
                as Box<dyn ToSql + Sync + Send>);
        format!(
            "paradedb.parse_with_field('search_title', ${}::text)",
            bi + 1
        )
    } else {
        params.push(Box::new(req.query.clone()) as Box<dyn ToSql + Sync + Send>);
        format!("paradedb.match('search_title', ${}::text)", bi + 1)
    };
    *next_idx += 2;
    Some(format!(
        "paradedb.boolean(should => ARRAY[\
         paradedb.parse_with_field('full_text', ${bi}::text), {title_expr}], \
         minimum_should_match => 1)"
    ))
}

struct PredicateParts {
    question: Option<String>,
    filters: Vec<String>,
}

/// Prédicat tantivy de la requête entière. Sans le `must_not`, l'agrégat
/// compterait le fonds mort avec le vif. Le parcours s'en sert aussi pour lister
/// sa page, l'index sachant trier et tronquer ce qu'il vient de compter.
pub(super) fn build_predicate(
    req: &SearchRequest,
    next_idx: &mut usize,
    params: &mut Params,
) -> String {
    compose(&predicate_parts(req, next_idx, params))
}

fn compose(parts: &PredicateParts) -> String {
    let mut must: Vec<String> = parts.question.iter().cloned().collect();
    must.extend(parts.filters.iter().cloned());
    if must.is_empty() {
        must.push("paradedb.all()".to_string());
    }
    format!(
        "paradedb.boolean(must => ARRAY[{}], must_not => ARRAY[paradedb.exists('deleted_at')])",
        must.join(", ")
    )
}

fn compose_complement(parts: &PredicateParts, ids: &[i64]) -> Option<String> {
    let question = parts.question.as_ref()?;
    let terms: Vec<String> = ids
        .iter()
        .map(|id| format!("paradedb.term('id', {id}::bigint)"))
        .collect();
    let mut must = vec![format!(
        "paradedb.term_set(terms => ARRAY[{}])",
        terms.join(", ")
    )];
    must.extend(parts.filters.iter().cloned());
    Some(format!(
        "paradedb.boolean(must => ARRAY[{}], \
         must_not => ARRAY[{question}, paradedb.exists('deleted_at')])",
        must.join(", ")
    ))
}

fn predicate_parts(
    req: &SearchRequest,
    next_idx: &mut usize,
    params: &mut Params,
) -> PredicateParts {
    let question = question(req, next_idx, params);
    let mut must: Vec<String> = Vec::new();
    if let Some(types) = present(&req.jurisdiction_type) {
        must.push(term_any(
            "jurisdiction_type",
            &jt_codes(types),
            next_idx,
            params,
        ));
    }
    for (field, uids) in [
        (
            "solution_uid",
            present(&req.solution).map(|v| enum_uids("solution", v)),
        ),
        (
            "procedure_uid",
            present(&req.procedure).map(|v| enum_uids("procedure", v)),
        ),
        (
            "office_uid",
            present(&req.office).map(|v| enum_uids("office", v)),
        ),
        (
            "legal_domain_uid",
            present(&req.legal_domain).map(legal_domain_uids),
        ),
    ] {
        if let Some(uids) = uids {
            must.push(term_any(field, &uids, next_idx, params));
        }
    }
    if let Some(codes) = nonempty(&req.jurisdiction_code) {
        must.push(term_any("jurisdiction_code", &codes, next_idx, params));
    }
    if let Some(expr) = date_range(req, next_idx, params) {
        must.push(expr);
    }
    must.extend(build_array_filter_queries(req, next_idx, params));
    PredicateParts {
        question,
        filters: must,
    }
}

/// Total et compteurs de l'ensemble apparié, citations comprises depuis qu'elles
/// sortent du même balayage. Elles seules ne sont pas exhaustives.
pub(crate) async fn aggregate_facets(
    conn: &Client,
    req: &SearchRequest,
    hors_question: &[i64],
) -> std::result::Result<(i64, FacetCounters), tokio_postgres::Error> {
    let mut params: Params = Vec::new();
    let mut idx = 1usize;
    let parts = predicate_parts(req, &mut idx, &mut params);
    let predicate = compose(&parts);
    let complement = if hors_question.is_empty() {
        None
    } else {
        compose_complement(&parts, hors_question)
    };
    // L'index porte des jetons, pas des libellés, et le catalogue fait 1,3 M de
    // lignes : le titre se joint dans la même instruction, pas en mémoire.
    let agg = agg();
    let sql = format!(
        "WITH agg AS (\
           SELECT paradedb.aggregate('decisions_bm25', {predicate}, \
             '{agg}'::json, bucket_limit => 1000000) AS j, \
             {sup} AS k\
         ) \
         SELECT j::text, \
                (SELECT jsonb_object_agg(lt.text_uid, jsonb_build_array(lt.title, lt.slug)) \
                 FROM jsonb_array_elements(\
                        coalesce(j->'inst'->'buckets', '[]'::jsonb) \
                        || coalesce(k->'inst'->'buckets', '[]'::jsonb)) b \
                 JOIN legal_text lt ON lt.text_uid = b->>'key')::text, \
                k::text \
         FROM agg",
        sup = match &complement {
            Some(p) => format!(
                "paradedb.aggregate('decisions_bm25', {p}, '{agg}'::json, bucket_limit => 1000000)"
            ),
            None => "'{}'::jsonb".to_string(),
        },
    );
    let row = conn.query_one(sql.as_str(), &as_refs(&params)).await?;
    let raw: String = row.get(0);
    let json: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let mut counters = FacetCounters::default();
    let mut total = accumulate_counters(&json, &mut counters);
    if let Some(raw_sup) = row.get::<_, Option<String>>(2) {
        let sup: Value = serde_json::from_str(&raw_sup).unwrap_or(Value::Null);
        total += accumulate_counters(&sup, &mut counters);
    }
    let libelles: Value = row
        .get::<_, Option<String>>(1)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    for (uid, paire) in libelles.as_object().into_iter().flatten() {
        if let Some(titre) = paire.get(0).and_then(Value::as_str) {
            counters
                .instrument
                .labels
                .insert(uid.clone(), titre.to_string());
        }
        counters.instrument.slugs.insert(
            uid.clone(),
            paire.get(1).and_then(Value::as_str).map(str::to_string),
        );
    }
    Ok((total, counters))
}

fn buckets(node: &Value, key: &str) -> Vec<Value> {
    node.get(key)
        .and_then(|a| a.get("buckets"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn bucket_key(b: &Value) -> Option<String> {
    b.get("key").and_then(Value::as_str).map(str::to_string)
}

fn doc_count(b: &Value) -> i64 {
    b.get("doc_count").and_then(Value::as_i64).unwrap_or(0)
}

fn accumulate_counters(json: &Value, c: &mut FacetCounters) -> i64 {
    for seat in buckets(json, "seat") {
        if let Some(path) = bucket_key(&seat) {
            *c.seats.entry(path).or_insert(0) += doc_count(&seat);
        }
    }
    for (axe, dest) in [
        ("dom", &mut c.domain),
        ("sol", &mut c.solution),
        ("off", &mut c.office),
        ("pro", &mut c.procedure),
    ] {
        for b in buckets(json, axe) {
            if let Some(uid) = bucket_key(&b) {
                *dest.entry(uid).or_insert(0) += doc_count(&b);
            }
        }
    }
    // L'agrégat rend un seau par code source, la facette une valeur par groupe.
    // La somme est exacte parce qu'aucune décision ne porte deux codes du même
    // groupe : mesuré 0 sur les deux groupes qui en réunissent plusieurs.
    for b in buckets(json, "pub") {
        let Some(code) = bucket_key(&b) else { continue };
        for key in lj_core::publication::publication_keys(&[code]) {
            *c.publication
                .entry(format!("publication:{key}"))
                .or_insert(0) += doc_count(&b);
        }
    }
    // Un jour ne chevauche jamais deux années civiles, donc replier les seaux
    // d'un jour par millésime est exact. C'est ce qui dispense d'une colonne
    // `annee`, qu'aucune expression indexée ne sait rendre juste.
    for b in buckets(json, "jour") {
        let Some(annee) = b
            .get("key_as_string")
            .and_then(Value::as_str)
            .and_then(|s| s.get(..4))
        else {
            continue;
        };
        *c.year.entry(annee.to_string()).or_insert(0) += doc_count(&b);
    }
    for b in buckets(json, "inst") {
        if let Some(uid) = bucket_key(&b) {
            *c.instrument.counts.entry(uid).or_insert(0) += doc_count(&b);
        }
    }
    // Le composite porte son instrument en préfixe, ce qui range les articles
    // sous leur texte sans imbriquer les deux axes : imbriqués, 6 s contre 0,7.
    for b in buckets(json, "art") {
        let Some(composite) = bucket_key(&b) else {
            continue;
        };
        let Some((uid, num_key)) = composite.split_once('|') else {
            continue;
        };
        if !c.instrument.counts.contains_key(uid) {
            continue;
        }
        *c.instrument
            .articles
            .entry(uid.to_string())
            .or_default()
            .entry(num_key.to_string())
            .or_insert(0) += doc_count(&b);
    }
    json.get("total")
        .and_then(|t| t.get("value"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(v: serde_json::Value) -> SearchRequest {
        serde_json::from_value(v).expect("requête valide")
    }

    /// The seat is one axis: the ticked paths enter as one `term_set`, and a
    /// path matches everything under it.
    #[test]
    fn le_siege_entre_en_un_seul_term_set() {
        let r = req(serde_json::json!({
            "query": "",
            "seat": ["CC.SOCIALE", "CA.PARIS.C3"],
        }));
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let p = build_predicate(&r, &mut idx, &mut params);
        assert!(p.contains("paradedb.term_set"), "{p}");
        assert!(p.contains("paradedb.term('seat_paths'"), "{p}");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn sans_question_ni_filtre_le_predicat_est_tout_le_fonds_vif() {
        let p = build_predicate(
            &req(serde_json::json!({"query": ""})),
            &mut 1,
            &mut Vec::new(),
        );
        assert_eq!(
            p,
            "paradedb.boolean(must => ARRAY[paradedb.all()], \
             must_not => ARRAY[paradedb.exists('deleted_at')])"
        );
    }

    /// Le second agrégat compte les servies que la question n'apparie pas :
    /// question en `must_not`, filtres des deux côtés, et les mêmes `$n` que le
    /// prédicat de base puisque les deux entrent dans une instruction unique.
    #[test]
    fn le_complement_soustrait_la_question_et_garde_les_filtres() {
        let r = req(serde_json::json!({
            "query": "conges payes", "jurisdictionType": ["TJ"],
        }));
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let parts = predicate_parts(&r, &mut idx, &mut params);
        let base = compose(&parts);
        let comp = compose_complement(&parts, &[7, 9]).expect("question posée");
        assert_eq!(params.len(), 3, "corps, titre, type de juridiction");
        for p in [&base, &comp] {
            assert!(
                p.contains("paradedb.term('jurisdiction_type', $3::text)"),
                "{p}"
            );
        }
        let (must, must_not) = comp.split_once("must_not =>").expect("must_not posé");
        assert!(must.contains("paradedb.term('id', 7::bigint)"), "{must}");
        assert!(must.contains("paradedb.term('id', 9::bigint)"), "{must}");
        assert!(!must.contains("search_title"), "{must}");
        assert!(must_not.contains("search_title"), "{must_not}");
        assert!(
            must_not.contains("paradedb.exists('deleted_at')"),
            "{must_not}"
        );
    }

    #[test]
    fn sans_question_le_complement_n_existe_pas() {
        let mut params: Params = Vec::new();
        let parts = predicate_parts(
            &req(serde_json::json!({"query": "", "jurisdictionType": ["TJ"]})),
            &mut 1,
            &mut params,
        );
        assert!(compose_complement(&parts, &[7]).is_none());
    }

    #[test]
    fn les_supprimees_sortent_toujours() {
        for q in ["", "responsabilite"] {
            let p = build_predicate(
                &req(serde_json::json!({"query": q})),
                &mut 1,
                &mut Vec::new(),
            );
            assert!(
                p.contains("must_not => ARRAY[paradedb.exists('deleted_at')]"),
                "{p}"
            );
        }
    }

    /// Les bornes de dates passent en paramètre, jamais inlinées.
    #[test]
    fn les_bornes_de_dates_sont_un_daterange_parametre() {
        let mut params: Params = Vec::new();
        let r = req(serde_json::json!({
            "query": "", "dateFrom": "2020-01-01", "dateTo": "2021-06-30",
        }));
        let p = build_predicate(&r, &mut 1, &mut params);
        assert!(
            p.contains("paradedb.range('date_lecture', $1::text::daterange)"),
            "{p}"
        );
        assert_eq!(params.len(), 1);

        let mut params: Params = Vec::new();
        let r = req(serde_json::json!({"query": "", "dateTo": "2021-06-30"}));
        let _ = build_predicate(&r, &mut 1, &mut params);
        assert_eq!(params.len(), 1);
    }

    /// Les axes référentiels portent l'uid complet, pas le suffixe.
    #[test]
    fn les_axes_referentiels_portent_l_uid_complet() {
        let mut params: Params = Vec::new();
        let r = req(serde_json::json!({
            "query": "", "solution": ["REJET"], "procedure": ["QPC"], "office": ["JEX"],
        }));
        let p = build_predicate(&r, &mut 1, &mut params);
        for field in ["solution_uid", "procedure_uid", "office_uid"] {
            assert!(p.contains(&format!("paradedb.term('{field}'")), "{p}");
        }
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn la_projection_lit_l_arbre_et_replie_les_jours() {
        let json = serde_json::json!({
            "seat": {"buckets": [
                {"key": "CC", "doc_count": 30},
                {"key": "CC.SOCIALE", "doc_count": 18}]},
            "sol": {"buckets": [{"key": "solution:REJET", "doc_count": 25}]},
            "pro": {"buckets": [{"key": "procedure:QPC", "doc_count": 4}]},
            "pub": {"buckets": [{"key": "b", "doc_count": 7}, {"key": "n", "doc_count": 3}]},
            "jour": {"buckets": [
                {"key_as_string": "2024-03-01T00:00:00Z", "doc_count": 5},
                {"key_as_string": "2024-11-20T00:00:00Z", "doc_count": 6},
                {"key_as_string": "2025-01-09T00:00:00Z", "doc_count": 2}]},
            "total": {"value": 30.0},
        });
        let mut c = FacetCounters::default();
        let total = accumulate_counters(&json, &mut c);
        assert_eq!(total, 30);
        assert_eq!(c.seats["CC"], 30);
        assert_eq!(c.seats["CC.SOCIALE"], 18);
        assert_eq!(c.solution["solution:REJET"], 25);
        assert_eq!(c.procedure["procedure:QPC"], 4);
        assert_eq!(c.publication["publication:PUBLIE_BULLETIN"], 7);
        assert_eq!(c.publication["publication:INEDIT_BULLETIN"], 3);
        assert_eq!(c.year["2024"], 11);
        assert_eq!(c.year["2025"], 2);
    }

    #[test]
    fn une_reponse_vide_ne_panique_pas() {
        let mut c = FacetCounters::default();
        let total = accumulate_counters(&Value::Null, &mut c);
        assert_eq!(total, 0);
        assert!(c.seats.is_empty() && c.year.is_empty());
    }

    /// Le composite range l'article sous son texte par son préfixe. Celui d'un
    /// texte hors du top des instruments est jeté : la facette ne l'afficherait
    /// sous rien, les deux axes étant plats et tronqués à des rangs différents.
    #[test]
    fn l_article_se_range_sous_son_instrument_ou_disparait() {
        let mut c = FacetCounters::default();
        accumulate_counters(
            &serde_json::json!({
                "inst": {"buckets": [{"key": "LEGITEXT000006070716", "doc_count": 12}]},
                "art": {"buckets": [
                    {"key": "LEGITEXT000006070716|700", "doc_count": 9},
                    {"key": "LEGITEXT000006070716|145", "doc_count": 3},
                    {"key": "LEGITEXT000006072050|l1234-1", "doc_count": 7},
                    {"key": "sans-separateur", "doc_count": 5}
                ]}
            }),
            &mut c,
        );
        assert_eq!(c.instrument.counts["LEGITEXT000006070716"], 12);
        let articles = &c.instrument.articles["LEGITEXT000006070716"];
        assert_eq!(articles["700"], 9);
        assert_eq!(articles["145"], 3);
        assert_eq!(c.instrument.articles.len(), 1);
    }
}

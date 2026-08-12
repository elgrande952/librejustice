//! Détection de mode, traduction booléenne et construction de la query
//! phrase-combo body (gazetteer + chunks).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use lj_core::body_tok::{fold, is_stopword, tokenize as tokenize_body};
use regex::Regex;

use lj_dtos::QueryMode;

/// `\b(?:ET|OU|SAUF|AND|OR|NOT)\b|\b(?:PROCHE|NEAR)\d+\b|["*]` — détecteur de
/// query booléenne.
static BOOL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(?:ET|OU|SAUF|AND|OR|NOT)\b|\b(?:PROCHE|NEAR)\d+\b|[*"]"#).unwrap()
});

// ── Détection de mode / traduction booléenne ─────────────────────────────────

/// `lexical` si la query porte des opérateurs/guillemets, sinon `hybrid`.
pub fn detect_query_mode(query: &str) -> QueryMode {
    if BOOL_RE.is_match(query) {
        QueryMode::Lexical
    } else {
        QueryMode::Hybrid
    }
}

pub(crate) fn is_boolean_query(query: &str) -> bool {
    BOOL_RE.is_match(query)
}

/// Collapse les guillemets doublés `""x""` → `"x"` : deux quotes de chaque
/// côté, pas trois.
static DOUBLE_QUOTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\"\"([^\"]*?)\"\"").unwrap());
static ET_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bET\b").unwrap());
static OU_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bOU\b").unwrap());
static SAUF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bSAUF\b").unwrap());
/// Un token = phrase quotée (slop `~N` collé compris), ou token nu.
static TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[^"]*"(?:~\d+)?|\S+"#).unwrap());

/// Découpage pour la passe de proximité : phrase quotée, opérateur
/// `PROCHE`/`NEAR` suivi de son entier, ou token nu. L'opérateur passe avant
/// `\S+` pour être reconnu même collé à une phrase (`"a b"PROCHE3`).
static PROX_SCAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""[^"]*"|\b(?:PROCHE|NEAR)\d+\b|\S+"#).unwrap());
/// Token qui *est* l'opérateur de proximité, avec son slop en capture.
static PROX_OP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:PROCHE|NEAR)(\d+)$").unwrap());

pub(crate) fn translate_boolean(query: &str) -> String {
    let q = DOUBLE_QUOTE_RE.replace_all(query, r#""$1""#).into_owned();
    let q = rewrite_proximity(&q);
    let q = ET_RE.replace_all(&q, "AND").into_owned();
    let q = OU_RE.replace_all(&q, "OR").into_owned();
    let q = SAUF_RE.replace_all(&q, "NOT").into_owned();
    enforce_and_precedence(&q)
}

/// Slop d'un token opérateur (`PROCHE5` → 5), `None` si ce n'en est pas un.
fn proximity_slop(tok: &str) -> Option<u32> {
    PROX_OP_RE.captures(tok)?.get(1)?.as_str().parse().ok()
}

/// Un opérande de proximité : ni mot-clé booléen, ni opérateur de proximité.
fn is_proximity_operand(tok: &str) -> bool {
    !matches!(tok, "ET" | "OU" | "SAUF" | "AND" | "OR" | "NOT") && proximity_slop(tok).is_none()
}

/// Mots portés par un opérande : une phrase quotée est aplatie en ses mots, un
/// token nu se rend lui-même. Les guillemets tombent — la phrase à slop les
/// reposera autour de l'ensemble.
fn operand_words(tok: &str) -> Vec<String> {
    tok.replace('"', " ")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Réécrit les chaînes de proximité `A PROCHE5 B [PROCHE3 C…]` en phrase à slop
/// tantivy `"A B C"~5` (slop = max des N de la chaîne), seule construction de la
/// grammaire `paradedb.parse` qui exprime la proximité. Les opérandes sont des
/// mots nus ou des phrases quotées, aplatis dans la phrase ; le slop tantivy
/// tolère l'inversion (`"commercial bail"~5` matche « bail commercial »).
///
/// Un opérateur privé d'opérande à gauche ou à droite (`PROCHE5 bail`,
/// `"bail commercial"PROCHE5`, `bail ET PROCHE5`) est droppé : rien à
/// rapprocher.
fn rewrite_proximity(query: &str) -> String {
    if !PROX_SCAN_RE
        .find_iter(query)
        .any(|m| proximity_slop(m.as_str()).is_some())
    {
        return query.to_string();
    }
    let toks: Vec<&str> = PROX_SCAN_RE.find_iter(query).map(|m| m.as_str()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let Some(mut slop) = proximity_slop(toks[i]) else {
            out.push(toks[i].to_string());
            i += 1;
            continue;
        };
        let left = out.last().filter(|t| is_proximity_operand(t)).cloned();
        let right = toks.get(i + 1).copied().filter(|t| is_proximity_operand(t));
        let (Some(left), Some(right)) = (left, right) else {
            i += 1;
            continue;
        };
        out.pop();
        let mut words = operand_words(&left);
        words.extend(operand_words(right));
        i += 2;
        // Chaîne : `A PROCHE5 B PROCHE3 C` → une seule phrase, slop = max.
        while let (Some(next_slop), Some(operand)) = (
            toks.get(i).and_then(|t| proximity_slop(t)),
            toks.get(i + 1).copied().filter(|t| is_proximity_operand(t)),
        ) {
            slop = slop.max(next_slop);
            words.extend(operand_words(operand));
            i += 2;
        }
        out.push(format!("\"{}\"~{slop}", words.join(" ")));
    }
    out.join(" ")
}

/// Caractères que la grammaire tantivy interprète hors guillemets : un token nu
/// qui en porte un (`d'acte`, `acte(s`) fait rejeter la query entière par
/// `paradedb.parse` — l'apostrophe ouvre une phrase single-quote jamais fermée.
/// `' ( ) [ { : ^` cassent tous le parse ; `~` (slop) et `*` (wildcard) parsent
/// et restent porteurs de sens.
const TANTIVY_SYNTAX: &[char] = &['\'', '(', ')', '[', ']', '{', '}', ':', '^', '\\'];

/// Token composé uniquement d'étoiles : un wildcard sans préfixe n'est pas une
/// prefix-query — `+*` fait rejeter la query entière par `paradedb.parse`.
/// Droppé comme un stopword.
fn is_bare_star(tok: &str) -> bool {
    !tok.is_empty() && tok.chars().all(|c| c == '*')
}

/// Re-sert un token nu porteur de syntaxe tantivy en phrase double-quotée :
/// l'index la re-tokenise comme le texte source (l'élision `d'acte` se splitte
/// à l'identique des deux côtés), le match est préservé. Les tokens déjà
/// quotés et les wildcards passent tels quels.
fn quote_bare_syntax_token(tok: &str, syntax: &[char]) -> String {
    if tok.starts_with('"') || tok.contains('*') || !tok.contains(syntax) {
        tok.to_string()
    } else {
        format!("\"{}\"", tok.replace('"', " "))
    }
}

/// Force la précédence AND (Tantivy par défaut OR) en préfixant `+`/`-` ;
/// strippe les stopwords FR qui n'existent pas dans l'index.
fn enforce_and_precedence(query: &str) -> String {
    if query.contains(" OR ") {
        // Chemin OR servi tel quel à tantivy : le grouping `(…)` y est de la
        // syntaxe voulue, seule l'apostrophe est neutralisée.
        let toks = TOKEN_RE
            .find_iter(query)
            .filter(|m| !is_bare_star(m.as_str().trim_start_matches(['+', '-'])))
            .map(|m| {
                let tok = m.as_str();
                match tok.as_bytes().first() {
                    Some(b'+') | Some(b'-') => {
                        let (prefix, body) = tok.split_at(1);
                        format!("{prefix}{}", quote_bare_syntax_token(body, &['\'']))
                    }
                    _ => quote_bare_syntax_token(tok, &['\'']),
                }
            });
        // Le drop d'une étoile nue peut rendre un opérateur pendant
        // (« congés OR » / « OR congés ») — un parse error tantivy.
        let is_op = |t: &str| matches!(t, "AND" | "OR" | "NOT");
        let mut out: Vec<String> = Vec::new();
        for t in toks {
            if is_op(&t) && out.last().is_none_or(|p| is_op(p)) {
                continue;
            }
            out.push(t);
        }
        while out.last().is_some_and(|t| is_op(t)) {
            out.pop();
        }
        return out.join(" ");
    }
    let mut out: Vec<String> = Vec::new();
    let mut next_neg = false;
    for m in TOKEN_RE.find_iter(query) {
        let tok = m.as_str();
        if tok == "AND" {
            continue;
        }
        if tok == "NOT" {
            next_neg = true;
            continue;
        }
        if is_bare_star(tok.trim_start_matches(['+', '-'])) {
            next_neg = false;
            continue;
        }
        if tok.starts_with('+') || tok.starts_with('-') {
            let (prefix, body) = tok.split_at(1);
            out.push(format!(
                "{prefix}{}",
                quote_bare_syntax_token(body, TANTIVY_SYNTAX)
            ));
        } else if is_stopword(&fold(tok)) {
            next_neg = false;
            continue;
        } else if next_neg {
            out.push(format!("-{}", quote_bare_syntax_token(tok, TANTIVY_SYNTAX)));
        } else {
            out.push(format!("+{}", quote_bare_syntax_token(tok, TANTIVY_SYNTAX)));
        }
        next_neg = false;
    }
    out.join(" ")
}

// ── Phrase-combo body (gazetteer + chunks) ───────────────────────────────────

/// Génère la query `paradedb.parse` pour la jambe BM25 body (chemin prod =
/// [`BodyArm::Split`]).
pub(crate) fn phrase_combo_parse(query: &str) -> String {
    body_query_for_arm(query, BodyArm::Split)
}

/// Bras de construction de la jambe body BM25, pour l'A/B de ranking offline
/// (`lj-bench rank-arms`). `Split` est le chemin prod : sac OR + phrase entière +
/// runs de contenu découpés aux stopwords (le gazetteer reste hors chemin).
/// `Bag` ne garde que le sac OR (aucune clause phrase). `Weighted(w)` part de
/// `Split` et pondère chaque clause phrase par `^w` (`w < 1` ⇒
/// sous-pondération).
#[derive(Debug, Clone, Copy)]
pub enum BodyArm {
    Bag,
    Split,
    Weighted(f64),
}

/// Query body d'un bras, à passer à `paradedb.parse_with_field('body', …)`.
/// Réutilise le builder pur [`phrase_combo_clauses`] : un seul lieu de vérité
/// pour la construction du corps, les bras n'en sélectionnant qu'une vue.
pub fn body_query_for_arm(query: &str, arm: BodyArm) -> String {
    let toks = tokenize_body(query);
    if toks.is_empty() {
        return query.to_string();
    }
    let parts = phrase_combo_clauses(&toks, &[]);
    match arm {
        // parts[0] = le sac OR (toujours en tête) ; les clauses phrase sont jetées.
        BodyArm::Bag => parts.into_iter().next().unwrap_or_default(),
        BodyArm::Split => parts.join(" "),
        BodyArm::Weighted(w) => parts
            .into_iter()
            .enumerate()
            .map(|(i, p)| if i == 0 { p } else { format!("{p}^{w}") })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Construit les clauses (gazetteer injecté via `spans`, pour testabilité) :
///
/// 1. sac OR : tous les tokens dédupés (rappel maximal) ;
/// 2. phrase entière (si ≥ 2 tokens) : boost du verbatim ;
/// 3. clauses phrase, en un balayage gauche→droite : à chaque span de
///    collocation reconnue (peut contenir des stopwords internes — l'index
///    préserve les gaps de position, donc `"tribunal de commerce"` matche), on
///    émet la collocation ; entre/hors spans, on regroupe les runs de contenu
///    ≥ 2 tokens, bornés aux tokens non couverts.
///
/// La collocation à stopword interne remplace ainsi le fragment produit par le
/// split-au-stopword (« tribunal · commerce paris » → « tribunal de commerce »).
pub(crate) fn phrase_combo_clauses(toks: &[String], spans: &[(usize, usize)]) -> Vec<String> {
    // 1. Sac OR : dédup en préservant l'ordre.
    let mut seen = HashSet::new();
    let or_bag = toks
        .iter()
        .filter(|t| seen.insert((*t).clone()))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let mut parts: Vec<String> = vec![or_bag];

    // 2. Phrase entière.
    if toks.len() >= 2 {
        parts.push(format!("\"{}\"", toks.join(" ")));
    }

    // 3. Balayage : collocations (spans) + runs de contenu sur les gaps.
    let span_end: HashMap<usize, usize> = spans.iter().copied().collect();
    let mut run: Vec<&str> = Vec::new();
    let flush = |run: &mut Vec<&str>, parts: &mut Vec<String>| {
        if run.len() >= 2 {
            parts.push(format!("\"{}\"", run.join(" ")));
        }
        run.clear();
    };
    let mut i = 0;
    while i < toks.len() {
        if let Some(&end) = span_end.get(&i) {
            flush(&mut run, &mut parts);
            parts.push(format!("\"{}\"", toks[i..end].join(" ")));
            i = end;
        } else if is_stopword(&toks[i]) {
            flush(&mut run, &mut parts);
            i += 1;
        } else {
            run.push(&toks[i]);
            i += 1;
        }
    }
    flush(&mut run, &mut parts);

    // Dédup des clauses : sans stopword ni collocation, l'unique run == la phrase
    // entière (double poids sinon).
    let mut seen_parts = HashSet::new();
    parts.retain(|p| seen_parts.insert(p.clone()));
    parts
}

/// `true` si la requête ne porte aucun token de contenu indexable : vide, ou
/// 100 % stopwords (que le tokenizer `body` retire). Les wildcards `terme*`
/// survivent à la tokenisation comme prefix-query (jamais term-vides), donc une
/// requête à `*` n'est jamais court-circuitée. Sinon les jambes BM25 généreraient
/// une query tantivy sans clause, que `paradedb.parse` rejette (`body:()` → parse
/// error).
pub(crate) fn query_lacks_searchable_terms(query: &str) -> bool {
    let has_prefix_wildcard = query
        .split_whitespace()
        .any(|t| t.contains('*') && !is_bare_star(t.trim_start_matches(['+', '-'])));
    !has_prefix_wildcard && tokenize_body(query).iter().all(|t| is_stopword(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    use lj_core::body_tok::content_chunks;

    #[test]
    fn detect_mode_boolean_operators() {
        assert_eq!(detect_query_mode("congés ET payés"), QueryMode::Lexical);
        assert_eq!(detect_query_mode("\"phrase exacte\""), QueryMode::Lexical);
        assert_eq!(detect_query_mode("expulsion locative"), QueryMode::Hybrid);
        assert_eq!(
            detect_query_mode("travail PROCHE3 dimanche"),
            QueryMode::Lexical
        );
        assert_eq!(
            detect_query_mode("travail NEAR3 dimanche"),
            QueryMode::Lexical
        );
    }

    #[test]
    fn translate_boolean_et_ou_sauf() {
        assert_eq!(translate_boolean("congés ET payés"), "+congés +payés");
        assert_eq!(translate_boolean("congés OU payés"), "congés OR payés");
        assert_eq!(translate_boolean("congés SAUF maladie"), "+congés -maladie");
    }

    #[test]
    fn proximity_infix_becomes_slop_phrase() {
        assert_eq!(
            translate_boolean("licenciement PROCHE5 harcèlement"),
            r#"+"licenciement harcèlement"~5"#
        );
        assert_eq!(
            translate_boolean("bail NEAR5 commercial"),
            translate_boolean("bail PROCHE5 commercial")
        );
        assert_eq!(
            translate_boolean(r#""travail dominical" PROCHE3 rupture"#),
            r#"+"travail dominical rupture"~3"#
        );
        assert_eq!(
            translate_boolean("bail PROCHE5 commercial PROCHE3 résiliation"),
            r#"+"bail commercial résiliation"~5"#
        );
        assert_eq!(
            translate_boolean("bail PROCHE5 commercial ET résiliation"),
            r#"+"bail commercial"~5 +résiliation"#
        );
        assert_eq!(
            translate_boolean("bail PROCHE5 commercial SAUF habitation"),
            r#"+"bail commercial"~5 -habitation"#
        );
        assert_eq!(
            translate_boolean("bail PROCHE5 commercial OU habitation"),
            r#""bail commercial"~5 OR habitation"#
        );
    }

    #[test]
    fn dangling_proximity_operator_is_dropped() {
        assert_eq!(
            translate_boolean(r#""travail dimanche"PROCHE3"#),
            r#"+"travail dimanche""#
        );
        assert_eq!(translate_boolean("PROCHE5 bail"), "+bail");
        assert_eq!(translate_boolean("bail ET PROCHE5"), "+bail");
        assert_eq!(translate_boolean("approche5 ET bail"), "+approche5 +bail");
        assert_eq!(detect_query_mode("approche5 bail"), QueryMode::Hybrid);
    }

    #[test]
    fn enforce_and_strips_stopwords() {
        assert_eq!(enforce_and_precedence("congés de payés"), "+congés +payés");
    }

    #[test]
    fn bare_syntax_tokens_are_quoted() {
        assert_eq!(
            translate_boolean(r#"prise d'acte "produit les effets d'une démission""#),
            r#"+prise +"d'acte" +"produit les effets d'une démission""#
        );
        assert_eq!(translate_boolean(r#"acte(s ET nul"#), r#"+"acte(s" +nul"#);
        assert_eq!(
            translate_boolean(r#"répar* ET "l'astreinte""#),
            r#"+répar* +"l'astreinte""#
        );
        assert_eq!(
            translate_boolean("congés OU d'aménagement"),
            r#"congés OR "d'aménagement""#
        );
    }

    #[test]
    fn bare_star_token_is_dropped() {
        assert_eq!(translate_boolean("2 * 3"), "+2 +3");
        assert_eq!(translate_boolean("faute ** dommage"), "+faute +dommage");
        assert_eq!(translate_boolean("congés OU *"), "congés");
        assert_eq!(translate_boolean("répar* ET faute"), "+répar* +faute");
        assert!(query_lacks_searchable_terms("*"));
        assert!(query_lacks_searchable_terms("* **"));
    }

    #[test]
    fn stopword_only_query_lacks_searchable_terms() {
        assert!(query_lacks_searchable_terms("de la"));
        assert!(query_lacks_searchable_terms("le la les des du"));
        assert!(query_lacks_searchable_terms("de ET la"));
        assert!(query_lacks_searchable_terms(""));
        assert!(query_lacks_searchable_terms("   "));
        assert!(!query_lacks_searchable_terms("responsabilité"));
        assert!(!query_lacks_searchable_terms("code de la route"));
        assert!(!query_lacks_searchable_terms("de*"));
    }

    #[derive(serde::Deserialize)]
    struct QueryCase {
        query: String,
        mode: String,
        translate_boolean: String,
        phrase_combo_parse: String,
        tokenize_body: Vec<String>,
        content_chunks: Vec<Vec<String>>,
    }
    #[derive(serde::Deserialize)]
    struct QueryFixture {
        cases: Vec<QueryCase>,
    }

    #[test]
    fn query_construction_parity_oracle() {
        let raw = include_str!("../../tests/fixtures/oracle/query_strings.json");
        let fix: QueryFixture = serde_json::from_str(raw).expect("fixture query_strings");
        for c in &fix.cases {
            let mode = match detect_query_mode(&c.query) {
                QueryMode::Lexical => "lexical",
                QueryMode::Hybrid => "hybrid",
            };
            assert_eq!(mode, c.mode, "detect_query_mode({:?})", c.query);
            assert_eq!(
                translate_boolean(&c.query),
                c.translate_boolean,
                "translate_boolean({:?})",
                c.query
            );
            assert_eq!(
                phrase_combo_clauses(&tokenize_body(&c.query), &[]).join(" "),
                c.phrase_combo_parse,
                "phrase_combo_clauses({:?}, [])",
                c.query
            );
            assert_eq!(
                tokenize_body(&c.query),
                c.tokenize_body,
                "tokenize_body({:?})",
                c.query
            );
            assert_eq!(
                content_chunks(&tokenize_body(&c.query), 2),
                c.content_chunks,
                "content_chunks({:?})",
                c.query
            );
        }
    }

    #[test]
    fn phrase_combo_dedups_or_leg() {
        let out = phrase_combo_parse("podologie podologie");
        assert!(out.starts_with("podologie "), "got: {out}");
    }

    #[test]
    fn phrase_combo_gazetteer_replaces_split_fragment() {
        let toks = tokenize_body("tribunal de commerce de paris competence litige");
        let m = lj_core::collocations::Matcher::from_phrases(["tribunal de commerce"]);
        let clauses = phrase_combo_clauses(&toks, &m.spans(&toks));
        assert!(
            clauses.iter().any(|c| c == "\"tribunal de commerce\""),
            "{clauses:?}"
        );
        assert!(
            !clauses.iter().any(|c| c == "\"commerce paris\""),
            "{clauses:?}"
        );
        assert_eq!(clauses[0], "tribunal de commerce paris competence litige");
        assert!(clauses
            .iter()
            .any(|c| c == "\"tribunal de commerce de paris competence litige\""));
    }

    #[test]
    fn phrase_combo_gazetteer_two_disjoint_collocations() {
        let toks = tokenize_body("code de la route exces de vitesse permis");
        let m =
            lj_core::collocations::Matcher::from_phrases(["code de la route", "exces de vitesse"]);
        let clauses = phrase_combo_clauses(&toks, &m.spans(&toks));
        assert!(
            clauses.iter().any(|c| c == "\"code de la route\""),
            "{clauses:?}"
        );
        assert!(
            clauses.iter().any(|c| c == "\"exces de vitesse\""),
            "{clauses:?}"
        );
        assert!(
            !clauses.iter().any(|c| c == "\"route exces\""),
            "{clauses:?}"
        );
    }

    #[test]
    fn phrase_combo_uncovered_content_run_survives() {
        let toks = tokenize_body("tribunal de commerce detention provisoire");
        let m = lj_core::collocations::Matcher::from_phrases(["tribunal de commerce"]);
        let clauses = phrase_combo_clauses(&toks, &m.spans(&toks));
        assert!(
            clauses.iter().any(|c| c == "\"tribunal de commerce\""),
            "{clauses:?}"
        );
        assert!(
            clauses.iter().any(|c| c == "\"detention provisoire\""),
            "{clauses:?}"
        );
    }
}

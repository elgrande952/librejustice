//! Cache référentiel process-local : `facet_value` + `jurisdiction` chargés
//! depuis Postgres via `lj-store`, servis aux handlers pour résoudre labels de
//! facettes, tags des hits et arbre juridiction.
//!
//! Cache moka mono-entrée, TTL 1 h (aligné sur les caches search/rerank) : le
//! référentiel ne bouge qu'aux migrations/backfills ; un UPDATE de label est
//! visible au pire une heure plus tard, sans redéploiement.

use std::collections::HashMap;
use std::sync::Arc;

use lj_dtos::FacetTag;
use lj_store::repository::{DecisionRepository, FacetValueRow, JurisdictionRow};

use crate::error::{ApiError, Result};
use crate::state::AppState;

/// Une valeur de facette du référentiel (`facet_value`), indexée par uid.
#[derive(Debug, Clone)]
pub struct FacetValueEntry {
    pub facet: String,
    pub label: String,
    pub abbr: Option<String>,
    pub parent_uid: Option<String>,
}

/// Une unité juridictionnelle (`jurisdiction`), indexée par code.
/// `source_code` = code de la source (location Judilibre) — sert aux suggestions
/// MCP quand un client envoie un ancien code.
#[derive(Debug, Clone)]
pub struct JurisdictionEntry {
    pub jurisdiction_type: String,
    pub source_code: String,
    pub label: String,
}

/// Vue en mémoire des référentiels de facettes. Immutable après chargement,
/// partagée en `Arc` via le cache de l'[`AppState`].
#[derive(Debug, Default)]
pub struct Referential {
    values: HashMap<String, FacetValueEntry>,
    jurisdictions: HashMap<String, JurisdictionEntry>,
}

impl Referential {
    /// Construit la vue depuis les lignes DB (ou des fixtures de test).
    pub fn new(values: Vec<FacetValueRow>, jurisdictions: Vec<JurisdictionRow>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|r| {
                    (
                        r.uid,
                        FacetValueEntry {
                            facet: r.facet,
                            label: r.label,
                            abbr: r.abbr,
                            parent_uid: r.parent_uid,
                        },
                    )
                })
                .collect(),
            jurisdictions: jurisdictions
                .into_iter()
                .map(|r| {
                    (
                        r.code,
                        JurisdictionEntry {
                            jurisdiction_type: r.jurisdiction_type,
                            source_code: r.source_code,
                            label: r.label,
                        },
                    )
                })
                .collect(),
        }
    }

    /// Entrée `facet_value` d'un uid complet (`solution:REJET`).
    pub fn value(&self, uid: &str) -> Option<&FacetValueEntry> {
        self.values.get(uid)
    }

    /// Label FR d'un uid complet ; repli transparent sur le **suffixe** d'uid
    /// si l'entrée manque.
    pub fn label<'a>(&'a self, uid: &'a str) -> &'a str {
        self.values
            .get(uid)
            .map(|e| e.label.as_str())
            .unwrap_or_else(|| uid_suffix(uid))
    }

    /// Tag référentiel d'un uid complet : `key` = suffixe d'uid, `label` résolu.
    pub fn tag(&self, uid: &str) -> FacetTag {
        FacetTag {
            key: uid_suffix(uid).to_string(),
            label: self.label(uid).to_string(),
        }
    }

    /// Label d'un type de juridiction (`TJ` → « Tribunal judiciaire »), depuis
    /// les lignes `jurisdiction_type:*`.
    pub fn jurisdiction_type_label(&self, code: &str) -> Option<&str> {
        self.values
            .get(&format!("jurisdiction_type:{code}"))
            .map(|e| e.label.as_str())
    }

    /// Entrée `jurisdiction` d'un code (`tj_le_havre`).
    pub fn jurisdiction(&self, code: &str) -> Option<&JurisdictionEntry> {
        self.jurisdictions.get(code)
    }

    /// Itère les unités juridictionnelles `(code, entrée)`.
    pub fn jurisdictions(&self) -> impl Iterator<Item = (&str, &JurisdictionEntry)> {
        self.jurisdictions.iter().map(|(c, e)| (c.as_str(), e))
    }

    /// Tokens (suffixes d'uid) d'une facette, triés — le vocabulaire valide
    /// d'un axe à catégorie contrôlée (`chamber`, `publication`).
    pub fn facet_tokens(&self, facet: &str) -> Vec<&str> {
        let mut tokens: Vec<&str> = self
            .values
            .iter()
            .filter(|(_, e)| e.facet == facet)
            .map(|(uid, _)| uid_suffix(uid))
            .collect();
        tokens.sort_unstable();
        tokens
    }

    /// Premier token hors vocabulaire d'un axe à catégorie contrôlée.
    pub fn find_unknown_token<'a>(&self, facet: &str, tokens: &'a [String]) -> Option<&'a str> {
        tokens
            .iter()
            .map(String::as_str)
            .find(|t| !self.values.contains_key(&format!("{facet}:{t}")))
    }
}

/// Message correctif pour un `jurisdiction_code` hors référentiel, avec
/// suggestions : code source exact (« tj75056 » → « tj_paris »), segment de
/// libellé matchant le lieu demandé — graphies ultramarines comprises via
/// [`lj_core::outremer`], sans quoi « ta_cayenne » ne trouve jamais
/// « ta_guyane » —, et codes proches en distance d'édition. Les unités du
/// **type** demandé passent devant : `ta_cayenne` cherche un tribunal
/// administratif, pas la cour d'appel homonyme.
pub fn unknown_jurisdiction_code_msg(code: &str, refs: &Referential) -> String {
    // Préfixe de type effectivement porté par des codes du référentiel
    // (« ta_ », « tcom_ ») — sinon le code entier est du texte libre.
    let kind = code
        .split_once('_')
        .map(|(k, _)| format!("{k}_"))
        .filter(|k| refs.jurisdictions().any(|(c, _)| c.starts_with(k)));
    let place = kind.as_deref().map_or(code, |k| &code[k.len()..]);
    let mut needles = vec![lj_core::text::slug(place)];
    // Dernier segment : les codes devinés portent souvent la ville en queue
    // (« tribunal_paris », « conseil_etat »).
    if let Some(last) = place.rsplit(['_', '-']).next() {
        needles.push(lj_core::text::slug(last));
    }
    for n in needles.clone() {
        needles.extend(
            lj_core::outremer::equivalents(&n)
                .iter()
                .map(|s| (*s).into()),
        );
    }

    // Tri : lieu reconnu d'abord (le seul signal fiable), puis type demandé,
    // puis distance.
    let mut scored: Vec<(bool, bool, usize, String, String)> = refs
        .jurisdictions()
        .filter_map(|(c, e)| {
            let label = lj_core::text::slug(&e.label);
            let distance = levenshtein(code, c);
            let place_hit = e.source_code == code || needles.iter().any(|n| segment_of(&label, n));
            let same_kind = kind.as_deref().is_some_and(|k| c.starts_with(k));
            (place_hit || distance <= 3).then(|| {
                (
                    !place_hit,
                    !same_kind,
                    distance,
                    c.to_string(),
                    e.label.clone(),
                )
            })
        })
        .collect();
    scored.sort();
    let matches: Vec<String> = scored
        .iter()
        .take(5)
        .map(|(_, _, _, c, label)| format!("\"{c}\" ({label})"))
        .collect();
    let hint = if matches.is_empty() {
        String::new()
    } else {
        format!("; did you mean: {}", matches.join(", "))
    };
    format!("unknown jurisdiction_code '{code}'{hint}")
}

/// `needle` présent dans `hay` en segment entier (bornes `-` ou extrémités) —
/// une sous-chaîne libre fait matcher « ta_basse_terre » sur Nanterre.
fn segment_of(hay: &str, needle: &str) -> bool {
    needle.len() >= 3
        && hay.match_indices(needle).any(|(i, _)| {
            let end = i + needle.len();
            (i == 0 || hay.as_bytes()[i - 1] == b'-')
                && (end == hay.len() || hay.as_bytes()[end] == b'-')
        })
}

/// Distance de Levenshtein (DP à une ligne), pour les suggestions de codes.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Suffixe d'un uid namespacé (`solution:REJET` → `REJET`).
pub fn uid_suffix(uid: &str) -> &str {
    uid.split_once(':').map(|(_, s)| s).unwrap_or(uid)
}

/// Référentiel depuis le cache de l'état (TTL 1 h), chargé au premier accès.
pub async fn referential(state: &AppState) -> Result<Arc<Referential>> {
    state
        .referential_cache
        .try_get_with((), load(state))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("referential load: {e}")))
}

async fn load(state: &AppState) -> Result<Arc<Referential>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let values = repo.load_facet_values().await.map_err(ApiError::Store)?;
    let jurisdictions = repo.load_jurisdictions().await.map_err(ApiError::Store)?;
    Ok(Arc::new(Referential::new(values, jurisdictions)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jr(code: &str, jurisdiction_type: &str, label: &str) -> JurisdictionRow {
        JurisdictionRow {
            code: code.to_string(),
            source_code: code.to_string(),
            jurisdiction_type: jurisdiction_type.to_string(),
            city: None,
            label: label.to_string(),
        }
    }

    fn fv(uid: &str, label: &str, parent: Option<&str>) -> FacetValueRow {
        FacetValueRow {
            uid: uid.to_string(),
            facet: uid.split(':').next().unwrap().to_string(),
            label: label.to_string(),
            abbr: None,
            parent_uid: parent.map(str::to_string),
            sort: 0,
        }
    }

    #[test]
    fn tag_resolves_label_and_falls_back_to_suffix() {
        let refs = Referential::new(
            vec![
                fv("solution:REJET", "Rejet", None),
                fv("jurisdiction_type:TJ", "Tribunal judiciaire", None),
            ],
            vec![JurisdictionRow {
                code: "tj_le_havre".to_string(),
                source_code: "tj76351".to_string(),
                jurisdiction_type: "TJ".to_string(),
                city: Some("Le Havre".to_string()),
                label: "Tribunal judiciaire du Havre".to_string(),
            }],
        );
        let tag = refs.tag("solution:REJET");
        assert_eq!((tag.key.as_str(), tag.label.as_str()), ("REJET", "Rejet"));
        let tag = refs.tag("solution:INCONNU");
        assert_eq!(tag.label, "INCONNU");
        assert_eq!(
            refs.jurisdiction_type_label("TJ"),
            Some("Tribunal judiciaire")
        );
        assert_eq!(
            refs.jurisdiction("tj_le_havre").map(|j| j.label.as_str()),
            Some("Tribunal judiciaire du Havre")
        );
    }

    #[test]
    fn unknown_code_suggests_accented_label_from_ascii_needle() {
        let refs = Referential::new(
            vec![],
            vec![JurisdictionRow {
                code: "ce".to_string(),
                source_code: "CONSETAT".to_string(),
                jurisdiction_type: "CE".to_string(),
                city: None,
                label: "Conseil d'État".to_string(),
            }],
        );
        assert_eq!(
            unknown_jurisdiction_code_msg("conseil_etat", &refs),
            "unknown jurisdiction_code 'conseil_etat'; did you mean: \"ce\" (Conseil d'État)"
        );
    }

    #[test]
    fn unknown_code_bridges_outremer_naming_and_ranks_the_asked_type_first() {
        let refs = Referential::new(
            vec![],
            vec![
                jr("ta_guyane", "TA", "Tribunal administratif de la Guyane"),
                jr("ca_cayenne", "CA", "Cour d'appel de Cayenne"),
                jr("ta_caen", "TA", "Tribunal administratif de Caen"),
                jr("tcom_nanterre", "TCOM", "Tribunal de commerce de Nanterre"),
                jr(
                    "ta_guadeloupe",
                    "TA",
                    "Tribunal administratif de la Guadeloupe",
                ),
            ],
        );
        assert_eq!(
            unknown_jurisdiction_code_msg("ta_cayenne", &refs),
            "unknown jurisdiction_code 'ta_cayenne'; did you mean: \
             \"ta_guyane\" (Tribunal administratif de la Guyane), \
             \"ca_cayenne\" (Cour d'appel de Cayenne), \
             \"ta_caen\" (Tribunal administratif de Caen)"
        );
        let msg = unknown_jurisdiction_code_msg("ta_basse_terre", &refs);
        assert!(msg.contains("ta_guadeloupe"), "{msg}");
        assert!(!msg.contains("nanterre"), "{msg}");
    }

    #[test]
    fn facet_tokens_and_unknown_token_lookup() {
        let refs = Referential::new(
            vec![
                fv("chamber:SOCIALE", "Chambre sociale", None),
                fv("chamber:CIVILE", "Chambre civile", None),
                fv("publication:B", "Bulletin", None),
            ],
            vec![],
        );
        assert_eq!(refs.facet_tokens("chamber"), vec!["CIVILE", "SOCIALE"]);
        let values = vec!["SOCIALE".to_string(), "JAF".to_string()];
        assert_eq!(refs.find_unknown_token("chamber", &values), Some("JAF"));
        assert_eq!(
            refs.find_unknown_token("publication", &["B".to_string()]),
            None
        );
    }
}

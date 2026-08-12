//! `lj-dtos` — contrats api ↔ web.
//!
//! Tous les structs sont `#[serde(rename_all = "camelCase")]`. `lj-api` et
//! `lj-web` importent ces types, jamais de redéfinition côté consommateur. Les
//! enums de schéma (`JurisdictionLevel`…) vivent dans `schema` (ce crate).
//!
//! Les corps de requête sont annotés `#[serde(deny_unknown_fields)]`. Les
//! timestamps (profil/activité) sont portés en `String` (RFC 3339) : `lj-dtos`
//! n'a pas `chrono` en dépendance et reste un pur crate de contrats sérialisés
//! — la conversion `NaiveDate`/`DateTime` vit dans `lj-store`/`lj-api`.
//!
//! Noyau léger serde-pur partagé backend ↔ front wasm : aucune dépendance
//! lourde (pas de `lj-core`, qui tire `tokenizers`/`quick-xml` incompatibles
//! `wasm32`), pour garder le wasm minimal. La taxonomie (`schema`) vit ici,
//! source unique ; `lj-core` la réexporte.

use serde::{Deserialize, Serialize};

pub mod schema;

pub use schema::{
    abbreviate_jurisdiction_name, Domain, JurisdictionLevel, JurisdictionType, Office, Procedure,
    Significance, Solution,
};

// ── Enums propres à l'API (canal, mode de recherche, tri) ────────────────────

/// Canal d'origine d'une activité utilisateur (`web` = UI ; `mcp` = endpoint IA).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivitySource {
    Web,
    Mcp,
}

/// Moteur interrogé par une recherche de l'historique — axe orthogonal au canal
/// `ActivitySource` : `decisions` (jurisprudence), `textes` (référentiel de
/// normes) ou `annonces` (pot des événements en cours).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    Decisions,
    Textes,
    Annonces,
}

/// Contexte d'un appel de recherche (param transport `context`, pas un
/// filtre) : `user` = recherche posée par l'utilisateur ; `teaser` = fetch
/// machine des ponts croisés décisions ↔ textes — jamais enregistré en
/// historique, marqué sur le span pour l'exclure des comptages d'usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchContext {
    User,
    Teaser,
}

/// Header posé par le front quand une session vit dans le storage local mais
/// que le jeton d'accès n'a pas pu être obtenu (deadline de rafraîchissement
/// Supabase dépassé) : la requête part en anonyme, et sans ce témoin elle est
/// indiscernable de celle d'un visiteur réellement déloggé. Reporté en attribut
/// de span `librejustice.search.session` = `degraded` (vs `ok`).
pub const SESSION_DEGRADED_HEADER: &str = "x-lj-session-degraded";

/// Mode de recherche demandé. `semantic` force le hybride.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    Auto,
    Lexical,
    Semantic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    Relevance,
    DateDesc,
    DateAsc,
}

/// Mode de recherche effectivement résolu et renvoyé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryMode {
    Lexical,
    Hybrid,
}

// ── Health ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthResponse {
    /// Toujours `"ok"`.
    pub status: String,
    pub version: String,
}

// ── Stats corpus ───────────────────────────────────────────────────────────

/// Compteurs globaux du corpus pour la page d'accueil. Comptes exacts servis
/// depuis un cache process-local (TTL long).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorpusStatsResponse {
    /// Nombre de décisions actives indexées (non soft-deleted).
    pub decisions_count: i64,
    /// Nombre de textes normatifs du corpus (tout `legal_text`, toutes natures :
    /// codes, lois, traités, décrets, arrêtés, circulaires… — pas seulement le
    /// catalogue navigable `/codes`).
    pub texts_count: i64,
    /// Nombre d'articles en vigueur du corpus (identités `(text_uid, num_key)`
    /// distinctes, statut `VIGUEUR`).
    pub articles_count: i64,
}

// ── Search ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_type: Option<Vec<JurisdictionType>>,
    /// Solutions du dispositif (référentiel `solution:*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<Vec<Solution>>,
    /// Voies procédurales (référentiel `procedure:*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<Vec<Procedure>>,
    /// Juges/offices spécialisés (référentiel `office:*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<Vec<Office>>,
    /// Domaines de place (référentiel `legal_domain:*`) — une racine
    /// sélectionnée matche elle-même + toutes ses feuilles (expansion API).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<Vec<Domain>>,
    /// Codes du référentiel `jurisdiction` (`tj76351`, `ca_paris`, `cc`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<Vec<String>>,
    /// Chambres (catégorie contrôlée `chamber:*`) — axe uniforme
    /// tous ordres (suffixes d'uid : `CIVILE`, `SOCIALE`, `ETRANGERS`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chamber: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_instrument: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_article: Option<Vec<String>>,
    /// Niveaux de publication (suffixes d'uid `publication:*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<Vec<String>>,
    /// Portées jurisprudentielles (référentiel `significance:*`) — groupes
    /// de `publication_codes` au rang le plus fort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub significance: Option<Vec<Significance>>,
    /// Date ISO `YYYY-MM-DD`. Cap : `1678-01-01` ↔ `2262-01-01`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_to: Option<String>,
    #[serde(default = "default_search_mode")]
    pub mode: SearchMode,
    #[serde(default = "default_sort")]
    pub sort: SortOrder,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    /// Mode IA : active le reranker LLM. No-op en lexical.
    #[serde(default)]
    pub ai_mode: bool,
}

fn default_search_mode() -> SearchMode {
    SearchMode::Auto
}
fn default_sort() -> SortOrder {
    SortOrder::Relevance
}
fn default_limit() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BestChunk {
    pub chunk_index: i32,
    pub snippet: String,
}

/// Un choix de facette, servi avec son libellé depuis le référentiel DB.
/// `parent` porte la hiérarchie (arbres juridiction/domaine, 2 niveaux) :
/// `None` = racine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FacetChoice {
    pub value: String,
    pub label: String,
    pub count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// Paire clé (code référentiel) + libellé FR résolue par l'API depuis
/// `facet_value` — le front rend le label sans table compilée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FacetTag {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalInstrumentFacet {
    /// Token de filtre = `ref_text_uid` (référence catalogue).
    pub value: String,
    /// Libellé d'affichage = `legal_text.title` du uid.
    pub label: String,
    /// Slug URL du texte (`legal_text.slug`) — token lisible côté MCP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub count: i64,
    #[serde(default)]
    pub articles: Vec<FacetChoice>,
}

/// Facettes de recherche : Juridiction (arbre) · Office · Domaine (arbre) ·
/// Solution · Publication · Date · Textes cités.
///
/// `juridiction` : niveau 1 = racines à valeur uid complet
/// (`jurisdiction_type:TJ`) ; niveau 2 = codes `jurisdiction` (`tj76351`,
/// `parent` = uid racine). Les autres facettes portent le suffixe d'uid
/// (`REJET`, `JEX`, `CIVIL_DROIT_LOCATIF`…).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchFacets {
    #[serde(default)]
    pub jurisdiction: Vec<FacetChoice>,
    /// Chambre (catégorie contrôlée `chamber:*`) — axe uniforme
    /// tous ordres, remplace le grain-chambre Cassation de la facette juridiction.
    #[serde(default)]
    pub chamber: Vec<FacetChoice>,
    #[serde(default)]
    pub office: Vec<FacetChoice>,
    #[serde(default)]
    pub legal_domain: Vec<FacetChoice>,
    #[serde(default)]
    pub solution: Vec<FacetChoice>,
    #[serde(default)]
    pub significance: Vec<FacetChoice>,
    #[serde(default)]
    pub publication: Vec<FacetChoice>,
    #[serde(default)]
    pub date_lecture_year: Vec<FacetChoice>,
    #[serde(default)]
    pub legal_instrument: Vec<LegalInstrumentFacet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchHit {
    pub id: String,
    pub jurisdiction_type: JurisdictionType,
    /// Code de cour précis (`jurisdiction:*`, ex. `tj_paris`) — token du
    /// filtre `jurisdiction_code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    pub title_html: String,
    /// Siège (chambre/formation/office) recomposé, rendu en 2ᵉ ligne discrète
    /// sous le titre — hors du `title_html` qui reste sur une ligne.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    pub score: f64,
    pub best_chunk: BestChunk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Tags référentiels résolus (clé + libellé).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<FacetTag>,
    /// Spécialisation de chambre (`chamber:*`), tag référentiel résolu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chamber: Option<FacetTag>,
    /// Catégorie de publication (`publication:*`, facette de place), tag résolu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<i64>,
    /// Résumé v4 embarqué en mode IA uniquement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchResponse {
    pub query: String,
    pub total: i64,
    pub hits: Vec<SearchHit>,
    pub query_mode: QueryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facets: Option<SearchFacets>,
    #[serde(default)]
    pub all_hit_ids: Vec<String>,
}

// ── Decision detail ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalReference {
    pub instrument: String,
    /// Slug du `legal_text` résolu (FK de citation) — présent quand la citation
    /// est ancrée au catalogue. Le front bâtit `/texte/{slug}/{numKey}`
    /// directement, sans re-slugifier. `None` ⇒ rendu brut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub articles: Vec<LegalRefArticle>,
}

/// Un article cité : `num` = libellé affiché (brut source) ; `numKey` = clé
/// canonique résolue (`legal_citation.ref_num_key`) pour le lien
/// `/texte/{slug}/{numKey}` — vide si l'article n'a pas été ancré au catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalRefArticle {
    pub num: String,
    #[serde(default)]
    pub num_key: String,
}

/// Cible d'une mention de citation : un article (ou un texte) résolu. `href`
/// pointe l'article (`/texte/{slug}/{numKey}`) ; `None` ⇒ citation non résolue.
/// `label` = libellé de la cible (titre/clé canonique).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitationTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    pub label: String,
}

/// Une mention cliquable d'une citation dans le corps d'un paragraphe.
/// `start`/`end` sont des offsets CODEPOINTS locaux au paragraphe rendu,
/// demi-ouverts `[start, end)`. `targets` porte une ou plusieurs cibles : une
/// citation multi-articles (« articles 1382, 1383 et 1384 du code civil »)
/// partage un seul span de texte mais vise N articles → le front rend un lien
/// simple pour 1 cible, un menu déroulant pour ≥2. Sur chevauchement de spans,
/// on fusionne en l'enveloppe et on unit les cibles (aucune n'est droppée).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitationSpan {
    pub start: usize,
    pub end: usize,
    pub targets: Vec<CitationTarget>,
}

/// Bloc de corps structuré d'une décision (renderer pur). `kind` peut se
/// répéter ; `anchor` est l'ancre DOM unique ; `label` = titre affiché.
///
/// `paragraphSpans` : aligné index-à-index sur `paragraphs`, porte par
/// paragraphe ses mentions de citation cliquables (vide = aucune ; champ absent
/// pour l'ancien fonds dont les offsets ne sont pas peuplés).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionSection {
    pub kind: String,
    pub anchor: String,
    pub label: String,
    pub paragraphs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paragraph_spans: Vec<Vec<CitationSpan>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionSourceXml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nom_jurisdiction_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numero_dossier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formation_jugement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_recours: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionDetail {
    pub id: String,
    pub jurisdiction_type: JurisdictionType,
    /// Titre lisible machine/stable « <juridiction>, <date ISO>, <n° rôle> ».
    pub title: String,
    pub paragraphs: Vec<String>,
    /// Mentions de citation cliquables alignées sur `paragraphs`, pour le rendu
    /// de repli sans sections autoritatives. Vide / absent si aucune.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paragraph_spans: Vec<Vec<CitationSpan>>,
    /// Sommaire/corps structuré autoritatif (None ⇒ heuristique front).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<Vec<DecisionSection>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Code de cour précis (`jurisdiction:*`, ex. `tj_paris`) — token du
    /// filtre `jurisdiction_code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    /// Tags référentiels résolus (clé + libellé).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<FacetTag>,
    /// Catégorie de publication (`publication:*`, facette de place), tag résolu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_audience: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Siège composé depuis les axes structurés : position de chambre qualifiée
    /// par le type de formation ou l'office — « pôle 5 — 3e chambre (formation à
    /// trois) ». Jamais une chaîne source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    /// Spécialisation de chambre (`chamber:*`) et type de formation
    /// (`formation:*`), tags référentiels résolus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chamber: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formation: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_references: Option<Vec<LegalReference>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_xml: Option<DecisionSourceXml>,
    /// Mots-clés de matière (themes Judilibre), du général au spécifique.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub themes: Vec<String>,
    /// Libellé brut de la nomenclature des affaires civiles (nac Judilibre).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nac: Option<String>,
    /// ECLI (identifiant européen stable et citable) si présent. Provenance/audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecli: Option<String>,
    /// Source de provenance autoritaire (`decision_sources.source` : `judilibre`,
    /// `opendata`, `dila-jade`, `cnda`…). Provenance/audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Chronologie de l'affaire : décisions chaînées par les liens
    /// appel/pourvoi/renvoi résolus, décision courante incluse, de la plus récente
    /// à la plus ancienne. Vide si la décision n'est pas chaînée.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chronology: Vec<ChronologyEntry>,
    /// Commentaires institutionnels : analyses officielles dépliées, lien vers les
    /// conclusions du rapporteur public, documents liés (rapports, avis,
    /// communiqués Cass). Non cherchables — enrichissement de la fiche, rendu en
    /// accordéon fermé en fin de page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<Commentaire>,
}

/// Commentaire institutionnel d'une décision : contenu porté localement déplié
/// sur place (`body`) ou ligne-lien externe (`url`). Trois formes : `analyse`
/// (body local, AJCE), `conclusions` (existence seule, lien composé vers
/// ArianeWeb), `note` (document lié — rapports/avis/communiqués Cass, notes de
/// doctrine — rendu comme lien titré vers l'éditeur).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Commentaire {
    /// `analyse` | `conclusions` | `note`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Date ISO du document commenté (date de lecture).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Libellé du lien (`note`) : type de document Cass, titre de la note…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Éditeur/source du lien (`Cour de cassation`, `GISTI`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// Accès : `libre` | `abonnes`. Absent = libre.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
    /// Rubriques du plan de classement (`code : libellé hiérarchique`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rubriques: Vec<String>,
    /// Renvois doctrinaux (`(1) Cf. CE, …`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renvois: Vec<String>,
    /// Lien externe — présent = la ligne se rend comme un lien sortant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Étape de la chronologie d'une affaire : une décision de la chaîne
/// appel/pourvoi/renvoi, identifiée par sa juridiction et sa date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChronologyEntry {
    pub id: String,
    /// Nom de la juridiction (repli : libellé du type).
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Vrai pour la décision affichée (rendue non cliquable).
    #[serde(default)]
    pub current: bool,
    /// Clé de solution de l'étape (`INFIRMATION`, `CONFIRMATION`, `REJET`…) :
    /// le sort d'une décision se lit dans la solution de celle qui la révise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    /// Numéros RG de l'étape — identifient sans ambiguïté quelle décision une
    /// infirmation/confirmation vise (les titres seuls prêtent à confusion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Nature du lien vers l'étape suivante (la décision attaquée, juste en
    /// dessous) : `APPEL_DE` | `POURVOI_CONTRE` | `RENVOI_APRES_CASSATION`.
    /// Absent quand la paire adjacente n'est pas directement liée (chaîne
    /// lacunaire, affaires sérielles) et sur la dernière étape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// Libellé FR d'une source de provenance (`decision_sources.source`). Un code
/// inconnu est renvoyé tel quel, par transparence d'audit. Pas un vocabulaire
/// de facette : c'est de la provenance, hors référentiel `facet_value`.
pub fn source_label(code: &str) -> &str {
    match code {
        "judilibre" => "Judilibre (Cour de cassation)",
        "opendata" => "Open Data des juridictions administratives",
        "dila-jade" => "DILA — JADE (juridictions administratives)",
        "dila-constit" => "DILA — Conseil constitutionnel",
        "dila-cass" => "DILA — Cour de cassation",
        "dila-capp" => "DILA — cours d'appel",
        "cedh" => "CEDH — HUDOC",
        "cjue" => "CJUE — EUR-Lex",
        "cnda" => "Cour nationale du droit d'asile",
        "dila-cnil" => "DILA — CNIL (délibérations)",
        other => other,
    }
}

/// Origine canonique du site (permaliens d'audit, URLs absolues). Source unique
/// partagée backend ↔ front : `lj-web` aligne sa `CANONICAL_BASE` dessus.
pub const SITE_BASE: &str = "https://librejustice.fr";

/// Permalien canonique LibreJustice d'une décision (`/decision/{id}`).
pub fn decision_permalink(id: &str) -> String {
    format!("{SITE_BASE}/decision/{id}")
}

/// Lignes de provenance/audit d'une décision — `(libellé, valeur)` dans l'ordre
/// d'affichage : source d'origine, ECLI, permalien LibreJustice. Builder pur
/// partagé par le bloc « Source » du web, l'export PDF et l'export DOCX —
/// contenu unique, rendu propre à chacun.
pub fn provenance_rows(detail: &DecisionDetail) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if let Some(src) = detail.source.as_deref().filter(|s| !s.is_empty()) {
        rows.push(("Source", source_label(src).to_string()));
    }
    if let Some(ecli) = detail.ecli.as_deref().filter(|s| !s.is_empty()) {
        rows.push(("ECLI", ecli.to_string()));
    }
    rows.push(("Permalien", decision_permalink(&detail.id)));
    rows
}

/// Décision proche — carte des volets « similaires » de la page décision.
/// L'ordre dans chaque volet est le rang de fusion des signaux (citations
/// directes, articles partagés IDF, KNN) : les jambes n'ont pas d'échelle de
/// score commune, aucun score n'est porté.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimilarDecisionHit {
    pub id: String,
    pub jurisdiction_type: JurisdictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Tags référentiels résolus (clé + libellé).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Rail « similaires » en deux volets : décisions de principe (référence,
/// similaires sur le fond — portée notable, graphe de citations + KNN
/// pré-filtré) et décisions d'espèce (similaires en fait — KNN générique).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimilarDecisionsResponse {
    pub decision_id: String,
    pub principe: Vec<SimilarDecisionHit>,
    pub espece: Vec<SimilarDecisionHit>,
    /// Vrai quand la source ne tranche aucune question de droit : les volets
    /// muets le sont par décision, pas faute de candidat assez proche.
    #[serde(default)]
    pub abstained: bool,
}

/// Prévisualisation légère d'une décision (hover card des liens de
/// jurisprudence) : identité + solution + codes de publication + résumé —
/// jamais le corps.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionPreview {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

// ── Référentiel LEGI versionné ──────────────────────────────────────────────

/// Une version d'article dans la timeline LEGI. `dateDebut`/`dateFin` ISO
/// `YYYY-MM-DD` ; `dateFin` absente ⇒ pas de fin (sentinelle normalisée NULL
/// à la frontière de parsing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawArticleVersion {
    pub legiarti: String,
    pub etat: String,
    pub date_debut: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_fin: Option<String>,
    /// Version future (`date_debut` > aujourd'hui, calculé côté API — l'horloge
    /// vit côté serveur) : badge « à venir » sur la frise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub upcoming: bool,
}

/// Défaut de `LawArticleResponse::translation` (droit FR LEGI = texte officiel).
fn default_officiel() -> String {
    "officiel".to_string()
}

/// Article LEGI servi à une date (law-at-date). `code` = `code_court` (slug).
/// `dateDebut` ISO requise ; `dateFin` absente ⇒ pas de fin.
/// `legifranceUrl` versionnée `/codes/article_lc/{LEGIARTI}/{date}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawArticleResponse {
    pub legiarti: String,
    pub legitext: String,
    pub code: String,
    /// Titre humain du texte (`legal_text.title`, ex. « Code de la famille
    /// sénégalais ») pour l'affichage « Article N du … ». `code` reste le slug
    /// d'URL ; sans ce champ le front retombe sur le slug humanisé (laid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_title: Option<String>,
    pub num: String,
    /// Clé canonique de l'article (`num_key`) pour les liens internes — l'URL
    /// `/texte/{code}/{numKey}` que le serve résout en lookup exact.
    /// `num` reste le libellé affiché.
    pub num_key: String,
    pub etat: String,
    pub date_debut: String,
    /// Provenance de la version servie (`legifrance` / `jorf` / `treaty`) — pour
    /// lier vers la section descriptive de `/sources`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titre_text: Option<String>,
    /// Fil d'Ariane TOC cliquable : les divisions enclosantes de l'article,
    /// de la racine à la section directe, `href` = vue-lecture de la section
    /// (`/texte/{code}/section/{cid}`). Vide quand la structure n'est pas ingérée
    /// (texte étranger, JORF) — le front retombe sur `titreText`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breadcrumb: Vec<LinkedTextRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_fin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte: Option<String>,
    /// Renvois cliquables du corps : spans codepoints demi-ouverts sur `texte`,
    /// même forme que côté décisions. Hrefs datés quand l'article est servi à date
    /// explicite — le renvoi navigue dans le même temps que la lecture.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texte_spans: Vec<CitationSpan>,
    /// Corps dans la langue d'origine, affiché en regard du FR si présent
    /// (couche vérification/vérité ; souvent absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte_original: Option<String>,
    /// Langue de `texteOriginal` (ISO-639-1, `ar`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang_original: Option<String>,
    /// Provenance du `texte` : `officiel` / `non_officiel` / `automatique`
    /// → badge UI. Défaut `officiel` (droit FR LEGI).
    #[serde(default = "default_officiel")]
    pub translation: String,
    /// Fraîcheur « as-of » effective : dernière base crédible que la copie reflète
    /// le droit en vigueur (ISO `YYYY-MM-DD`). Jamais inconnue (au pire la date de
    /// get). `legifrance`/`kali` : date du dernier sync ; sinon date de
    /// get/curation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_asof: Option<String>,
    /// Autorité du diffuseur, libellé FR (« source gouvernementale »,
    /// « agrégateur tiers », « traduction automatique »…). Axe distinct de
    /// `translation` (officialité du texte). Calculé via mapping pur
    /// `lj_core::source_authority`.
    pub source_authority: String,
    /// Lien « source » unique de l'article : page/PDF du diffuseur pour le curé
    /// (étranger, traités), ou page Légifrance versionnée pour un article natif
    /// LEGI/KALI (calculée côté API depuis l'identité). Absente seulement si aucune
    /// des deux n'est dérivable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Source secondaire amont : site qu'un agrégateur (jafbase) pointe. Rare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_upstream_url: Option<String>,
    /// Apparat éditorial affiché : la « Nota » officielle Légifrance (entrée en
    /// vigueur, QPC, applicabilité) ou la jurisprudence/doctrine + renvois « voir
    /// aussi » des éditions annotées curées. Rendu sous le corps, distinct du texte
    /// normatif. Absent si l'article n'en porte pas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nota: Option<String>,
    /// Première version future de l'article (`date_debut` > aujourd'hui, ISO,
    /// calculé côté API) : bandeau « sera modifié le … ». La sentinelle
    /// `2222-02-22` signifie « à une date à déterminer ».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upcoming_version_date: Option<String>,
    pub versions: Vec<LawArticleVersion>,
    /// Articles voisins pour la lecture en contexte : division enclosante ou
    /// fenêtre, l'article courant marqué `current`. Vide si pas de contexte
    /// exploitable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ArticleNeighbor>,
    /// Corps « dispositions modifiées » d'un article modificatif (ordonnance/loi
    /// de réforme), dérivé du graphe `legal_link` (bloc `<LIENS>` DILA) : cibles
    /// exactes par ID, liens garantis. Non vide ⇒ le front rend cette liste au lieu
    /// de `texte` (qui reste le résumé brut, illisible). Vide pour un article
    /// normal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifications: Vec<ArticleModification>,
    /// Textes qui ont modifié/créé/abrogé CET article (liens entrants du graphe) :
    /// l'historique « Modifié par », dans l'ordre du fichier source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified_by: Vec<LinkedTextRef>,
    /// Dispositions qui citent cet article (liens entrants CITATION).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cited_by: Vec<LinkedTextRef>,
    /// Commentaires de norme : doctrine ancrée sur cet article ou sur le texte
    /// entier, même forme que côté décision. Non cherchables — accordéon fermé en
    /// fin de page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<Commentaire>,
    /// Travaux parlementaires (zéro ingest) : une ligne par loi modificatrice de
    /// l'article, `href` composé
    /// `https://www.legifrance.gouv.fr/jorf/id/{JORFTEXT}` (bloc « Travaux
    /// préparatoires » et dossiers législatifs servis par Légifrance).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub travaux_parlementaires: Vec<LinkedTextRef>,
}

/// Opération d'un segment du comparateur de versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LawCompareOp {
    Equal,
    Insert,
    Delete,
}

/// Tronçon contigu du diff entre deux rédactions. Le texte d'un côté se
/// reconstruit en concaténant `equal`+`delete` (ancien) ou `equal`+`insert`
/// (nouveau) ; les sauts de ligne restent dans `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawCompareSegment {
    pub op: LawCompareOp,
    pub text: String,
}

/// Réponse du comparateur de versions d'un article : identité, les deux
/// versions comparées (métadonnées de frise), le diff en segments, et la
/// timeline complète pour les sélecteurs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawCompareResponse {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_title: Option<String>,
    pub num: String,
    pub num_key: String,
    pub from: LawArticleVersion,
    pub to: LawArticleVersion,
    pub segments: Vec<LawCompareSegment>,
    pub versions: Vec<LawArticleVersion>,
}

/// Une référence de texte/article liée par le graphe `legal_link` : libellé
/// verbatim DILA (« LOI n°2018-287 du 20 avril 2018 - art. 16 »), lien interne
/// quand la cible est en base, date de signature du texte si connue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkedTextRef {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// Article voisin pour le contexte de lecture : numéro + état, sans corps
/// (clic = navigation `/texte/{code}/{numKey}`). `current` = l'article de la
/// page. `numKey` = clé canonique pour le lien ; `num` = affichage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleNeighbor {
    pub num: String,
    pub num_key: String,
    pub etat: String,
    pub current: bool,
}

/// Une cible d'une disposition d'article modificatif (graphe `legal_link`) :
/// un article (numéro linkable, `href` garanti quand la cible est en base),
/// une division de code (`section`, non linkable), ou un texte entier
/// (`texte`, lié à son sommaire).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModificationItem {
    /// `article` (numéro dans `label`), `section` (titre de division) ou
    /// `texte` (libellé complet).
    pub kind: String,
    /// Numéro d'article (« 1302 », « L611-7 »), titre de section, ou libellé.
    pub label: String,
    /// Lien interne (`/texte/{code}/{numKey}` pour un article résolu,
    /// `/texte/{code}` pour un texte) ; absent si la cible n'est pas en base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

/// Un bloc du corps « dispositions modifiées » d'un article modificatif (graphe
/// `legal_link`). Une action portant sur un texte cible, avec ses dispositions.
/// Rendu en liste par le front.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleModification {
    /// `modifie` / `cree` / `abroge`.
    pub action: String,
    /// Nom affichable du code cible (« Code civil »).
    pub code: String,
    /// Lien interne vers le sommaire du code cible (`/texte/{code}`) quand résolu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_href: Option<String>,
    pub items: Vec<ModificationItem>,
}

/// Sommaire d'un code LEGI. `code` = `code_court` (slug).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawCodeSummary {
    pub legitext: String,
    pub code: String,
    pub titre: String,
    pub nature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derniere_modification: Option<String>,
    pub article_count: i64,
    /// Dates programmées de versions futures du texte (`VERSIONS_A_VENIR`
    /// DILA), ISO, triées — bandeau « sera modifié le … ». La sentinelle
    /// `2222-02-22` signifie « à une date à déterminer ».
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upcoming_versions: Vec<String>,
    /// Corps monolithique d'un texte sans articles (circulaires…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// État de diffusion du texte (`VIGUEUR`/`ABROGE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// NOR (familles qui en portent : circulaires, actes JO).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nor: Option<String>,
    /// Date de signature ISO (`date_texte`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_texte: Option<String>,
    /// Libellé de portée quand le texte relève de la doctrine administrative ;
    /// absent pour les normes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Fond du catalogue des normes — maillage retour vers `/normes/{fond}` ;
    /// absent pour un acte individuel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fond: Option<String>,
    /// Libellé du fond (« Lois et ordonnances »).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fond_label: Option<String>,
    /// Année de parcours du texte dans son fond (`/normes/{fond}/{annee}`) ;
    /// absente = bucket `sans-date`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fond_year: Option<i32>,
    /// Les articles les plus cités du texte entier (rebond de la colonne droite) —
    /// même palmarès que celui d'une division, au périmètre du code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_cited: Vec<TopCitedArticle>,
}

/// Décision citant un article LEGI, via `legal_citation`. `portee` = portée
/// jurisprudentielle (groupes de `publication_codes`) — ordre de la liste et
/// badge « Portée majeure/importante ».
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitingDecisionHit {
    pub id: String,
    pub title: String,
    pub jurisdiction_type: JurisdictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    pub significance: Significance,
    /// Résumé de la décision (première phrase affichée en carte, comme les
    /// décisions similaires). Absent tant que le cron résumé n'est pas passé.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Clés canoniques des articles cités parmi ceux interrogés — servi par
    /// les citantes d'une division, où la ligne doit dire par quel article la
    /// décision entre. Vide pour les citantes d'un article : la réponse est dans
    /// l'URL. Le front y résout les numéros affichables depuis la vue-lecture qu'il
    /// rend déjà.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cited_articles: Vec<String>,
}

/// Article « souvent cité avec » l'article de la page : co-citation dans
/// `legal_citation` sur un échantillon de décisions citantes, boilerplate
/// procédural exclu. `numKey` = numéro affichable et clé de lien ; `count` =
/// décisions de l'échantillon citant les deux articles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoCitedArticle {
    pub num_key: String,
    pub text_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    pub count: i64,
}

/// Un résultat de recherche d'article (`/recherche-textes`). `code` = slug du
/// texte parent (lien `/texte/{code}/{numKey}`) ; `codeTitle` = titre lisible du
/// code ; `titrePath` = fil d'Ariane ; `snippet` = extrait surligné du corps.
/// `numKey` = clé canonique pour le lien, `num` = affichage. `status` +
/// `dateDebut` (ISO) portent le badge d'état de la carte : libellé
/// [`article_status_label`] pour une version, « JO du <date> » quand
/// `source = "jorf"` (une publication n'a pas d'état).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleSearchHit {
    pub code: String,
    pub code_title: String,
    pub num: String,
    pub num_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titre_path: Option<String>,
    pub snippet: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_debut: Option<String>,
}

/// Réponse de `/api/search-textes` : hits paginés, total exact sous le
/// prédicat BM25 + filtres, et comptes par facette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleSearchResponse {
    pub hits: Vec<ArticleSearchHit>,
    pub total: i64,
    pub facets: ArticleSearchFacets,
}

/// Comptes par facette de la recherche d'articles, sous le même prédicat que les hits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleSearchFacets {
    /// Textes porteurs de hits : `value` = slug (`/texte/{slug}`), token du
    /// filtre `code`. Top-20 par count — l'énumération complète pèserait
    /// plusieurs Mo de JSON par réponse. Les autres axes et `total` restent
    /// exacts.
    #[serde(default)]
    pub code: Vec<FacetChoice>,
    pub jurisdiction: Vec<FacetChoice>,
    pub nature: Vec<FacetChoice>,
    pub source: Vec<FacetChoice>,
    /// Sur-facette « portée » : `norme` | `doctrine_administrative`, agrégée côté
    /// API depuis les buckets `nature` (mapping code, pas de colonne).
    #[serde(default)]
    pub scope: Vec<FacetChoice>,
}

/// Entrée du catalogue des codes (`/api/codes`). `code` = slug (lien `/texte/{code}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeCatalogueEntry {
    pub code: String,
    pub title: String,
    pub nature: String,
    pub jurisdiction: String,
    pub article_count: i64,
}

/// Réponse de `/api/codes` : liste des textes navigables du corpus. Le
/// périmètre `scope=head` (codes, constitutions, état civil — le fond `codes`
/// de l'arborescence des normes) est appliqué en SQL ; le reste du catalogue se
/// parcourt par année sous `/normes/{fond}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeCatalogueResponse {
    pub entries: Vec<CodeCatalogueEntry>,
}

/// Une juridiction du catalogue hub (`/api/juridictions`).
/// `code` = clé d'URL (`/juridiction/{code}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionHubEntry {
    pub code: String,
    pub label: String,
    pub decision_count: i64,
}

/// Une famille de juridictions du catalogue (`CA` → « Cours d'appel »),
/// dans l'ordre éditorial des hubs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionTypeGroup {
    pub jurisdiction_type: String,
    pub label: String,
    pub jurisdictions: Vec<JurisdictionHubEntry>,
}

/// Réponse de `/api/juridictions` : catalogue groupé par famille.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionCatalogueResponse {
    pub groups: Vec<JurisdictionTypeGroup>,
}

/// Compteur d'une année d'un hub juridiction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionYearCount {
    pub year: i32,
    pub count: i64,
}

/// Réponse de `/api/juridictions/{code}` : hub d'une juridiction — années
/// couvertes (plus récente en tête) et volume total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionHubResponse {
    pub code: String,
    pub label: String,
    pub jurisdiction_type: String,
    pub type_label: String,
    pub decision_count: i64,
    pub years: Vec<JurisdictionYearCount>,
}

/// Une décision d'une page hub juridiction×année : le lien crawlable
/// (`title` = ancre, `publicId` → `/decision/{publicId}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionHubDecision {
    pub public_id: String,
    pub title: String,
    pub date_lecture: String,
}

/// Réponse de `/api/juridictions/{code}/{annee}` : page paginée des décisions
/// d'une année (`total` = décisions de l'année, pagination par `page`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionYearResponse {
    pub code: String,
    pub label: String,
    pub year: i32,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
    pub decisions: Vec<JurisdictionHubDecision>,
}

/// Un fond du catalogue des normes (`/api/normes`). `fond` = clé d'URL
/// (`/normes/{fond}`) ; le fond `codes` renvoie vers le catalogue `/codes`
/// existant (pas de hub année).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormFondEntry {
    pub fond: String,
    pub label: String,
    pub text_count: i64,
}

/// Réponse de `/api/normes` : catalogue des fonds, dans l'ordre éditorial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormCatalogueResponse {
    pub fonds: Vec<NormFondEntry>,
}

/// Compteur d'une année d'un hub fond. `year = None` = bucket « sans date »
/// (URL `/normes/{fond}/sans-date`), toujours en dernier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormYearCount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    pub count: i64,
}

/// Réponse de `/api/normes/{fond}` : hub d'un fond — années couvertes (plus
/// récente en tête, bucket sans date en queue) et volume total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormFondResponse {
    pub fond: String,
    pub label: String,
    pub text_count: i64,
    pub years: Vec<NormYearCount>,
}

/// Un texte d'une page hub fond×année : le lien crawlable (`title` = ancre,
/// `slug` → `/texte/{slug}`). `date` absente sur le bucket sans date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormHubText {
    pub slug: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// Réponse de `/api/normes/{fond}/{annee}` : page paginée des textes d'une
/// année (`year = None` = bucket « sans date », `total` = textes du bucket).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormYearResponse {
    pub fond: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
    pub texts: Vec<NormHubText>,
}

/// Entrée de la table des matières d'un code. `numKey` = clé canonique, `num` =
/// affichage, `titlePath` = fil d'Ariane, `status` = état de l'article.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TocEntry {
    pub num: String,
    pub num_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_path: Option<String>,
    pub status: String,
}

/// Nœud du sommaire arborescent réel d'un texte, aplati en ordre de lecture —
/// le front reconstruit l'imbrication par `depth` (1 = premier niveau).
/// `kind` = `section` (avec `cid`, l'ancre stable `#{cid}` et la clé de la
/// vue-lecture `/texte/{code}/section/{cid}`) ou `article` (avec `numKey`,
/// le lien `/texte/{code}/{numKey}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TocNode {
    pub kind: String,
    pub depth: i32,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_key: Option<String>,
    pub etat: String,
}

/// Taille maximale (en articles) d'une vue-lecture servie/matérialisée en
/// place : seuil de la vue-lecture intégrale des textes courts côté API
/// (`reading` du sommaire) et borne d'éligibilité d'une division à l'accordéon
/// de lecture côté front. Une seule règle, récursive — le texte entier n'est
/// que la division racine.
pub const INLINE_READING_MAX: usize = 150;

/// Réponse de `/api/texte/{code}/sommaire` : table des matières d'un code.
/// `tree` = l'arbre structurel réel daté quand le texte en a un ; il prime sur
/// `entries` (sommaire à plat par `titlePath`, servi seulement quand `tree` est
/// vide — textes sans structure ingérée). `reading` = vue-lecture intégrale
/// (corps joints) servie à la place des `entries` pour les textes courts ;
/// `tree` reste servi pour le rail « Plan du texte ».
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeTocResponse {
    pub entries: Vec<TocEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tree: Vec<TocNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reading: Vec<LawSectionItem>,
}

/// Item de la vue-lecture d'une section : sous-arbre en ordre de lecture.
/// `kind` = `section` (intertitre, `cid` pour l'ancre) ou `article` (corps
/// `texte`/`nota` joints ; `numKey` → lien vers la page article).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawSectionItem {
    pub kind: String,
    pub depth: i32,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_key: Option<String>,
    pub etat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nota: Option<String>,
}

/// Référence d'une division du texte : fil d'Ariane et navigation bloc
/// précédent / suivant de la vue-lecture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawSectionRef {
    pub cid: String,
    pub label: String,
}

/// Article du palmarès « les plus cités » d'un périmètre : une division pour
/// `/section/{cid}`, le texte entier pour `/texte/{code}`, classés par
/// `citing_decision_counts`. `numKey` = clé canonique de l'article, `label` =
/// son numéro affichable (formaté par le front comme la vue-lecture), `href` =
/// sa page. `count` = décisions citantes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopCitedArticle {
    pub num_key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    pub count: i64,
}

/// Texte citant une division, agrégé au grain du texte citant et non de son
/// article : un arrêté qui vise trois articles de la division sort une fois,
/// `articleCount` à 3. C'est ce compte qui classe la liste, et le seuil de deux
/// articles qui la débruite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SectionCitingText {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    pub article_count: i64,
}

/// Réponse de `/api/texte/{code}/section/{cid}` : vue-lecture d'une section —
/// les articles de la division rendus à la suite, intertitres des sous-sections
/// inclus. `code` = slug du texte, `title` = titre de la section (porté par
/// l'arête parente). `ancestors` = divisions englobantes (de la racine au
/// parent) ; `prev`/`next` = bloc précédent / suivant en ordre de lecture (hors
/// sous-arbre de la section). `topCited` = les articles les plus cités du
/// sous-arbre ; `citingTexts` = les textes qui citent la division, au centre
/// sous les décisions citantes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawSectionResponse {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_title: Option<String>,
    pub cid: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ancestors: Vec<LawSectionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<LawSectionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<LawSectionRef>,
    pub items: Vec<LawSectionItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_cited: Vec<TopCitedArticle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citing_texts: Vec<SectionCitingText>,
}

// ── Compte utilisateur ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserProfile {
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Timestamp RFC 3339.
    pub created_at: String,
    pub last_seen_at: String,
    pub track_activity: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserProfileUpdate {
    /// Longueur 1 à 80.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Bascule du mode d'enregistrement d'activité. `false` purge aussi les
/// données existantes (recherches, lectures, signets).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityTrackingUpdate {
    pub enabled: bool,
}

// ── Signets ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BookmarkItem {
    pub id: String,
    pub title: String,
    pub jurisdiction_type: JurisdictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Solution résolue (clé + libellé référentiel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub bookmarked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BookmarksResponse {
    pub items: Vec<BookmarkItem>,
    pub total: i64,
}

// ── Historique de recherche ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchHistoryEntry {
    pub id: i64,
    pub query: String,
    /// Payload de filtres opaque.
    pub filters: serde_json::Value,
    pub source: ActivitySource,
    /// Moteur interrogé : route le relancement de l'entrée
    /// (`/decisions?q=` vs `/textes?q=`).
    pub engine: SearchEngine,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchHistoryResponse {
    pub items: Vec<SearchHistoryEntry>,
    pub total: i64,
}

// ── Décisions consultées ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionViewItem {
    pub id: String,
    pub title: String,
    pub jurisdiction_type: JurisdictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Solution résolue (clé + libellé référentiel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub view_count: i64,
    pub last_source: ActivitySource,
    pub last_viewed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionViewsResponse {
    pub items: Vec<DecisionViewItem>,
    pub total: i64,
}

/// Préfixe un titre d'instrument par « du / de la / de l' » selon son premier
/// mot : élision devant voyelle (« de l'Ordonnance n° 2016-131 », « de
/// l'Arrêté »), féminins usuels des natures de textes (« de la Loi »), sinon
/// « du » (« du Code civil », « du Décret »). Sert les titres « Article N … »
/// (page /texte, hover card, MCP).
pub fn instrument_with_de(name: &str) -> String {
    let first = name.split_whitespace().next().unwrap_or("").to_lowercase();
    if first
        .chars()
        .next()
        .is_some_and(|c| "aeiouéèêë".contains(c))
    {
        return format!("de l'{name}");
    }
    const FEMININS: &[&str] = &[
        "loi",
        "constitution",
        "convention",
        "décision",
        "directive",
        "déclaration",
        "délibération",
        "charte",
    ];
    if FEMININS.contains(&first.as_str()) {
        format!("de la {name}")
    } else {
        format!("du {name}")
    }
}

/// Libellé FR d'un état DILA de version d'article (`legal_article.status`) :
/// les états différés (`*_DIFF` = décidé mais pas encore effectif) et les cas
/// de chronique (mort-né, disjoint) deviennent lisibles à l'écran. État inconnu
/// ou vide → l'appelant retombe sur la valeur brute. Vit ici (pas dans
/// `lj-core`) : consommé par le front WASM comme par l'API — même statut que
/// [`instrument_with_de`].
pub fn article_status_label(status: &str) -> Option<&'static str> {
    Some(match status {
        "VIGUEUR" => "En vigueur",
        "MODIFIE" => "Modifié",
        "ABROGE" => "Abrogé",
        "PERIME" => "Périmé",
        "REMPLACE" => "Remplacé",
        "TRANSFERE" => "Transféré",
        "DEPLACE" => "Déplacé",
        "ANNULE" => "Annulé",
        "DENONCE" => "Dénoncé",
        "DISJOINT" => "Disjoint",
        "VIGUEUR_DIFF" => "Entrée en vigueur différée",
        "ABROGE_DIFF" => "Abrogation différée",
        "MODIFIE_MORT_NE" => "Modifié, jamais entré en vigueur",
        _ => return None,
    })
}

// ── Fiche entité ────────────────────────────────────────────────────────────

/// Fiche d'une entité du référentiel (`entity`) : identité registre
/// + agrégats contentieux dérivés de `decision_party`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityPageResponse {
    pub header: EntityHeaderDto,
    pub stats: EntityStatsDto,
}

/// Identité registre. `namespace` = préfixe de l'uid (`siren` | `rna` |
/// `cnb` | `oacc`) ; `nature` = `physique` | `morale_privee` |
/// `morale_publique`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityHeaderDto {
    /// Uid complet namespacé (ex. `siren:552043002`).
    pub uid: String,
    pub namespace: String,
    pub nature: String,
    /// Dénomination courante.
    pub denomination: String,
    pub sigle: Option<String>,
    pub forme: Option<String>,
    pub active: bool,
    /// Dénominations datées (la courante incluse), ordre chronologique.
    pub denominations: Vec<EntityDenominationDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDenominationDto {
    pub denomination: String,
    /// Dates ISO `YYYY-MM-DD` ; `None` = borne ouverte.
    pub date_debut: Option<String>,
    pub date_fin: Option<String>,
}

/// Agrégats contentieux d'une entité sur le corpus de décisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityStatsDto {
    pub decision_count: i64,
    pub as_applicant: i64,
    pub as_defendant: i64,
    /// Décisions par année, ordre chronologique.
    pub by_year: Vec<EntityYearCountDto>,
    /// Décisions par juridiction, décroissant.
    pub by_jurisdiction: Vec<EntityKeyCountDto>,
    /// Conseils (avocats/cabinets) observés aux côtés de l'entité, décroissant.
    pub top_counsel: Vec<EntityCounselDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityYearCountDto {
    pub year: i32,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityKeyCountDto {
    /// Clé machine (ex. code juridiction).
    pub key: String,
    /// Libellé d'affichage.
    pub label: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityCounselDto {
    /// Uid registre du conseil si lui-même résolu (`cnb:…`, `oacc:…`).
    pub uid: Option<String>,
    pub name: String,
    pub count: i64,
}

/// Décisions citant l'entité, paginées, plus récentes d'abord. Les items
/// portent les mêmes hits que la recherche (rendu unifié `ResultCard`) plus le
/// rôle de l'entité.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDecisionsResponse {
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
    pub items: Vec<EntityDecisionHitDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDecisionHitDto {
    pub hit: SearchHit,
    /// Côté de l'entité dans cette décision (`applicant` | `defendant`).
    pub side: Option<String>,
    /// Qualité de l'entité (`party` | `law_firm` | `counsel_name`).
    pub quality: String,
}

// ── Autocomplétion ──────────────────────────────────────────────────────────

/// Suggestions d'autocomplétion (`GET /suggest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuggestResponse {
    /// Nombre de mots de fin de query que chaque suggestion remplace (contexte
    /// re-suggéré inclus — le mot en cours de frappe compte pour un).
    pub matched_tokens: u32,
    pub suggestions: Vec<String>,
}

// ── Annuaire des entités ────────────────────────────────────────────────────

/// Résultat de recherche d'entités (annuaire — registre complet).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntitySearchResponse {
    pub items: Vec<EntityDirectoryItemDto>,
}

/// Listing annuaire paginé d'une catégorie — registre complet, trié par
/// contentieux décroissant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDirectoryResponse {
    /// Lignes paginables du filtre courant (registre entier de la catégorie,
    /// ou sous-ensemble barreau).
    pub total: i64,
    /// Dont entités avec ≥ 1 décision liée (même filtre).
    pub contentieux: i64,
    pub page: i64,
    pub page_size: i64,
    pub items: Vec<EntityDirectoryItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDirectoryItemDto {
    /// Uid complet namespacé (ex. `siren:552043002`) → fiche `/entite/{ns}/{id}`.
    pub uid: String,
    pub namespace: String,
    pub denomination: String,
    pub nature: String,
    pub forme: Option<String>,
    pub active: bool,
    /// Slug barreau (avocats `cnb:` uniquement, extrait de l'uid).
    pub barreau: Option<String>,
    /// Nombre de décisions liées (source du tri).
    pub decision_count: i64,
}

/// Compteurs d'une catégorie de l'annuaire : total du registre chargé et
/// entités avec au moins une décision liée.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnuaireCategorieStatsDto {
    pub registre: i64,
    pub contentieux: i64,
}

/// Compteurs de l'annuaire par catégorie (page d'accueil annuaire).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnuaireStatsResponse {
    pub entreprises: AnnuaireCategorieStatsDto,
    pub personnes_publiques: AnnuaireCategorieStatsDto,
    pub associations: AnnuaireCategorieStatsDto,
    pub avocats: AnnuaireCategorieStatsDto,
    pub cabinets: AnnuaireCategorieStatsDto,
}

/// Encart « Parties » d'une décision : acteurs extraits, liés au registre
/// quand résolus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionPartiesResponse {
    pub parties: Vec<DecisionPartyDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionPartyDto {
    pub quality: String,
    pub side: Option<String>,
    /// Verbatim extrait de la décision.
    pub value: String,
    pub nature: Option<String>,
    /// Slug officiel CNB du barreau (counsel uniquement).
    pub barreau: Option<String>,
    /// Uid registre si résolu → fiche `/entite/{ns}/{id}`.
    pub entity_uid: Option<String>,
}

// ── Fiche entité — volet registre servi par APIs externes ───────────────────

/// Volet registre d'une fiche entité, servi à l'affichage par les APIs
/// publiques (recherche-entreprises, BODACC/JOAFE Opendatasoft) — aucun stock
/// local. Chaque section absente (API indisponible, entité hors périmètre)
/// est simplement `None`/vide : le rendu dégrade sans erreur.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityRegistreResponse {
    /// Identité enrichie + dirigeants + finances (entreprises `siren:` seulement).
    pub entreprise: Option<RegistreEntrepriseDto>,
    /// Les annonces de l'entité, tous fonds confondus et groupés : le live DILA
    /// (BODACC pour `siren:`, JOAFE pour `rna:`) et nos lignes `annonce` par rôle
    /// d'arête, projetés dans le même `AnnonceHit`. Vide = aucune annonce, ou
    /// amont indisponible.
    pub annonces: Vec<EntityAnnonceGroupDto>,
    /// Liens sortants (annuaire-entreprises, documents INPI…).
    pub liens: Vec<RegistreLienDto>,
}

/// Identité registre d'une entreprise (recherche-entreprises.api.gouv.fr —
/// données SIRENE + RNE).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistreEntrepriseDto {
    pub siege_adresse: Option<String>,
    /// Code NAF/APE de l'activité principale (ex. `62.01Z`).
    pub activite_naf: Option<String>,
    pub date_creation: Option<String>,
    /// Libellé de la tranche d'effectif salarié (ex. « 20 à 49 salariés »).
    pub effectif: Option<String>,
    pub dirigeants: Vec<RegistreDirigeantDto>,
    /// Années comptables publiées, plus récente d'abord.
    pub finances: Vec<RegistreFinanceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistreDirigeantDto {
    /// Nom affichable (personne physique « Prénom NOM », morale : dénomination).
    pub nom: String,
    pub qualite: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistreFinanceDto {
    pub annee: String,
    pub chiffre_affaires: Option<i64>,
    pub resultat_net: Option<i64>,
}

/// Lien sortant vers la source officielle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistreLienDto {
    pub label: String,
    pub url: String,
}

// ── Annonces — pot des événements juridiques en cours ───────────────────────

/// Les fonds du pot, dans l'ordre d'affichage. Chacun a son backend (table
/// `annonce` pour `marche`, proxy ODS DILA pour `bodacc` et `joafe`) mais tous
/// se projettent dans le même hit : le fond est un axe de facette, pas un
/// moteur séparé.
pub const ANNONCE_FOND_MARCHE: &str = "marche";
pub const ANNONCE_FOND_BODACC: &str = "bodacc";
pub const ANNONCE_FOND_JOAFE: &str = "joafe";

/// Libellé public d'un fond. `None` = jeton inconnu, que la frontière HTTP
/// refuse plutôt que de le laisser filtrer dans le vide.
pub fn annonce_fond_label(fond: &str) -> Option<&'static str> {
    match fond {
        ANNONCE_FOND_MARCHE => Some("Marchés publics"),
        ANNONCE_FOND_BODACC => Some("Vie des entreprises"),
        ANNONCE_FOND_JOAFE => Some("Journal officiel des associations"),
        _ => None,
    }
}

/// Ce que le fond nomme l'acteur porté par la ligne — l'acheteur d'une
/// consultation, l'entreprise d'une annonce BODACC. Un pot, un champ, un
/// libellé par fond.
pub fn annonce_acteur_role(fond: &str) -> &'static str {
    match fond {
        ANNONCE_FOND_BODACC => "Entreprise",
        ANNONCE_FOND_JOAFE => "Association",
        _ => "Acheteur",
    }
}

/// Ce fond porte-t-il une échéance ? Un marché ouvre un délai de dépôt ; une
/// annonce BODACC ou une parution JOAFE constate un fait déjà survenu. Un fond
/// sans échéance n'a rien à mettre dans un agenda, et on ne lui propose donc
/// pas de flux ICS.
pub fn annonce_fond_a_echeance(fond: &str) -> bool {
    fond == ANNONCE_FOND_MARCHE
}

/// Tri de la liste d'annonces. `echeance` est le défaut : le produit répond à
/// « qu'est-ce qui ferme bientôt chez moi ». `nouveautes` trie par première
/// apparition dans nos canaux — le seul curseur fiable pour les moteurs
/// scrapés, donc l'ordre du flux d'alerte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnonceSort {
    Echeance,
    Publication,
    Nouveautes,
}

/// Filtre de la fenêtre servie par `/annonces` — facettes seulement (ni BM25
/// ni embeddings), `q` est un filtre par mot sur le titre et l'acheteur.
/// `closes` élargit à l'archive (les lignes fermées restent servies : assiette
/// du « qui achète quoi »).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceSearchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Fonds retenus — un axe de facette comme les autres. Vide = tous les
    /// fonds, servis groupés : deux backends n'ont pas de score commun à
    /// comparer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fond: Vec<String>,
    /// Codes département canoniques (`75`, `2A`, `973`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub departement: Vec<String>,
    /// Suffixes d'uid `nature_marche:*` (`TRAVAUX`, `SERVICES`…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub famille: Vec<String>,
    /// Divisions CPV — suffixes d'uid `cpv:*` (`45`, `72`…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpv: Vec<String>,
    /// Fenêtre d'échéance en jours (`7`, `30`) : ne garde que ce qui ferme
    /// dans l'intervalle. Les lignes sans échéance connue en sortent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echeance_jours: Option<u32>,
    /// Inclut les annonces closes (défaut : la fenêtre vivante seule).
    #[serde(default)]
    pub closes: bool,
    #[serde(default = "default_annonce_sort")]
    pub sort: AnnonceSort,
    #[serde(default = "default_annonce_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

fn default_annonce_sort() -> AnnonceSort {
    AnnonceSort::Echeance
}
fn default_annonce_limit() -> u32 {
    20
}

impl Default for AnnonceSearchRequest {
    fn default() -> Self {
        Self {
            q: None,
            fond: Vec::new(),
            departement: Vec::new(),
            famille: Vec::new(),
            cpv: Vec::new(),
            echeance_jours: None,
            closes: false,
            sort: default_annonce_sort(),
            limit: default_annonce_limit(),
            offset: 0,
        }
    }
}

/// Une annonce en liste, quel que soit son fond : la projection commune où une
/// consultation de marché et une annonce BODACC se lisent dans la même ligne.
/// Le libellé d'acteur est la vérité affichable — le rattachement au graphe des
/// entités est faillible et vit à part (`entites` du détail).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceHit {
    pub uid: String,
    pub fond: String,
    pub fond_label: String,
    pub titre: String,
    /// Qui l'annonce concerne : l'acheteur d'une consultation, l'entreprise
    /// d'une annonce BODACC. Vide quand la source ne le publie pas.
    pub acteur: String,
    /// Comment ce fond nomme cet acteur (« Acheteur », « Entreprise »).
    pub acteur_role: String,
    /// Code département canonique, puis son nom résolu par l'API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub departement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub departement_label: Option<String>,
    /// Famille d'avis du fond : nature du marché (`nature_marche:*`), famille
    /// BODACC (procédure collective, création, radiation…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub famille: Option<FacetTag>,
    /// Division CPV — axe propre au fond `marche`, absent partout ailleurs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpv: Option<FacetTag>,
    /// Date de parution ISO `YYYY-MM-DD`.
    pub date_publication: String,
    /// Date limite de remise des offres, RFC 3339. Absente = inconnue de toutes
    /// les sources — la ligne n'entre alors jamais dans la fenêtre vivante.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echeance: Option<String>,
    /// Consultation clôturée : échéance passée.
    #[serde(default)]
    pub closed: bool,
    /// Marché attribué : un avis de résultat est arrivé. Implique `closed` — un
    /// marché attribué ne prend plus d'offres. Les trois états du marché se lisent
    /// dans cet ordre : attribué, sinon clos, sinon ouvert.
    #[serde(default)]
    pub attribue: bool,
    /// Où agir : la page de la consultation sur la plateforme de l'acheteur,
    /// le PDF officiel d'une annonce BODACC.
    pub url_action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<String>,
    /// Première apparition dans nos canaux (RFC 3339) — le curseur des flux
    /// d'alerte : les moteurs scrapés n'ont pas de date de publication fiable et
    /// le BOAMP republie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen_at: Option<String>,
    /// Canaux qui publient l'annonce (`boamp`, `atexo`…) — la preuve de la
    /// couverture unifiée, et l'aveu du contraire quand il n'y en a qu'un.
    #[serde(default)]
    pub canaux: Vec<String>,
}

/// Un axe de la barre de filtres. La liste des axes est déclarée par le
/// backend du fond et non figée dans le contrat : on n'offre jamais un filtre
/// que la source ne sait pas exécuter — le CPV n'existe que pour les marchés,
/// la fenêtre d'échéance que pour ce qui a une échéance. `key` est la clé
/// d'URL, donc aussi celle du paramètre d'API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceFacetAxis {
    pub key: String,
    pub label: String,
    pub choices: Vec<FacetChoice>,
}

/// Les annonces d'un fond. La vue par défaut en empile une par fond plutôt que
/// d'entrelacer : deux backends n'ont pas de score commun à comparer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceFondGroup {
    pub fond: String,
    pub label: String,
    pub total: i64,
    pub hits: Vec<AnnonceHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceSearchResponse {
    /// Somme des totaux de fond.
    pub total: i64,
    /// Un groupe par fond interrogé, dans l'ordre du vocabulaire. Un seul fond
    /// filtré ⇒ un seul groupe, que le front rend en liste simple.
    pub groupes: Vec<AnnonceFondGroup>,
    pub sort: AnnonceSort,
    /// Barre de filtres pour l'état courant : chaque axe est compté sans son
    /// propre filtre (multi-sélection : cocher un département n'efface pas les
    /// autres du décompte), les autres filtres appliqués.
    #[serde(default)]
    pub axes: Vec<AnnonceFacetAxis>,
}

/// Un lien profond de la consultation, par usage : l'avis, le règlement de la
/// consultation, le dossier. Aucun fichier n'est stocké — ce sont des liens
/// sortants vers la plateforme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceLienDto {
    /// `avis` | `rc` | `dce`.
    pub kind: String,
    pub label: String,
    pub url: String,
    pub moteur: String,
}

/// Un canal de publication de l'annonce. `firstSeenAt` est le curseur des
/// alertes ; la présence de la ligne dit à elle seule que le canal publie
/// encore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceCanalDto {
    pub moteur: String,
    pub source_uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_avis: Option<String>,
    pub first_seen_at: String,
}

/// Une entité du graphe rattachée à l'annonce, par rôle (`acheteur`,
/// `titulaire`…). `href` pointe la fiche entité.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceEntiteDto {
    pub role: String,
    pub label: String,
    pub href: String,
}

/// Un lot attribué d'un résultat de marché. Le SIREN n'est porté que par les
/// avis eForms ; `href` pointe la fiche entité quand il existe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceLotDto {
    pub titulaire: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub montant: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devise: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nb_offres: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_attribution: Option<String>,
}

/// L'attribution d'un marché : le même objet que la consultation, à son
/// dernier moment. Sa présence est ce qui rend `AnnonceHit::attribue` vrai.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceResultatDto {
    #[serde(default)]
    pub lots: Vec<AnnonceLotDto>,
    /// Le paragraphe d'attribution des avis simplifiés, servi tel quel : la
    /// source y nomme l'attributaire sans rien structurer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte: Option<String>,
    /// L'avis de résultat au BOAMP, quand la consultation et son attribution
    /// n'ont pas la même page source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avis_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceDetailResponse {
    pub annonce: AnnonceHit,
    #[serde(default)]
    pub liens: Vec<AnnonceLienDto>,
    #[serde(default)]
    pub canaux_detail: Vec<AnnonceCanalDto>,
    #[serde(default)]
    pub entites: Vec<AnnonceEntiteDto>,
    /// Le résultat du marché — porté par la fiche d'une attribution, et par
    /// celle d'une consultation dont le résultat est paru.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resultat: Option<AnnonceResultatDto>,
}

/// Une entrée du catalogue par département (page SSR crawlable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceDepartementEntry {
    pub code: String,
    pub label: String,
    pub count: i64,
}

/// Catalogue `/annonces` : le stock ouvert ventilé par département, plus les
/// natures — les chemins de crawl du fond.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceCatalogueResponse {
    pub fond: String,
    /// Annonces ouvertes, tous départements (les lignes sans département
    /// incluses).
    pub total: i64,
    pub departements: Vec<AnnonceDepartementEntry>,
    #[serde(default)]
    pub familles: Vec<FacetChoice>,
}

/// Un groupe d'annonces sur la fiche entité. La clé est un fond quand
/// l'origine est le live DILA (`bodacc`, `joafe`), un rôle d'arête quand la
/// ligne est à nous (`acheteur` = marchés en cours, `titulaire` = marchés
/// remportés) : ce qui distingue deux groupes, c'est ce que l'entité est dans
/// chacun. `total` est le compte réel, `items` la tête de liste.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityAnnonceGroupDto {
    pub key: String,
    pub label: String,
    pub total: i64,
    pub items: Vec<AnnonceHit>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn instrument_with_de_contracte_selon_le_premier_mot() {
        assert_eq!(
            instrument_with_de("Ordonnance n° 2016-131 du 10 février 2016"),
            "de l'Ordonnance n° 2016-131 du 10 février 2016"
        );
        assert_eq!(instrument_with_de("Code civil"), "du Code civil");
        assert_eq!(
            instrument_with_de("Loi n° 2018-287 du 20 avril 2018"),
            "de la Loi n° 2018-287 du 20 avril 2018"
        );
        assert_eq!(instrument_with_de("Décret n° 94-46"), "du Décret n° 94-46");
        assert_eq!(
            instrument_with_de("Arrêté du 12 mai"),
            "de l'Arrêté du 12 mai"
        );
        assert_eq!(instrument_with_de("Constitution"), "de la Constitution");
    }

    #[test]
    fn search_mode_camel_and_lowercase_roundtrip() {
        assert_eq!(
            serde_json::to_string(&SearchMode::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::to_string(&SortOrder::DateDesc).unwrap(),
            "\"date_desc\""
        );
        assert_eq!(
            serde_json::to_string(&QueryMode::Hybrid).unwrap(),
            "\"hybrid\""
        );
        assert_eq!(
            serde_json::to_string(&ActivitySource::Mcp).unwrap(),
            "\"mcp\""
        );
    }

    #[test]
    fn search_request_camel_aliases_and_defaults() {
        let req: SearchRequest = serde_json::from_str(r#"{"query":"bail commercial"}"#).unwrap();
        assert_eq!(req.query, "bail commercial");
        assert_eq!(req.mode, SearchMode::Auto);
        assert_eq!(req.sort, SortOrder::Relevance);
        assert_eq!(req.limit, 20);
        assert_eq!(req.offset, 0);
        assert!(!req.ai_mode);
        assert!(req.jurisdiction_type.is_none());

        let body = json!({
            "query": "x",
            "jurisdictionType": ["CA", "TJ"],
            "dateFrom": "2020-01-01",
            "aiMode": true
        });
        let req: SearchRequest = serde_json::from_value(body).unwrap();
        assert_eq!(
            req.jurisdiction_type,
            Some(vec![JurisdictionType::Ca, JurisdictionType::Tj])
        );
        assert_eq!(req.date_from.as_deref(), Some("2020-01-01"));
        assert!(req.ai_mode);
    }

    #[test]
    fn search_request_forbids_unknown_fields() {
        let err = serde_json::from_str::<SearchRequest>(r#"{"query":"x","bogus":1}"#);
        assert!(err.is_err());
    }

    #[test]
    fn search_hit_serializes_camel_case() {
        let hit = SearchHit {
            id: "ce-1".into(),
            jurisdiction_type: JurisdictionType::Ce,
            jurisdiction_code: Some("ce".into()),
            jurisdiction_name: Some("Conseil d'État".into()),
            title_html: "<b>x</b>".into(),
            seat: None,
            score: 1.5,
            best_chunk: BestChunk {
                chunk_index: 0,
                snippet: "…".into(),
            },
            date_lecture: Some("2024-08-06".into()),
            docket_numbers: Some(vec!["12345".into()]),
            solution: Some(FacetTag {
                key: "REJET".into(),
                label: "Rejet".into(),
            }),
            procedure: None,
            office: None,
            legal_domain: None,
            chamber: None,
            publication: None,
            publication_codes: vec!["B".into()],
            chars: Some(4200),
            summary: None,
        };
        let v = serde_json::to_value(&hit).unwrap();
        assert_eq!(v["jurisdictionType"], "CE");
        assert_eq!(v["titleHtml"], "<b>x</b>");
        assert_eq!(v["bestChunk"]["chunkIndex"], 0);
        assert_eq!(v["solution"]["key"], "REJET");
        assert_eq!(v["solution"]["label"], "Rejet");
        assert!(v.get("summary").is_none());
        assert!(v.get("procedure").is_none());
    }

    #[test]
    fn facet_choice_parent_optional() {
        let flat: FacetChoice =
            serde_json::from_str(r#"{"value":"REJET","label":"Rejet","count":3}"#).unwrap();
        assert_eq!(flat.parent, None);
        let child = FacetChoice {
            value: "tj76351".into(),
            label: "Le Havre".into(),
            count: 2,
            parent: Some("jurisdiction_type:TJ".into()),
        };
        let v = serde_json::to_value(&child).unwrap();
        assert_eq!(v["parent"], "jurisdiction_type:TJ");
        assert!(serde_json::to_value(&flat).unwrap().get("parent").is_none());
    }

    #[test]
    fn legi_article_response_serializes_camel_case() {
        let art = LawArticleResponse {
            upcoming_version_date: None,
            breadcrumb: Vec::new(),
            legiarti: "LEGIARTI000006832947".into(),
            legitext: "LEGITEXT000006070721".into(),
            code: "code-civil".into(),
            code_title: Some("Code civil".into()),
            num: "L131-4".into(),
            num_key: "L131-4".into(),
            etat: "VIGUEUR".into(),
            date_debut: "1992-05-15".into(),
            source: "legifrance".into(),
            titre_text: Some("Livre Ier > Titre III".into()),
            date_fin: None,
            texte: Some("Le contrat…".into()),
            texte_spans: vec![],
            texte_original: None,
            lang_original: None,
            translation: "officiel".into(),
            source_asof: Some("2026-06-28".into()),
            source_authority: "source gouvernementale".into(),
            source_url: Some(
                "https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006832947/1992-05-15"
                    .into(),
            ),
            source_upstream_url: None,
            nota: Some("Conformément à la loi n° 2019-222…".into()),
            versions: vec![LawArticleVersion {
                upcoming: false,
                legiarti: "LEGIARTI000006832947".into(),
                etat: "MODIFIE".into(),
                date_debut: "1804-03-15".into(),
                date_fin: Some("1992-05-14".into()),
            }],
            context: Vec::new(),
            modifications: Vec::new(),
            modified_by: Vec::new(),
            cited_by: Vec::new(),
            commentaires: Vec::new(),
            travaux_parlementaires: Vec::new(),
        };
        let v = serde_json::to_value(&art).unwrap();
        assert_eq!(v["legiarti"], "LEGIARTI000006832947");
        assert_eq!(v["dateDebut"], "1992-05-15");
        assert_eq!(v["titreText"], "Livre Ier > Titre III");
        assert_eq!(
            v["sourceUrl"],
            "https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006832947/1992-05-15"
        );
        assert_eq!(v["versions"][0]["dateFin"], "1992-05-14");
        assert!(v.get("dateFin").is_none());
        assert_eq!(v["nota"], "Conformément à la loi n° 2019-222…");
    }

    #[test]
    fn citing_decision_hit_serializes_camel_case() {
        let hit = CitingDecisionHit {
            id: "cass-1".into(),
            title: "Cour de cassation, 2024-01-10, 22-12.345".into(),
            jurisdiction_type: JurisdictionType::Cc,
            date_lecture: Some("2024-01-10".into()),
            significance: Significance::Majeure,
            summary: None,
            cited_articles: vec!["748-6".into(), "748-7".into()],
        };
        let v = serde_json::to_value(&hit).unwrap();
        assert_eq!(v["jurisdictionType"], "CC");
        assert_eq!(v["dateLecture"], "2024-01-10");
        assert_eq!(v["significance"], "MAJEURE");
        assert_eq!(v["citedArticles"][1], "748-7");
    }
}

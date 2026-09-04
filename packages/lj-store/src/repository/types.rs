//! Types publics du repository : structs/enums de contrat (lignes de résultat,
//! bundles d'écriture, statuts d'upsert). Réexportés tels quels par [`super`].

use chrono::NaiveDate;
use lj_core::decision::Decision;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertStatus {
    Created,
    Updated,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpsertResult {
    pub id: i64,
    pub status: UpsertStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingDecisionState {
    pub id: i64,
    pub source_uid: String,
    pub content_checksum: String,
    pub has_embeddings: bool,
    pub public_id: Option<String>,
}

/// Une ligne du backfill summary. `date_lecture` est rendu en `text` côté SQL
/// (chaîne ISO ou `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingSummaryRow {
    pub decision_id: i64,
    pub public_id: String,
    pub jurisdiction_type: String,
    pub jurisdiction_name: Option<String>,
    pub date_lecture: Option<String>,
    pub docket_numbers: Option<Vec<String>>,
}

/// Corps + métadonnées grain décision d'une décision poolée pour le GT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GtDoc {
    pub id: i64,
    pub public_id: String,
    pub jurisdiction_name: String,
    pub jurisdiction_type: String,
    pub date_lecture: String,
    pub search_title: String,
    pub full_text: String,
}

/// Un fichier sitemap prêt à publier en base (`sitemaps`). `body` est le
/// contenu servi tel quel (`.xml` brut ou `.xml.gz` déjà gzippé).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitemapRow {
    pub filename: String,
    pub content_type: String,
    pub body: Vec<u8>,
    pub lastmod: NaiveDate,
}

/// Ligne `legal_text` (catalogue des textes de loi). `text_uid` = identité
/// globalement unique (LEGITEXT/JORFTEXT/CELEX/EU…), `source` hors identité.
/// `slug` = code court pour l'URL. `title_key` = `normalize_instrument(title)`,
/// posé côté Rust à l'upsert. `date_texte` = date du texte (par quoi on cite) ;
/// `date_publi` = publication JO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalTextRow {
    pub text_uid: String,
    pub jurisdiction: String,
    pub title: String,
    pub title_key: String,
    pub nature: String,
    pub last_modified: Option<NaiveDate>,
    pub date_texte: Option<NaiveDate>,
    pub date_publi: Option<NaiveDate>,
    /// Identité canonique diffuseur-agnostique, cascade `eli → nor →
    /// instrument_key`. `eli` = `<ID_ELI>` ; `nor` = `<NOR>` ; `instrument_key` =
    /// `nature|date|num`. Tous nullables : un texte peut n'en porter aucun (codes :
    /// identité = slug). Servent le collapse des manifestations LEGI/JORF d'un même
    /// acte en une `legal_text`.
    pub eli: Option<String>,
    pub nor: Option<String>,
    pub instrument_key: Option<String>,
    /// Corps monolithique d'un texte SANS articles numérotés (circulaires,
    /// réponses ministérielles…). `None` pour les familles à articles.
    pub body: Option<String>,
    /// État de diffusion du texte lui-même (`VIGUEUR`/`ABROGE`). `None` = non
    /// renseigné.
    pub status: Option<String>,
}

/// Texte sans slug, avec de quoi alimenter la cascade de désambiguïsation de la
/// passe d'assignation. `date_texte` déjà rendue `YYYY-MM-DD`.
#[derive(Debug, Clone)]
pub struct SlugSourceRow {
    pub text_uid: String,
    pub title: String,
    pub jurisdiction: String,
    pub date_texte: Option<String>,
    pub nor: Option<String>,
}

/// One side of a pair of `EU/` fiches whose uids differ only by the order of the
/// number and the year. The CELEX its articles carry says which of the two names
/// the act; the count says which holds the body, and it is not always the same.
#[derive(Debug, Clone)]
pub struct EuTwin {
    pub text_uid: String,
    pub celex: Option<String>,
    pub articles: i64,
}

/// Ligne `legal_article` (une version datée d'un article de loi).
/// IDENTITÉ = `(text_uid, num_key, date_debut)`. PROVENANCE par version :
/// `source` le fournisseur (legifrance/jorf/gisti/onu…), `source_uid` l'identifiant
/// natif (LEGIARTI/JORFARTI/`treaty/…`), `source_url` l'URL **non**
/// template-dérivable (curé, nullable). `position` = ordre de lecture réel
/// (≠ tri lexical 26<26-1<26-2). `content_checksum` xxh3-64 en `BIGINT`
/// (cast `i64::from_ne_bytes`). `date_debut` `None` côté write → sentinelle
/// '0001-01-01' (borne ouverte) à l'upsert ; en lecture la colonne est NOT NULL.
/// `date_fin` `None` = pas de fin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalArticleRow {
    pub text_uid: String,
    pub num: String,
    pub num_key: String,
    pub position: Option<i32>,
    pub title_path: Option<String>,
    pub status: String,
    pub date_debut: Option<NaiveDate>,
    pub date_fin: Option<NaiveDate>,
    pub texte: Option<String>,
    /// Corps dans la langue d'origine, si disponible. `BM25` ne couvre que `texte`
    /// (FR).
    pub texte_original: Option<String>,
    /// Langue de `texte_original` (ISO-639-1, `ar`…). `None` si pas d'original.
    pub lang_original: Option<String>,
    /// Provenance de `texte` : `officiel` / `non_officiel` / `automatique`.
    pub translation: String,
    pub nota: Option<String>,
    pub content_checksum: u64,
    pub source: String,
    pub source_uid: String,
    pub source_url: Option<String>,
    /// Date « as-of » de fraîcheur : dernière base dont cette copie reflète le droit
    /// en vigueur. `None` pour les sources *live* (legifrance/kali/jorf), dont la
    /// fraîcheur se dérive de `ingest_freshness`.
    pub source_asof: Option<NaiveDate>,
    /// Source secondaire amont : le site qu'un agrégateur (jafbase) pointe.
    pub source_upstream_url: Option<String>,
}

/// Propriétaire d'arêtes `legal_link` : la version d'article dont le XML porte
/// le bloc `<LIENS>`, ou le texte lui-même (`num_key` vide, `date_debut` `None`
/// → sentinelle '0001-01-01' à l'écriture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalLinkOwner {
    pub text_uid: String,
    pub num_key: String,
    pub date_debut: Option<NaiveDate>,
}

/// Ligne `legal_link` : miroir fidèle d'un `<LIEN>` DILA. `typelien` brut +
/// famille `verb` normalisée + `direction` vue de l'owner (`outgoing` = il
/// agit/cite, `incoming` = il subit/est cité). Cible en clé pendante (IDs DILA),
/// `target_num_key` = `normalize_article(target_num)` posé au bord ingest.
/// `seq` = ordre du fichier, implicite (position dans le `Vec` du propriétaire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalLinkRow {
    pub typelien: String,
    pub verb: String,
    pub direction: String,
    pub target_kind: String,
    pub target_uid: Option<String>,
    pub target_text_uid: Option<String>,
    pub target_num: Option<String>,
    pub target_num_key: Option<String>,
    pub target_nature: Option<String>,
    pub target_label: String,
    pub target_date: Option<NaiveDate>,
    pub target_nor: Option<String>,
}

/// Une arête `legal_link` lue pour affichage, cible résolue au read-time :
/// `target_text_slug`/`target_text_title` = le texte porteur de la cible s'il est
/// en base ; `resolved_slug`/`resolved_num_key` = la version d'article cible
/// résolue (par `source_uid`, sinon par `(texte, num_key)`) — `Some` ⇒ le lien
/// `/texte/{resolved_slug}/{resolved_num_key}` est garanti.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLegalLink {
    pub typelien: String,
    pub verb: String,
    pub direction: String,
    pub target_kind: String,
    pub target_num: Option<String>,
    pub target_nature: Option<String>,
    pub target_label: String,
    pub target_date: Option<NaiveDate>,
    /// Uid du texte porteur de la cible (JORFTEXT d'une loi modificatrice…) —
    /// compose le lien travaux parlementaires.
    pub target_text_uid: Option<String>,
    pub target_text_slug: Option<String>,
    pub target_text_title: Option<String>,
    pub resolved_slug: Option<String>,
    pub resolved_num_key: Option<String>,
    /// Cible section : cid stable résolu via `legal_toc_edge` — `Some` ⇒ l'ancre
    /// `/texte/{target_text_slug}#{cid}` existe au sommaire.
    pub resolved_section_cid: Option<String>,
}

/// Propriétaire d'arêtes `legal_toc_edge` : le cid du texte (fichier
/// `texte/struct`) ou l'ID de version d'une section (`section_ta`). `text_uid` =
/// texte porteur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocOwner {
    pub owner_uid: String,
    pub text_uid: String,
}

/// Ligne `legal_toc_edge` : un enfant (article ou section) tel que listé par son
/// propriétaire, fenêtre `[date_debut, date_fin)` (sentinelles de fin absorbées
/// côté parser ; un `date_debut` sentinelle reste une vraie date → exclu de tout
/// filtrage daté). `seq` implicite (position dans le `Vec`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEdgeRow {
    pub child_kind: String,
    pub child_uid: String,
    pub child_cid: Option<String>,
    pub child_num_key: Option<String>,
    pub label: String,
    pub etat: String,
    pub date_debut: Option<NaiveDate>,
    pub date_fin: Option<NaiveDate>,
    pub niv: Option<i32>,
}

/// Un nœud de l'arbre structurel aplati en ordre de lecture (CTE récursive) :
/// `depth` = profondeur (1 = premier niveau du texte).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocTreeRow {
    pub depth: i32,
    pub child_kind: String,
    pub child_uid: String,
    pub child_cid: Option<String>,
    pub child_num_key: Option<String>,
    pub label: String,
    pub etat: String,
}

/// Un item de vue-lecture d'une section : les nœuds du sous-arbre en ordre de
/// lecture, corps (`texte`/`nota`) joint pour les articles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocReadingRow {
    pub depth: i32,
    pub child_kind: String,
    pub child_cid: Option<String>,
    pub child_num_key: Option<String>,
    pub label: String,
    pub etat: String,
    pub texte: Option<String>,
    pub nota: Option<String>,
}

/// Une décision citant un article de référentiel (backlink `legal_citation`).
/// Champs bruts, convertis en `CitingDecisionHit` côté `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitingDecisionRow {
    pub id: i64,
    pub public_id: String,
    pub jurisdiction_type: String,
    pub jurisdiction_name: Option<String>,
    pub date_lecture: Option<String>,
    pub docket_numbers: Option<Vec<String>>,
    /// Codes de publication source (`b`, `A`…), dont `lj-api` tire le libellé.
    pub publication_codes: Option<Vec<String>>,
    /// Résumé de la décision (carte citante, première phrase côté front).
    pub summary: Option<String>,
    /// Articles cités **parmi ceux interrogés**, quand la question portait sur
    /// plusieurs. Vide pour les citantes d'un article.
    pub cited_num_keys: Vec<String>,
}

/// Un article co-cité avec l'article de la page dans les décisions
/// (croisement `legal_citation`). `count` = décisions de l'échantillon citant les
/// deux ; `text_slug` `None` = texte sans page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoCitedArticleRow {
    pub num_key: String,
    pub count: i64,
    pub text_slug: Option<String>,
    pub text_title: String,
}

/// Un hit de recherche plein-texte d'article (`/recherche-textes`).
/// `slug` = slug du `legal_text` parent (lien `/texte/{slug}/{num}`) ; `texte` =
/// corps brut pour le snippet. `score` = `paradedb.score`.
#[derive(Debug, Clone, PartialEq)]
pub struct ArticleSearchRow {
    pub text_uid: String,
    pub slug: Option<String>,
    pub code_title: String,
    pub num: String,
    pub num_key: String,
    pub title_path: Option<String>,
    pub status: String,
    pub source: String,
    /// Début de la version servie — la date du « JO du … » quand
    /// `source = 'jorf'`. `None` sur les membres texte/conteneur.
    pub date_debut: Option<NaiveDate>,
    pub texte: Option<String>,
    pub score: f32,
}

/// Filtres du parcours d'articles, aux axes de la barre `/textes`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArticleBrowseFilters<'a> {
    pub text_uid: Option<&'a str>,
    pub jurisdiction: Option<&'a str>,
    pub nature: Option<&'a str>,
    pub source: Option<&'a str>,
    /// `(liste, true)` = nature dans la liste, `(liste, false)` = hors liste.
    pub nature_set: Option<(&'a [String], bool)>,
    pub at: Option<NaiveDate>,
}

/// Un hit du banc de ranking articles :
/// identité publique du doc (`slug` + `num`, `num` vide = jambe « textes à
/// corps ») + matière de jugement (titre, chemin de division, corps).
#[derive(Debug, Clone, PartialEq)]
pub struct ArticleRankHit {
    pub slug: String,
    pub num: String,
    pub code_title: String,
    pub title_path: Option<String>,
    pub texte: Option<String>,
}

/// Mode de la jambe titre du banc de ranking articles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArticleTitleMode {
    /// OR boosté ×4.
    Or,
    /// Conjonctif par alternative (requête nue / expansion) boosté ×4.
    Conj,
    /// Conjonctif ×4 + OR ×1.
    OrConj,
}

/// Paramètres de la fusion RRF inter-jambes du banc articles : chaque jambe
/// est classée seule (top-`leg_limit` par score BM25), puis fusionnée par
/// `w_jambe / (k + rang)` — `txt_weight` pondère les jambes « textes à
/// corps » (1.0 = interleave strict). `split_title` scinde titre et corps en
/// jambes séparées (4 jambes : art-titre, art-corps, txt-titre, txt-corps,
/// scores RRF sommés par doc) au lieu des 2 jambes à prédicat mixte
/// titre+corps. `container` ajoute une jambe « conteneurs » : `legal_text`
/// sans corps mais à articles (les codes), titre en match conjonctif SEUL — un
/// hit `slug` sans `num` (lien `/texte/{slug}`), prioritaire à rang RRF égal.
/// `foreign_weight` scinde la jambe articles en domestique (jurisdiction
/// FR/UE/INTL + pays nommés dans la requête,
/// [`lj_core::jurisdictions::query_jurisdictions`]) et étrangère, pondérée
/// par ce poids. `container_alias` admet dans la jambe conteneurs les
/// expansions d'alias **pleine requête**
/// ([`lj_core::aliases::whole_query_expansions`] : « code civil du sénégal »
/// → « code de la famille sénégalais »). `country_leg` ajoute une jambe
/// « pays nommé » : articles des juridictions nommées, requête débarrassée
/// des tokens pays ([`lj_core::jurisdictions::strip_query_jurisdictions`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArticleRrf {
    pub k: f64,
    pub txt_weight: f64,
    pub leg_limit: i64,
    pub split_title: bool,
    pub container: bool,
    pub foreign_weight: Option<f64>,
    pub container_alias: bool,
    pub country_leg: bool,
    /// Fusionne la jambe étrangère dans la jambe articles par SCORE
    /// (`score × foreign_weight`) au lieu de l'interleave par rang. Sans effet si
    /// `foreign_weight` est `None`.
    pub foreign_score_merge: bool,
    /// Jambe « termes d'usage » optionnelle (poids RRF) : sacs de n-grammes
    /// des contextes de citation (`scratch_usage_terms`) — le vocabulaire que les
    /// décisions emploient autour d'un article, absent de sa lettre (« frais
    /// irrépétibles » → art. 700 CPC).
    pub usage_weight: Option<f64>,
    /// Table des sacs interrogée par la jambe usage (défaut :
    /// `legal_article_usage`).
    pub usage_table: Option<&'static str>,
}

/// Un comptage de facette de recherche d'articles : valeur de la facette
/// (jurisdiction/nature/source) et nombre de hits sous le même prédicat BM25 + filtres.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetCount {
    pub value: String,
    pub count: i64,
}

/// Total + facettes d'une recherche d'articles, calculés en UNE requête
/// `GROUPING SETS` sous le même prédicat BM25 + filtres que la recherche.
/// `code` (slug) / `jurisdiction` / `nature` portés par `legal_text` (`nature`
/// normalisée `upper()`), `source` par `legal_article`. Chaque axe trié count
/// décroissant puis valeur ascendante.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleSearchStats {
    pub total: i64,
    /// Slugs des textes porteurs de hits — token du filtre `code` (`/texte/{slug}`).
    pub code: Vec<FacetCount>,
    pub jurisdiction: Vec<FacetCount>,
    pub nature: Vec<FacetCount>,
    pub source: Vec<FacetCount>,
}

/// Volet banc des renvois du référentiel : accord des spans du corps
/// (`text_legal_citation`, notre extraction) avec ce que la source DÉCLARE citer
/// (`legal_link`), au grain (article citant → article cité).
///
/// Deux mesures indépendantes. La **couverture** dit quelle part des renvois
/// déclarés est rendue cliquable dans le texte ; ce qu'elle laisse dehors se lit
/// dans les deux `missing_*`. L'**accord de code** ne porte que sur les couples
/// où les deux sources nomment le même numéro d'article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRefsOracleStats {
    /// Couples déclarés par la source (dédoublonnés).
    pub declared: i64,
    /// Dont un span du corps porte exactement le même couple.
    pub with_span: i64,
    /// Déclarés sans span, la cible étant le texte porteur (renvoi interne).
    pub missing_internal: i64,
    /// Déclarés sans span, la cible étant un tiers.
    pub missing_external: i64,
    /// Spans dont le numéro cité est déclaré depuis le même article.
    pub compared: i64,
    /// Dont la cible est PARMI les textes déclarés pour ce numéro.
    pub agree: i64,
    /// Clés où la source déclare plusieurs textes pour un même numéro :
    /// l'oracle s'y abstient, l'extracteur garde la main.
    pub ambiguous: i64,
}

/// Une entrée du catalogue des codes (`/api/codes`) : un `legal_text` à slug +
/// nombre d'articles en vigueur. `text_uid` = identité globale, `slug` = code court de
/// l'URL `/texte/{slug}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalTextCatalogRow {
    pub text_uid: String,
    pub slug: String,
    pub title: String,
    pub nature: String,
    pub jurisdiction: String,
    pub article_count: i64,
}

/// Un candidat de catalogue proposé pour une valeur d'instrument inconnue
/// (`suggest_instruments`). Le titre sert le message « did you mean », le slug
/// et l'uid servent le rattrapage quand le candidat EST le texte demandé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentCandidate {
    pub slug: String,
    pub title: String,
    pub text_uid: String,
}

/// Issue d'une résolution de nom de texte (`resolve_instrument`). `Rattrape`
/// nomme la règle qui a tranché : l'appelant la journalise, le banc la compte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstrumentResolution {
    /// Le slug demandé est au catalogue, tel quel ou par alias d'usage.
    Direct { slug: String, text_uid: String },
    /// Un candidat unique porte le texte demandé, sous une autre graphie.
    Rattrape {
        slug: String,
        text_uid: String,
        regle: lj_core::instrument_match::Rule,
    },
    /// Aucune cible unique : les candidats partent dans l'erreur corrective.
    Inconnu {
        candidates: Vec<InstrumentCandidate>,
    },
}

/// Une entrée de la table des matières d'un code : version en vigueur par
/// `num_key`, sans corps (clic = navigation `/texte/{slug}/{num}`).
/// `position` = ordre de lecture réel (≠ tri lexical).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocArticleRow {
    pub num: String,
    pub num_key: String,
    pub title_path: Option<String>,
    pub status: String,
    pub position: Option<i32>,
    /// Fenêtre de la version rendue close : l'article ne s'applique plus.
    pub expired: bool,
}

/// Un article voisin pour le contexte de lecture : numéro + état, sans corps
/// (clic = navigation `/texte/{slug}/{num}`). `current` marque l'article de la
/// page courante.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleNeighborRow {
    pub num: String,
    pub num_key: String,
    pub status: String,
    pub current: bool,
}

/// Une version d'article dans la timeline. Dates en ISO `String`
/// (`::text` côté SQL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LawVersionRow {
    pub source_uid: String,
    pub status: String,
    pub date_debut: Option<String>,
    pub date_fin: Option<String>,
}

/// Sommaire d'un code avec le décompte d'articles en vigueur.
/// `last_modified` en ISO `String`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LawCodeSummaryRow {
    pub text_uid: String,
    pub slug: Option<String>,
    pub title: String,
    pub nature: String,
    pub last_modified: Option<String>,
    pub article_count: i64,
    /// Dates programmées de versions futures du texte (`VERSIONS_A_VENIR`) ;
    /// peut contenir la sentinelle `2222-02-22` (date inconnue).
    pub upcoming_versions: Vec<NaiveDate>,
    /// Corps monolithique d'un texte sans articles.
    pub body: Option<String>,
    /// État de diffusion (`VIGUEUR`/`ABROGE`).
    pub status: Option<String>,
    pub nor: Option<String>,
    /// Date de signature ISO.
    pub date_texte: Option<String>,
}

/// Un chunk prêt à insérer dans `decision_chunks`. `embedding` peut être `None`
/// (la colonne `rabitq8(1024)` accepte NULL).
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkWrite {
    pub chunk_index: i32,
    pub char_start: i32,
    pub char_end: i32,
    pub body: String,
    pub embedding: Option<Vec<f32>>,
}

/// Une occurrence de citation prête à écrire dans `legal_citation` : span token
/// en codepoints sur `decisions.full_text` + cible catalogue inline (`None` =
/// non lié). Livrées par décision TRIÉES par `char_start`, sans doublon ni
/// chevauchement — la PK `(decision_id, char_start)` en dépend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationOccurrenceRow {
    pub char_start: i32,
    pub char_end: i32,
    pub ref_text_uid: Option<String>,
    pub ref_num_key: Option<String>,
    /// Span porteur de « et suivants » : la cible désigne une famille d'articles à
    /// partir de l'ancre — expansée dans les arrays de facettes via
    /// `_suivants_family`.
    pub suivants: bool,
}

/// Métadonnées catalogue d'un `legal_text` pour le banc offline :
/// `(title_key, slug, num)`. `num` = numéro d'acte en tête de titre (`Décret n°
/// 2000-1093 …` → `2000-1093`, `None` si non numéroté).
pub type LegalTextMeta = (String, Option<String>, Option<String>);

/// Hubs de crawl pour les sitemaps : par clé de hub (code juridiction, fond),
/// les buckets `(année, nombre de pages de listing)`.
pub type JurisdictionHubPages = Vec<(String, Vec<(i32, i64)>)>;
/// Le pendant côté normes, dont le bucket est un token (`2026`, `sans-date`).
pub type NormHubPages = Vec<(String, Vec<(String, i64)>)>;

/// Champs structurés extraits d'une décision, prêts à persister. Construits à
/// l'ingest (`lj_ingest::extract`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractedFields {
    pub date_lecture: Option<NaiveDate>,
    pub date_audience: Option<NaiveDate>,
    pub docket_numbers: Vec<String>,
    /// Formation structurée : clé canonique du siège
    /// ([`lj_core::chamber::Seat`]) + uid référentiel `formation:*`
    /// (FK `facet_value`).
    pub chamber_key: Option<String>,
    pub formation_uid: Option<String>,
    /// Column `decisions.seat` (jsonb): what the source wrote, never resolved,
    /// the resolution reaching only the paths derived from it.
    pub seat: Option<lj_core::chamber::Seat>,
    /// Column `decisions.seat_paths` (text[]), unfolded ancestors included. The
    /// court code in it is the canonical one, never the source's.
    pub seat_paths: Vec<String>,
    /// Nom d'usage porté par la source de la décision (docname CEDH, ligne de
    /// titre EUR-Lex) : colonne `decisions.case_name`, écrite avec
    /// `case_name_source = 'source'`.
    pub case_name: Option<String>,
    /// Titre canonique persistant : composé à l'ingest via `lj_core::titles`
    /// (juridiction, siège, date FR, nom d'usage, premier numéro) : colonne
    /// `decisions.search_title`, champ indexé BM25.
    pub search_title: Option<String>,
    pub publication_codes: Vec<String>,
    /// Uids référentiels — FK vers `facet_value`.
    pub solution_uid: Option<String>,
    pub procedure_uid: Option<String>,
    pub office_uid: Option<String>,
    pub legal_domain_uid: Option<String>,
    /// Matrice NER {applicant, defendant} × {counsel_names, law_firms,
    /// companies}.
    pub applicant_counsel_names: Vec<String>,
    pub applicant_law_firms: Vec<String>,
    pub applicant_companies: Vec<String>,
    pub defendant_counsel_names: Vec<String>,
    pub defendant_law_firms: Vec<String>,
    pub defendant_companies: Vec<String>,
    /// Intervenants (rôle intervenant), sans côté.
    pub intervenors: Vec<String>,
    /// Thèmes Judilibre verbatim (`source_fields->'themes'`).
    pub themes: Vec<String>,
    /// Ligne du référentiel `jurisdiction` portée par la décision : le code
    /// devient `decisions.jurisdiction_code` (FK), la ligne est créée à la
    /// volée (`ON CONFLICT DO NOTHING`) par le chemin d'écriture.
    pub jurisdiction: Option<JurisdictionRow>,
    /// Occurrences de citations à plat, `None` si non extraites.
    pub citation_occurrences: Option<Vec<CitationOccurrenceRow>>,
    /// Liens de chronologie, `None` si non extraits.
    pub decision_links: Option<Vec<DecisionLinkRow>>,
    /// Citations de jurisprudence, `None` si non extraites.
    pub case_citations: Option<Vec<CaseCitationRow>>,
    /// Acteurs au grain `decision_party` — la relation canonique dont les 7
    /// cellules NER plates ci-dessus sont la projection. `None` si non extraits.
    pub parties: Option<Vec<DecisionPartyRow>>,
}

/// Ligne `decision_party` prête à écrire : un acteur de la décision — qualité ×
/// côté × valeur, clé de résolution pliée, nature (`None` = moteur muet) et
/// spans-évidences en tableaux parallèles (codepoints sur `full_text` ; vides =
/// provenance métadonnée sans occurrence corps). Miroir store de
/// `lj_extract::parties::ActorRow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionPartyRow {
    /// `party` | `law_firm` | `counsel_name`.
    pub quality: String,
    /// `applicant` | `defendant` | `None`.
    pub side: Option<String>,
    pub value: String,
    pub resolve_key: String,
    /// `physique` | `morale_privee` | `morale_publique`.
    pub nature: Option<String>,
    /// Slug officiel du barreau en apposition (`counsel_name`).
    pub barreau: Option<String>,
    /// `substituant` | `substitue` | `postulant` | `plaidant` (`counsel_name`) —
    /// `None` = aucun marqueur.
    pub role: Option<String>,
    /// `ord` de la ligne `law_firm` apposée au conseil dans la même décision
    /// (`counsel_name`) — `None` = aucune apposition lisible.
    pub firm_ord: Option<i32>,
    /// `ord` de la ligne partie que ce conseil représente : l'arête dont `side`
    /// est dérivé.
    pub party_ord: Option<i32>,
    /// `ord` de la ligne conseil substituée, posée sur le `substituant`.
    pub sub_ord: Option<i32>,
    pub char_starts: Vec<i32>,
    pub char_ends: Vec<i32>,
}

/// Ligne `case_citation` prête à écrire : span (offsets codepoints, token
/// identifiant) + clé pendante par famille (`cc|1823954`, `cjue|c-561/19`,
/// `rg|{jurisdiction_code}|21/04532`, `af|{jurisdiction_code}|1906041` fond
/// administratif TA/CAA…). Livrées par décision TRIÉES par `char_start`, sans
/// chevauchement — la PK `(decision_id, char_start)` en dépend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseCitationRow {
    pub char_start: i32,
    pub char_end: i32,
    pub target_ref: String,
}

/// Ligne `text_case_citation` : un texte/article du référentiel cite une
/// décision. Émetteur = article `(owner_num_key, owner_date_debut)` ou, si les
/// deux sont `None`, le corps `legal_text.body` du texte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextCaseCitationRow {
    pub owner_num_key: Option<String>,
    pub owner_date_debut: Option<NaiveDate>,
    pub char_start: i32,
    pub char_end: i32,
    /// Clé pendante par famille, même format que `case_citation`.
    pub target_ref: String,
}

/// Ligne `text_legal_citation` : un texte/article du référentiel renvoie à un
/// article de norme. Émetteur = article `(owner_num_key, owner_date_debut)` ou,
/// si les deux sont `None`, le corps `legal_text.body`. Toujours résolue
/// (`ref_text_uid` du catalogue) ; `ref_num_key` `None` = mention nue du texte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLegalCitationRow {
    pub owner_num_key: Option<String>,
    pub owner_date_debut: Option<NaiveDate>,
    pub char_start: i32,
    pub char_end: i32,
    pub ref_text_uid: String,
    pub ref_num_key: Option<String>,
}

/// Span de renvoi résolu d'une version d'article servie, prêt à composer côté
/// API : offsets codepoints sur le `texte` émetteur + cible jointe au catalogue
/// (slug pour le href, titre pour le libellé).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleCitationSpanRow {
    pub char_start: i32,
    pub char_end: i32,
    pub ref_slug: Option<String>,
    pub ref_num_key: Option<String>,
    pub ref_title: String,
    /// La fiche cible est lisible — ≥ 1 article, ou un corps monolithique
    /// (circulaires, traités/TI, BOFiP) : gate du lien « mention nue » vers
    /// `/texte/{slug}`.
    pub ref_readable: bool,
}

/// Ligne `decision_links` prête à écrire : type de lien + clé pendante
/// `canonical_ref` de la décision attaquée. Miroir store de
/// `lj_extract::chrono::PriorRef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionLinkRow {
    /// `APPEL_DE` | `POURVOI_CONTRE` | `RENVOI_APRES_CASSATION`.
    pub link_type: String,
    pub target_ref: String,
}

/// Une juridiction du catalogue hub `/juridictions` : ligne du référentiel +
/// volume de décisions actives.
#[derive(Debug, Clone)]
pub struct JurisdictionHubRow {
    pub code: String,
    pub jurisdiction_type: String,
    pub label: String,
    pub decision_count: i64,
}

/// Une décision d'une page hub juridiction×année : le strict nécessaire pour un
/// lien crawlable (`search_title` = ancre).
#[derive(Debug, Clone)]
pub struct HubDecisionRow {
    pub public_id: String,
    pub title: String,
    pub date_lecture: String,
}

/// Un texte d'une page hub fond×année du catalogue des normes : le strict
/// nécessaire pour un lien crawlable (`title` = ancre).
#[derive(Debug, Clone)]
pub struct NormTextRow {
    pub slug: String,
    pub title: String,
    /// Date de parcours (`date_texte` valide, sinon `date_publi` valide) —
    /// absente sur les pages « sans date ».
    pub date: Option<String>,
}

/// Ligne du référentiel `jurisdiction`. Miroir store de
/// `lj_extract::facets::JurisdictionRef`. `code` = code canonique user-facing
/// (`tj_paris`) ; `source_code` = code de la source (location Judilibre
/// `tj75056`), clé de résolution à l'ingest et des snapshots d'extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JurisdictionRow {
    pub code: String,
    pub source_code: String,
    pub jurisdiction_type: String,
    pub city: Option<String>,
    pub label: String,
}

/// Ligne du référentiel `chamber` : une chambre sous sa cour. Le libellé n'y
/// est pas stocké, c'est une fonction pure des deux autres colonnes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChamberRow {
    pub jurisdiction_code: String,
    pub chamber_key: String,
}

/// Ligne du référentiel `facet_value` : uid namespacé (`solution:REJET`…),
/// libellé FR, hiérarchie (`parent_uid`) et ordre d'affichage (`sort` du seed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetValueRow {
    pub uid: String,
    pub facet: String,
    pub label: String,
    pub abbr: Option<String>,
    pub parent_uid: Option<String>,
    pub sort: i32,
    /// Ce que la valeur recouvre quand elle en regroupe plusieurs.
    pub hint: Option<String>,
}

// ── Fiche entité (lecture) ───────────────────────────────────────────────────

/// Identité registre d'une entité (`entity`) pour l'en-tête de fiche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityHeaderRow {
    pub uid: String,
    pub nature: String,
    pub denomination: String,
    pub sigle: Option<String>,
    pub forme: Option<String>,
    pub active: bool,
}

/// Une dénomination datée d'une entité (`entity_denomination`). Dates rendues en
/// ISO `String` (`::text` côté SQL) ; `None` = borne ouverte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDenominationReadRow {
    pub denomination: String,
    pub date_debut: Option<String>,
    pub date_fin: Option<String>,
}

/// Comptes contentieux agrégés d'une entité (décisions distinctes) : total et
/// répartition par côté (`applicant`/`defendant`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityContentieuxCounts {
    pub decision_count: i64,
    pub as_applicant: i64,
    pub as_defendant: i64,
}

/// Décisions d'une entité groupées par année de lecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityYearCountRow {
    pub year: i32,
    pub count: i64,
}

/// Décisions d'une entité groupées par juridiction : `jurisdiction_code` du
/// référentiel (nullable) + `jurisdiction_type` de repli, le libellé étant résolu
/// côté `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityJurisdictionCountRow {
    pub jurisdiction_code: Option<String>,
    pub jurisdiction_type: String,
    pub count: i64,
}

/// Un conseil (avocat/cabinet) co-occurrent d'une entité : verbatim extrait +
/// uid registre résolu si le conseil l'est lui-même.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityCounselRow {
    pub value: String,
    pub entity_uid: Option<String>,
    pub count: i64,
}

/// Une décision citant l'entité (liste paginée) : id interne pour
/// l'hydratation `SearchHit` côté `lj-api` + côté et qualité représentatifs de
/// l'entité dans cette décision (pick stable par priorité `party` > `law_firm` >
/// `counsel_name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDecisionRow {
    pub decision_id: i64,
    pub quality: String,
    pub side: Option<String>,
}

/// Une entité de l'annuaire : ligne d'`entity` servant recherche et listing sans
/// jointure. `barreau` non `None` pour les avocats `cnb:` (2e segment de l'uid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDirectoryRow {
    pub uid: String,
    pub namespace: String,
    pub denomination: String,
    pub nature: String,
    pub forme: Option<String>,
    pub active: bool,
    pub barreau: Option<String>,
    pub decision_count: i64,
}

/// Une ligne `decision_party` lue pour l'encart « Parties » d'une décision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionPartyReadRow {
    pub quality: String,
    pub side: Option<String>,
    pub value: String,
    pub nature: Option<String>,
    pub barreau: Option<String>,
    pub entity_uid: Option<String>,
    pub ord: i32,
    /// `ord` de la partie que ce conseil représente.
    pub party_ord: Option<i32>,
}

/// Bundle d'écriture pour une décision (utilisé par le pipeline).
#[derive(Debug, Clone)]
pub struct BulkDecisionWrite {
    pub decision_id: Option<i64>,
    pub public_id: String,
    pub decision: Decision,
    pub content_checksum: String,
    /// `canonical_ref` calculé à l'ingest (`lj_ingest::extract`), passé à l'upsert.
    pub canonical_ref: Option<String>,
    pub write_mode: String,
    pub chunks: Vec<ChunkWrite>,
    /// `xml` (opendata) | `json` (judilibre) — le payload brut n'est pas stocké ;
    /// seul le format est persisté (`set_payload_format`).
    pub payload_format: String,
    pub extracted: Option<ExtractedFields>,
    /// Payload source moins le texte, offsets rebasés sur `full_text`.
    pub source_fields: Value,
    /// Version chunk+embed des chunks écrits (`None` = écrits sans embeddings).
    pub embed_version: Option<i16>,
}

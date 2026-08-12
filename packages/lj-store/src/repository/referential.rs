//! Textes & articles de loi (`legal_text` / `legal_article`) : upsert des
//! textes/articles, suppression LEGI par chemins, law-at-date, timeline des
//! versions, résolution de code, décisions citantes et sitemaps.
//!
//! L'identité d'un texte = `text_uid` (globalement unique, `source` hors clé) ;
//! l'identité d'une version d'article = `(text_uid, num_key, date_debut)`.
//! `source`/`source_uid`/`source_url` sont la provenance par version. Les
//! citations des décisions vivent dans `legal_citation` (module `citations`).

use super::norm_hubs::{NORM_FOND_EXPR, NORM_SCOPE};
use super::support::legal_article_row_from_row;
use super::types::{
    ArticleNeighborRow, ArticleRankHit, ArticleRrf, ArticleSearchRow, ArticleSearchStats,
    ArticleTitleMode, CitingDecisionRow, CoCitedArticleRow, FacetCount, FacetValueRow,
    JurisdictionRow, LawCodeSummaryRow, LawVersionRow, LegalArticleRow, LegalTextCatalogRow,
    LegalTextRow, SlugSourceRow, TocArticleRow, TocReadingRow,
};
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use chrono::NaiveDate;
use lj_core::normalizer::normalize_chars;
use tokio_postgres::types::ToSql;

/// Seuil de bascule des pages « décisions citantes » vers la marche par
/// récence, en nombre de décisions citantes (`citing_decision_counts`).
/// Partagé avec les citantes d'une division (`citations.rs`), qui l'applique à
/// la somme des décomptes de ses articles.
pub(super) const CITING_RECENCY_WALK_MIN_COUNT: i64 = 100_000;

impl DecisionRepository<'_> {
    /// Lit le référentiel `facet_value` entier : labels FR, hiérarchie
    /// (`parent_uid`) et ordre d'affichage, dans l'ordre du seed (`facet, sort`).
    #[tracing::instrument(name = "db.load_facet_values", skip(self), fields(db.system = "postgresql"))]
    pub async fn load_facet_values(&self) -> Result<Vec<FacetValueRow>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, facet, label, abbr, parent_uid, sort \
                 FROM facet_value ORDER BY facet, sort",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| FacetValueRow {
                uid: r.get(0),
                facet: r.get(1),
                label: r.get(2),
                abbr: r.get(3),
                parent_uid: r.get(4),
                sort: r.get(5),
            })
            .collect())
    }

    /// Lit le référentiel `jurisdiction` entier : une ligne par unité
    /// juridictionnelle (`tj_le_havre`, `ca_paris`…) avec code source, type,
    /// ville et label FR.
    #[tracing::instrument(name = "db.load_jurisdictions", skip(self), fields(db.system = "postgresql"))]
    pub async fn load_jurisdictions(&self) -> Result<Vec<JurisdictionRow>> {
        let rows = self
            .conn
            .query(
                "SELECT code, source_code, jurisdiction_type, city, label FROM jurisdiction",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| JurisdictionRow {
                code: r.get(0),
                source_code: r.get(1),
                jurisdiction_type: r.get(2),
                city: r.get(3),
                label: r.get(4),
            })
            .collect())
    }

    /// Upsert idempotent d'un texte de loi sur son identité `text_uid`. `title_key`
    /// (= `normalize_instrument(title)`) est calculé par l'appelant. Ne touche
    /// jamais `slug`, posé par [`Self::set_text_slugs`], et n'efface jamais `body`.
    #[tracing::instrument(name = "db.upsert_legal_text", skip(self, text), fields(db.system = "postgresql"))]
    pub async fn upsert_legal_text(&self, text: &LegalTextRow) -> Result<()> {
        let body = text.body.as_deref().map(normalize_chars);
        self.conn
            .execute(
                "
                INSERT INTO legal_text
                  (text_uid, jurisdiction, title, title_key, nature,
                   last_modified, date_texte, date_publi, eli, nor, instrument_key,
                   body, status)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                ON CONFLICT (text_uid) DO UPDATE SET
                  jurisdiction = EXCLUDED.jurisdiction,
                  title = EXCLUDED.title,
                  title_key = EXCLUDED.title_key,
                  nature = EXCLUDED.nature,
                  last_modified = EXCLUDED.last_modified,
                  date_texte = EXCLUDED.date_texte,
                  date_publi = EXCLUDED.date_publi,
                  eli = EXCLUDED.eli,
                  nor = EXCLUDED.nor,
                  instrument_key = EXCLUDED.instrument_key,
                  body = COALESCE(EXCLUDED.body, legal_text.body),
                  status = EXCLUDED.status
                ",
                &[
                    &text.text_uid,
                    &text.jurisdiction,
                    &text.title,
                    &text.title_key,
                    &text.nature,
                    &text.last_modified,
                    &text.date_texte,
                    &text.date_publi,
                    &text.eli,
                    &text.nor,
                    &text.instrument_key,
                    &body,
                    &text.status,
                ],
            )
            .await?;
        Ok(())
    }

    /// Insère un texte seulement s'il est absent (`ON CONFLICT DO NOTHING`) : un
    /// JORFTEXT déjà porté n'est jamais écrasé par la fiche d'origine du fond JO.
    /// Renvoie `true` si la ligne a été créée.
    pub async fn insert_legal_text_if_absent(&self, text: &LegalTextRow) -> Result<bool> {
        let n = self
            .conn
            .execute(
                "
                INSERT INTO legal_text
                  (text_uid, jurisdiction, title, title_key, nature,
                   last_modified, date_texte, date_publi, eli, nor, instrument_key,
                   body, status)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                ON CONFLICT (text_uid) DO NOTHING
                ",
                &[
                    &text.text_uid,
                    &text.jurisdiction,
                    &text.title,
                    &text.title_key,
                    &text.nature,
                    &text.last_modified,
                    &text.date_texte,
                    &text.date_publi,
                    &text.eli,
                    &text.nor,
                    &text.instrument_key,
                    &text.body,
                    &text.status,
                ],
            )
            .await?;
        Ok(n == 1)
    }

    /// Pose le corps d'un texte de référentiel. UPDATE ciblé, jamais de création :
    /// `false` = `text_uid` inconnu.
    #[tracing::instrument(name = "db.set_legal_text_body", skip(self, body), fields(db.system = "postgresql"))]
    pub async fn set_legal_text_body(&self, text_uid: &str, body: &str) -> Result<bool> {
        let body = normalize_chars(body);
        let n = self
            .conn
            .execute(
                "UPDATE legal_text SET body = $2 WHERE text_uid = $1",
                &[&text_uid, &body],
            )
            .await?;
        Ok(n > 0)
    }

    /// `text_uid` des textes d'une nature sans articles ni corps réel (`body` NULL
    /// ou < 300 caractères).
    pub async fn empty_legal_text_uids(&self, nature: &str) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid FROM legal_text lt
                 WHERE nature = $1
                   AND (body IS NULL OR length(body) < 300)
                   AND NOT EXISTS (
                     SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid
                   )
                 ORDER BY text_uid",
                &[&nature],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Textes sans slug, `(text_uid, title)` triés par `text_uid`.
    pub async fn texts_without_slug(&self) -> Result<Vec<SlugSourceRow>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, title, jurisdiction, date_texte::text, nor \
                 FROM legal_text WHERE slug IS NULL ORDER BY text_uid",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| SlugSourceRow {
                text_uid: r.get(0),
                title: r.get(1),
                jurisdiction: r.get(2),
                date_texte: r.get(3),
                nor: r.get(4),
            })
            .collect())
    }

    pub async fn existing_text_slugs(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query("SELECT slug FROM legal_text WHERE slug IS NOT NULL", &[])
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Pose les slugs calculés par la passe d'assignation, unique écrivain de la
    /// colonne. Ne remplit que les `slug NULL` — un slug posé est immuable.
    /// Renvoie le nombre de lignes écrites.
    pub async fn set_text_slugs(&self, slugs: &[(String, String)]) -> Result<u64> {
        let uids: Vec<&str> = slugs.iter().map(|(u, _)| u.as_str()).collect();
        let vals: Vec<&str> = slugs.iter().map(|(_, s)| s.as_str()).collect();
        let n = self
            .conn
            .execute(
                "
                UPDATE legal_text t
                SET slug = v.slug
                FROM unnest($1::text[], $2::text[]) AS v(text_uid, slug)
                WHERE t.text_uid = v.text_uid AND t.slug IS NULL
                ",
                &[&uids, &vals],
            )
            .await?;
        Ok(n)
    }

    /// Pose le flag `num_prefix_agnostic` d'un texte : la résolution d'article
    /// matchera sur le cœur numérique préfixe-strippé pour ce texte (codes
    /// territoriaux PF/NC cités avec un préfixe d'instrument incohérent).
    pub async fn set_legal_text_num_prefix_agnostic(
        &self,
        text_uid: &str,
        flag: bool,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE legal_text SET num_prefix_agnostic = $2 WHERE text_uid = $1",
                &[&text_uid, &flag],
            )
            .await?;
        Ok(())
    }

    /// Upsert idempotent d'une version d'article sur son identité
    /// `(text_uid, num_key, date_debut)`. Skip si le `content_checksum` (xxh3-64
    /// du bloc source brut) est inchangé. Le checksum `u64` est stocké en `BIGINT`
    /// via cast bit-à-bit `i64::from_ne_bytes`. `date_debut` `None` → sentinelle
    /// '0001-01-01' (borne ouverte ; la PK interdit le NULL).
    ///
    /// Le `DO UPDATE` exige `source_uid` égal : un article distinct qui plie sur la
    /// même PK n'écrase pas l'occupant, il est remonté en WARN et perdu.
    ///
    /// Renvoie `true` si la ligne a été insérée ou modifiée, `false` si skip.
    #[tracing::instrument(name = "db.upsert_legal_article", skip(self, art), fields(db.system = "postgresql"))]
    pub async fn upsert_legal_article(&self, art: &LegalArticleRow) -> Result<bool> {
        let checksum = i64::from_ne_bytes(art.content_checksum.to_ne_bytes());
        let texte = art.texte.as_deref().map(normalize_chars);
        let nota = art.nota.as_deref().map(normalize_chars);
        let texte_original = art.texte_original.as_deref().map(normalize_chars);
        let row = self
            .conn
            .query_one(
                "
                WITH ins AS (
                  INSERT INTO legal_article
                    (text_uid, num, num_key, position, title_path, status, date_debut,
                     date_fin, texte, nota, content_checksum, source, source_uid, source_url,
                     texte_original, lang_original, translation, source_asof, source_upstream_url)
                  SELECT $1, $2, $3, $4, $5, $6, COALESCE($7, DATE '0001-01-01'),
                         $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19
                  ON CONFLICT (text_uid, num_key, date_debut) DO UPDATE SET
                    num = EXCLUDED.num,
                    position = EXCLUDED.position,
                    title_path = EXCLUDED.title_path,
                    status = EXCLUDED.status,
                    date_fin = EXCLUDED.date_fin,
                    texte = EXCLUDED.texte,
                    nota = EXCLUDED.nota,
                    content_checksum = EXCLUDED.content_checksum,
                    source = EXCLUDED.source,
                    source_uid = EXCLUDED.source_uid,
                    source_url = EXCLUDED.source_url,
                    texte_original = EXCLUDED.texte_original,
                    lang_original = EXCLUDED.lang_original,
                    translation = EXCLUDED.translation,
                    source_asof = EXCLUDED.source_asof,
                    source_upstream_url = EXCLUDED.source_upstream_url,
                    -- Le chemin d'écriture n'attribue jamais un rang de lecture,
                    -- il l'invalide (ADR 0276) : `NULL` marque le texte à
                    -- recalculer, `refresh_reading_ranks` s'en charge en fin de
                    -- passe. Une insertion arrive à `NULL` par défaut.
                    reading_rank = CASE
                      WHEN legal_article.title_path IS DISTINCT FROM EXCLUDED.title_path
                        OR legal_article.position IS DISTINCT FROM EXCLUDED.position
                      THEN NULL ELSE legal_article.reading_rank END
                  WHERE legal_article.content_checksum IS DISTINCT FROM EXCLUDED.content_checksum
                    AND legal_article.source_uid = EXCLUDED.source_uid
                  RETURNING 1
                )
                SELECT (SELECT count(*) FROM ins)::int AS written,
                       (SELECT a.source_uid FROM legal_article a
                        WHERE a.text_uid = $1 AND a.num_key = $3
                          AND a.date_debut = COALESCE($7, DATE '0001-01-01')) AS holder
                ",
                &[
                    &art.text_uid,
                    &art.num,
                    &art.num_key,
                    &art.position,
                    &art.title_path,
                    &art.status,
                    &art.date_debut,
                    &art.date_fin,
                    &texte,
                    &nota,
                    &checksum,
                    &art.source,
                    &art.source_uid,
                    &art.source_url,
                    &texte_original,
                    &art.lang_original,
                    &art.translation,
                    &art.source_asof,
                    &art.source_upstream_url,
                ],
            )
            .await?;
        let written: i32 = row.get("written");
        let holder: Option<String> = row.get("holder");
        if written == 0 {
            if let Some(holder) = holder.filter(|h| *h != art.source_uid) {
                tracing::warn!(
                    text_uid = %art.text_uid,
                    num = %art.num,
                    num_key = %art.num_key,
                    date_debut = ?art.date_debut,
                    source = %art.source,
                    entrant = %art.source_uid,
                    occupant = %holder,
                    "CLASH D'IDENTITÉ D'ARTICLE (ADR 0236) : un article chronique distinct \
                     plie sur la même PK (text_uid, num_key, date_debut) — ligne entrante \
                     NON écrite, occupant conservé. À réviser : numéro réellement dupliqué \
                     dans le texte, ou clé d'identité à affiner."
                );
            }
        }
        Ok(written > 0)
    }

    /// Rafraîchit la fraîcheur « as-of » d'une source live re-synchronisée
    /// quotidiennement (legifrance/kali) : une ligne par source dans
    /// `ingest_freshness`.
    #[tracing::instrument(name = "db.upsert_ingest_freshness", skip(self), fields(db.system = "postgresql"))]
    pub async fn upsert_ingest_freshness(&self, source: &str, asof: NaiveDate) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO ingest_freshness (source, asof) VALUES ($1, $2)
                 ON CONFLICT (source) DO UPDATE SET asof = EXCLUDED.asof",
                &[&source, &asof],
            )
            .await?;
        Ok(())
    }

    /// Fraîcheur « as-of » d'une source live (legifrance/kali) depuis
    /// `ingest_freshness`. `None` si la source n'y figure pas — la fraîcheur vit
    /// alors sur `legal_article.source_asof`.
    #[tracing::instrument(name = "db.get_ingest_freshness", skip(self), fields(db.system = "postgresql"))]
    pub async fn get_ingest_freshness(&self, source: &str) -> Result<Option<NaiveDate>> {
        let row = self
            .conn
            .query_opt(
                "SELECT asof FROM ingest_freshness WHERE source = $1",
                &[&source],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Paires distinctes `(source, source_url)` des lignes dont `source` est une
    /// catégorie/méthode à reclasser (`treaty`, `eu-law`, `official-fr`,
    /// `traduction-automatique`) et qui portent une URL, d'où le libellé de
    /// diffuseur se dérive (`diffuseur_label_from_url`). Lecture seule.
    #[tracing::instrument(name = "db.distinct_category_source_urls", skip(self), fields(db.system = "postgresql"))]
    pub async fn distinct_category_source_urls(
        &self,
        sources: &[&str],
    ) -> Result<Vec<(String, String)>> {
        let sources: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT source, source_url FROM legal_article \
                 WHERE source = ANY($1) AND COALESCE(source_url, '') <> ''",
                &[&sources],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Reclasse `source` pour les lignes d'un couple `(ancien source, URL)` donné
    /// vers le `nouveau` libellé de diffuseur. Renvoie le nombre de lignes.
    #[tracing::instrument(name = "db.relabel_source_by_url", skip(self), fields(db.system = "postgresql"))]
    pub async fn relabel_source_by_url(&self, old: &str, url: &str, new: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "UPDATE legal_article SET source = $3 WHERE source = $1 AND source_url = $2",
                &[&old, &url, &new],
            )
            .await?;
        Ok(n)
    }

    /// Reclasse les traités du bulk JORF (`source='treaty'`, sans URL,
    /// `source_uid` natif `JORFARTI…`) en `source='jorf'` — la nature « traité »
    /// reste portée par `jurisdiction='INTL'`. Exclut les chaînes curées
    /// (avenants, `source_uid` propre au dataset). Renvoie le nombre de lignes.
    #[tracing::instrument(name = "db.relabel_treaty_jorf_bulk", skip(self), fields(db.system = "postgresql"))]
    pub async fn relabel_treaty_jorf_bulk(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "UPDATE legal_article SET source = 'jorf' \
                 WHERE source = 'treaty' AND COALESCE(source_url, '') = '' \
                   AND source_uid LIKE 'JORFARTI%'",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Canonicalise un libellé de diffuseur (`legal_article.source`) : `from` →
    /// `to`, pour les variantes d'hôte d'un même diffuseur (ex. `jafbase.fr` →
    /// `jafbase`). Renvoie le nombre de lignes modifiées.
    #[tracing::instrument(name = "db.canonicalize_source_label", skip(self), fields(db.system = "postgresql"))]
    pub async fn canonicalize_source_label(&self, from: &str, to: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "UPDATE legal_article SET source = $2 WHERE source = $1",
                &[&from, &to],
            )
            .await?;
        Ok(n)
    }

    /// Reliquat post-reclassement : lignes encore sous un `source`
    /// catégorie/méthode (URL vide, hors bulk JORF). `(source, nombre)`.
    /// Lecture seule.
    #[tracing::instrument(name = "db.count_category_source_leftovers", skip(self), fields(db.system = "postgresql"))]
    pub async fn count_category_source_leftovers(
        &self,
        sources: &[&str],
    ) -> Result<Vec<(String, i64)>> {
        let sources: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
        let rows = self
            .conn
            .query(
                "SELECT source, count(*) FROM legal_article \
                 WHERE source = ANY($1) GROUP BY source ORDER BY source",
                &[&sources],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// `text_uid` des véhicules de publication : les cibles d'une arête `publie`
    /// entrante possédée par une fiche instrument. Lecture seule.
    #[tracing::instrument(name = "db.published_vehicle_uids", skip(self), fields(db.system = "postgresql"))]
    pub async fn published_vehicle_uids(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT target_text_uid FROM legal_link
                 WHERE verb = 'publie' AND direction = 'incoming'
                   AND target_text_uid IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Suppression d'articles d'un fond DILA par chemins issus des
    /// `liste_suppression_<fond>*.dat`. Le stem du chemin `.dat`
    /// (`…/LEGIARTI000….xml` / `…/KALIARTI000….xml`) est le `source_uid` à purger
    /// pour le `source` donné (`'legifrance'` LEGI/JORF, `'kali'` KALI). Renvoie
    /// le nombre de lignes supprimées.
    #[tracing::instrument(name = "db.delete_legal_articles_by_paths", skip(self, paths), fields(db.system = "postgresql", source, paths = paths.len()))]
    pub async fn delete_legal_articles_by_paths(
        &self,
        source: &str,
        paths: &[String],
    ) -> Result<u64> {
        if paths.is_empty() {
            return Ok(0);
        }
        let source_uids: Vec<String> = paths
            .iter()
            .filter_map(|p| {
                let stem = p.rsplit('/').next().unwrap_or(p);
                let id = stem.strip_suffix(".xml").unwrap_or(stem);
                if id.is_empty() {
                    None
                } else {
                    Some(id.to_string())
                }
            })
            .collect();
        if source_uids.is_empty() {
            return Ok(0);
        }
        // Purge liée des arêtes du graphe AVANT les articles : la clé owner de
        // `legal_link` se résout via la ligne d'article encore présente. Sauf `jorf` :
        // une publication n'écrit aucun lien, et son triplet est celui de la version
        // LEGI du jour de parution.
        if source != "jorf" {
            self.conn
                .execute(
                    "DELETE FROM legal_link ll USING legal_article a \
                     WHERE a.source = $1 AND a.source_uid = ANY($2) \
                       AND ll.owner_text_uid = a.text_uid AND ll.owner_num_key = a.num_key \
                       AND ll.owner_date_debut = a.date_debut",
                    &[&source, &source_uids],
                )
                .await?;
        }
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_article \
                 WHERE source = $1 AND source_uid = ANY($2)",
                &[&source, &source_uids],
            )
            .await?;
        Ok(n)
    }

    /// Pose l'état de diffusion d'un lot de textes. Ne touche que la `nature`
    /// donnée. Renvoie le nombre de lignes modifiées ; un uid absent est ignoré.
    #[tracing::instrument(name = "db.set_legal_texts_status", skip(self, text_uids), fields(db.system = "postgresql", nature, status, uids = text_uids.len()))]
    pub async fn set_legal_texts_status(
        &self,
        nature: &str,
        text_uids: &[String],
        status: &str,
    ) -> Result<u64> {
        if text_uids.is_empty() {
            return Ok(0);
        }
        let n = self
            .conn
            .execute(
                "UPDATE legal_text SET status = $3 \
                 WHERE nature = $1 AND text_uid = ANY($2)",
                &[&nature, &text_uids, &status],
            )
            .await?;
        Ok(n)
    }

    /// Fond UE en base : un texte porteur d'articles `eu-law`/`eur-lex` par
    /// ligne — `(text_uid, celex, title, nature, jurisdiction, date_texte)`, le
    /// CELEX lu sur `legal_article.source_url`.
    #[allow(clippy::type_complexity)]
    pub async fn eu_law_registry(
        &self,
    ) -> Result<Vec<(String, String, String, String, String, Option<NaiveDate>)>> {
        let rows = self
            .conn
            .query(
                "SELECT t.text_uid, regexp_replace(min(a.source_url), '^.*/celex/', ''),
                        t.title, t.nature, t.jurisdiction, t.date_texte
                 FROM legal_article a
                 JOIN legal_text t USING (text_uid)
                 WHERE a.source IN ('eu-law', 'eur-lex') AND a.source_url LIKE '%/celex/%'
                 GROUP BY t.text_uid, t.title, t.nature, t.jurisdiction, t.date_texte",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4), r.get(5)))
            .collect())
    }

    /// Corps monolithiques du référentiel : `(text_uid, body)` des textes à corps.
    pub async fn legal_text_bodies(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, body FROM legal_text WHERE body IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Page keyset de toutes les versions d'articles à corps, ordonnée par la PK
    /// `(text_uid, num_key, date_debut)` : les versions d'un même texte arrivent
    /// groupées. `after` = dernière clé de la page précédente
    /// (`("", "", 0001-01-01)` pour la première).
    pub async fn legal_article_versions_page(
        &self,
        after: (&str, &str, NaiveDate),
        limit: i64,
    ) -> Result<Vec<(String, String, NaiveDate, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, num_key, date_debut, texte \
                 FROM legal_article \
                 WHERE (text_uid, num_key, date_debut) > ($1, $2, $3) \
                   AND texte IS NOT NULL \
                 ORDER BY text_uid, num_key, date_debut \
                 LIMIT $4",
                &[&after.0, &after.1, &after.2, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// Corps de la version la plus récente de chaque `(text_uid, num_key)`
    /// demandé, avec son `date_debut`. Une clé sans version à corps est absente du
    /// résultat.
    pub async fn legal_article_latest_bodies(
        &self,
        keys: &[(String, String)],
    ) -> Result<Vec<(String, String, String, NaiveDate)>> {
        let uids: Vec<&str> = keys.iter().map(|(u, _)| u.as_str()).collect();
        let nums: Vec<&str> = keys.iter().map(|(_, n)| n.as_str()).collect();
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT ON (a.text_uid, a.num_key) \
                     a.text_uid, a.num_key, a.texte, a.date_debut \
                 FROM legal_article a \
                 JOIN unnest($1::text[], $2::text[]) AS k(uid, num) \
                   ON k.uid = a.text_uid AND k.num = a.num_key \
                 WHERE a.texte IS NOT NULL \
                 ORDER BY a.text_uid, a.num_key, a.date_debut DESC",
                &[&uids, &nums],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// Purge des articles d'un texte hors snapshot courant : supprime les lignes
    /// du `source`/`text_uid` dont la version n'est pas `keep_date` ou dont le
    /// `num_key` a disparu du document. Les arêtes `legal_link` des lignes purgées
    /// partent d'abord.
    #[tracing::instrument(name = "db.delete_legal_articles_versions_except", skip(self, keep_num_keys), fields(db.system = "postgresql", source, text_uid))]
    pub async fn delete_legal_articles_versions_except(
        &self,
        source: &str,
        text_uid: &str,
        keep_date: chrono::NaiveDate,
        keep_num_keys: &[String],
    ) -> Result<u64> {
        self.conn
            .execute(
                "DELETE FROM legal_link ll USING legal_article a \
                 WHERE a.source = $1 AND a.text_uid = $2 \
                   AND (a.date_debut <> $3 OR a.num_key <> ALL($4)) \
                   AND ll.owner_text_uid = a.text_uid AND ll.owner_num_key = a.num_key \
                   AND ll.owner_date_debut = a.date_debut",
                &[&source, &text_uid, &keep_date, &keep_num_keys],
            )
            .await?;
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_article \
                 WHERE source = $1 AND text_uid = $2 \
                   AND (date_debut <> $3 OR num_key <> ALL($4))",
                &[&source, &text_uid, &keep_date, &keep_num_keys],
            )
            .await?;
        Ok(n)
    }

    /// Purge tous les articles d'un texte donné (`text_uid`, globalement unique).
    /// Vaut pour la forme mono-version (un article par `num_key`) comme
    /// multi-versions. Renvoie le nombre de lignes supprimées.
    #[tracing::instrument(name = "db.delete_legal_articles_by_text", skip(self), fields(db.system = "postgresql"))]
    pub async fn delete_legal_articles_by_text(&self, text_uid: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_article WHERE text_uid = $1",
                &[&text_uid],
            )
            .await?;
        Ok(n)
    }

    /// Collapse des doublons diffuseur : supprime les coquilles `LEGITEXT` vides
    /// (0 article) dont un autre texte porte le corps sous le même `title_key`.
    /// Ne touche jamais un texte cité (`ref_text_uid`) ni un texte porteur
    /// d'articles. Renvoie le nombre de coquilles collapsées.
    #[tracing::instrument(name = "db.collapse_empty_legitext_doublons", skip(self), fields(db.system = "postgresql"))]
    pub async fn collapse_empty_legitext_doublons(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "
                DELETE FROM legal_text t
                WHERE t.text_uid LIKE 'LEGITEXT%'
                  AND t.jurisdiction = 'FR'
                  AND NOT EXISTS (
                        SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)
                  AND EXISTS (
                        SELECT 1 FROM legal_text b
                        JOIN legal_article ba ON ba.text_uid = b.text_uid
                        WHERE b.text_uid <> t.text_uid
                          AND lower(b.title_key) = lower(t.title_key))
                  AND NOT EXISTS (
                        SELECT 1 FROM legal_citation c
                        WHERE public.lj_cit_terms(c.spans) @> ARRAY[t.text_uid])
                ",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Rôles des textes publiés, classification scopée à un lot de `text_uid` :
    /// les uids sont resetés à `instrument` puis re-classifiés. Tourne dans la
    /// transaction/connexion de l'appelant. Renvoie
    /// (individuel, habilitation, vehicule, modificatif).
    #[tracing::instrument(name = "db.classify_text_roles_scoped", skip_all, fields(db.system = "postgresql", uids = uids.len()))]
    pub async fn classify_text_roles_scoped(
        &self,
        uids: &[String],
    ) -> Result<(u64, u64, u64, u64)> {
        if uids.is_empty() {
            return Ok((0, 0, 0, 0));
        }
        self.run_text_role_sqls(Some(uids)).await
    }

    /// Rôles des textes publiés, backfill pleine-table ; le régime établi passe
    /// par [`Self::classify_text_roles_scoped`]. Recalcule tout. Renvoie
    /// (individuel, habilitation, vehicule, modificatif).
    #[tracing::instrument(name = "db.backfill_text_roles", skip(self), fields(db.system = "postgresql"))]
    pub async fn backfill_text_roles(&self) -> Result<(u64, u64, u64, u64)> {
        self.run_text_role_sqls(None).await
    }

    /// Exécute les six statements de [`text_role_sqls`] (reset + 5 passes), scopés
    /// à `uids` si fournis. Renvoie
    /// (individuel, habilitation, vehicule + vehicule_publie, modificatif).
    async fn run_text_role_sqls(&self, uids: Option<&[String]>) -> Result<(u64, u64, u64, u64)> {
        let mut counts = [0u64; 6];
        for (i, sql) in text_role_sqls(uids.is_some()).iter().enumerate() {
            counts[i] = match uids {
                Some(uids) => self.conn.execute(sql.as_str(), &[&uids]).await?,
                None => self.conn.execute(sql.as_str(), &[]).await?,
            };
        }
        Ok((counts[1], counts[2], counts[3] + counts[4], counts[5]))
    }

    /// Aligne `legal_link.verb` sur le repli verbe/nom courant de
    /// `lj_extract::legi::lien_verb`. Renvoie le nombre de lignes alignées.
    #[tracing::instrument(name = "db.normalize_link_verbs", skip(self), fields(db.system = "postgresql"))]
    pub async fn normalize_link_verbs(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "
                UPDATE legal_link SET verb = m.verb
                FROM (VALUES
                        ('RATIFIE', 'ratifie'), ('RATIFICATION', 'ratifie'),
                        ('DENONCE', 'denonce'), ('DENONCIATION', 'denonce'),
                        ('ANNULE', 'annule'), ('ANNULATION', 'annule'),
                        ('DISJOINT', 'disjoint'), ('DISJONCTION', 'disjoint'),
                        ('ETEND', 'etend'), ('EXTENSION', 'etend'),
                        ('RECTIFIE', 'rectifie'), ('TRANSPOSITION', 'transpose')
                     ) AS m (typelien, verb)
                WHERE legal_link.typelien = m.typelien
                  AND legal_link.verb <> m.verb
                ",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Article servi à une date. `date` absente ⇒ la version dans la fenêtre de
    /// validité du jour, sinon la dernière. Sinon, la version dont l'intervalle
    /// `[date_debut, date_fin]` couvre `date` (la plus récente en cas de
    /// chevauchement) ; `date_debut` est NOT NULL (sentinelle '0001-01-01' = borne
    /// ouverte basse). À date égale la version consolidée prime sur la publication
    /// au JO (`(source='jorf') ASC`). `None` si aucune version ne s'applique.
    /// Résolution vers l'identité `(text_uid, num_key)`, la version choisie ici par
    /// la date.
    #[tracing::instrument(name = "db.law_article_at_date", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_article_at_date(
        &self,
        text_uid: &str,
        num_key: &str,
        date: Option<NaiveDate>,
    ) -> Result<Option<LegalArticleRow>> {
        let row = match date {
            Some(d) => {
                self.conn
                    .query_opt(
                        "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM legal_article
                        WHERE text_uid = $1 AND num_key = $2
                          AND date_debut <= $3
                          AND (date_fin IS NULL OR date_fin >= $3)
                        ORDER BY date_debut DESC, (source = 'jorf') ASC
                        LIMIT 1
                        ",
                        &[&text_uid, &num_key, &d],
                    )
                    .await?
            }
            None => {
                let valid_a = article_valid_now("a");
                let norm_a = article_is_norm("a");
                self.conn
                    .query_opt(
                        &format!(
                            "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM legal_article a
                        WHERE text_uid = $1 AND num_key = $2
                        ORDER BY ({valid_a}) DESC, ({norm_a}) DESC,
                                 (source = 'jorf') ASC, date_debut DESC NULLS LAST
                        LIMIT 1
                        "
                        ),
                        &[&text_uid, &num_key],
                    )
                    .await?
            }
        };
        Ok(row.map(legal_article_row_from_row))
    }

    /// Lecture « à la suite » : les articles du texte `text_uid`, version en
    /// vigueur à `date` (ou `VIGUEUR` si `date` absente), triés par `position`
    /// (ordre de lecture réel, ≠ tri lexical). `position` NULL trié en dernier ;
    /// fallback `num_key`. Champs bruts.
    #[tracing::instrument(name = "db.law_text_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_text_articles(
        &self,
        text_uid: &str,
        date: Option<NaiveDate>,
    ) -> Result<Vec<LegalArticleRow>> {
        let rows = match date {
            Some(d) => {
                self.conn
                    .query(
                        "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM (
                          SELECT DISTINCT ON (num_key) *
                          FROM legal_article
                          WHERE text_uid = $1 AND date_debut <= $2
                            AND (date_fin IS NULL OR date_fin >= $2)
                          ORDER BY num_key, date_debut DESC, (source = 'jorf') ASC
                        ) v
                        ORDER BY position NULLS LAST, num_key
                        ",
                        &[&text_uid, &d],
                    )
                    .await?
            }
            None => {
                let valid_a = article_valid_now("a");
                let norm_a = article_is_norm("a");
                self.conn
                    .query(
                        &format!(
                            "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM (
                          -- Fenêtre du jour préférée, sinon dernière version
                          -- (ADR 0300 §3) ; vraie version avant moignon (0312),
                          -- consolidée avant publication.
                          SELECT DISTINCT ON (num_key) *
                          FROM legal_article a
                          WHERE text_uid = $1
                          ORDER BY num_key, ({valid_a}) DESC, ({norm_a}) DESC,
                                   (source = 'jorf') ASC, date_debut DESC NULLS LAST
                        ) v
                        ORDER BY position NULLS LAST, num_key
                        "
                        ),
                        &[&text_uid],
                    )
                    .await?
            }
        };
        Ok(rows.into_iter().map(legal_article_row_from_row).collect())
    }

    /// Timeline des versions d'un article, triée par `date_debut` croissante.
    /// `source_uid` = identifiant natif de la version (LEGIARTI…). Dates rendues
    /// en ISO `String` (`::text`).
    #[tracing::instrument(name = "db.law_article_versions", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_article_versions(
        &self,
        text_uid: &str,
        num_key: &str,
    ) -> Result<Vec<LawVersionRow>> {
        let rows = self
            .conn
            .query(
                "
                SELECT source_uid, status, date_debut::text, date_fin::text
                FROM legal_article
                WHERE text_uid = $1 AND num_key = $2
                ORDER BY date_debut
                ",
                &[&text_uid, &num_key],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| LawVersionRow {
                source_uid: r.get(0),
                status: r.get(1),
                date_debut: r.get(2),
                date_fin: r.get(3),
            })
            .collect())
    }

    /// Sommaire d'un texte par son `slug` : métadonnées du `legal_text` + nombre
    /// d'articles distincts toutes versions. `None` si le slug est inconnu.
    #[tracing::instrument(name = "db.law_code_summary", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_code_summary(&self, slug: &str) -> Result<Option<LawCodeSummaryRow>> {
        let row = self
            .conn
            .query_opt(
                "
                SELECT t.text_uid, t.slug, t.title, t.nature, t.last_modified::text,
                       -- Toutes versions confondues (ADR 0162 §5) : un texte
                       -- abrogé affiche ses articles, pas un sommaire vide.
                       (SELECT count(DISTINCT a.num_key)
                        FROM legal_article a
                        WHERE a.text_uid = t.text_uid) AS article_count,
                       t.upcoming_versions,
                       t.body, t.status, t.nor, t.date_texte::text
                FROM legal_text t
                WHERE t.slug = $1
                ",
                &[&slug],
            )
            .await?;
        Ok(row.map(|r| LawCodeSummaryRow {
            text_uid: r.get(0),
            slug: r.get(1),
            title: r.get(2),
            nature: r.get(3),
            last_modified: r.get(4),
            article_count: r.get(5),
            upcoming_versions: r.get(6),
            body: r.get(7),
            status: r.get(8),
            nor: r.get(9),
            date_texte: r.get(10),
        }))
    }

    /// Pose les dates de versions futures d'un texte. Écrit toujours, y compris
    /// vide (un incrément qui vide `VERSIONS_A_VENIR` vide la colonne).
    pub async fn set_legal_text_upcoming_versions(
        &self,
        text_uid: &str,
        dates: &[NaiveDate],
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE legal_text SET upcoming_versions = $2 WHERE text_uid = $1",
                &[&text_uid, &dates],
            )
            .await?;
        Ok(())
    }

    /// Résout un slug de code en identité de texte — `text_uid` et titre humain —
    /// par lookup exact.
    ///
    /// `slug` = la chaîne d'URL `/texte/{slug}`, le slug canonique que nos liens
    /// portent (stocké au DTO). `None` si le slug est inconnu → 404 côté appelant.
    ///
    /// Le titre voyage avec l'uid : la page article a besoin des deux (en-tête
    /// « Article N du <titre> »), et c'est la même ligne.
    #[tracing::instrument(name = "db.referential_ref", skip(self), fields(db.system = "postgresql"))]
    pub async fn referential_ref(&self, slug: &str) -> Result<Option<(String, Option<String>)>> {
        let row = self
            .conn
            .query_opt(
                "SELECT text_uid, title FROM legal_text WHERE slug = $1 LIMIT 1",
                &[&slug],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), r.get(1))))
    }

    /// Résout un slug d'instrument MCP (forme nom-libre slugifiée par l'appelant)
    /// vers son `text_uid`, alphabet des colonnes de filtre. `None` = inconnu →
    /// erreur corrective côté MCP.
    #[tracing::instrument(name = "db.resolve_instrument_uid", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_instrument_uid(&self, slug: &str) -> Result<Option<String>> {
        let row = self
            .conn
            .query_opt("SELECT text_uid FROM legal_text WHERE slug = $1", &[&slug])
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Le slug existe-t-il au catalogue ? — pour router `get_legal_text` vers
    /// `/texte/{slug}` après slugification du nom libre.
    #[tracing::instrument(name = "db.law_slug_exists", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_slug_exists(&self, slug: &str) -> Result<bool> {
        let row = self
            .conn
            .query_opt("SELECT 1 FROM legal_text WHERE slug = $1", &[&slug])
            .await?;
        Ok(row.is_some())
    }

    /// Suggestions `(slug, titre)` pour une valeur d'instrument inconnue, par
    /// similarité trigramme tolérante aux fautes (pg_trgm). `similarity`
    /// plein-champ, sur le slug et le titre entiers.
    #[tracing::instrument(name = "db.suggest_instruments", skip(self), fields(db.system = "postgresql"))]
    pub async fn suggest_instruments(
        &self,
        needle: &str,
        limit: i64,
    ) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT slug, title FROM legal_text \
                 WHERE slug IS NOT NULL \
                 AND GREATEST(similarity($1, slug), similarity($1, title)) > 0.4 \
                 ORDER BY GREATEST(similarity($1, slug), similarity($1, title)) DESC, \
                 length(title), title LIMIT $2",
                &[&needle, &limit],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Décisions citant un article : backlinks depuis les blobs
    /// `legal_citation.spans`. Paginée sous cap de fenêtre (`offset + limit ≤ 100`,
    /// tenu par la validation d'entrée). Champs bruts.
    ///
    /// Deux plans selon le volume de citantes (`citing_decision_counts`) : bitmap
    /// GIN et tri portée puis date pour un article courant, marche descendante de
    /// `idx_decisions_date_lecture` pour un article ultra-cité
    /// (≥ [`CITING_RECENCY_WALK_MIN_COUNT`]), qui rend la récence seule.
    ///
    /// Bornée à la fenêtre de validité `[date_debut, date_fin)` de la version
    /// servie. `date_fin` `None` = version en vigueur.
    #[tracing::instrument(name = "db.law_decisions_citing", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_decisions_citing(
        &self,
        ref_text_uid: &str,
        num_key: &str,
        date_debut: NaiveDate,
        date_fin: Option<NaiveDate>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CitingDecisionRow>> {
        let citing_count: i64 = self
            .conn
            .query_opt(
                "SELECT decision_count FROM citing_decision_counts \
                 WHERE cited_term = $1 || '|' || $2",
                &[&ref_text_uid, &num_key],
            )
            .await?
            .map(|r| r.get(0))
            .unwrap_or(0);
        let params: [&(dyn ToSql + Sync); 6] = [
            &ref_text_uid,
            &num_key,
            &limit,
            &offset,
            &date_debut,
            &date_fin,
        ];
        let rows = if citing_count >= CITING_RECENCY_WALK_MIN_COUNT {
            // L'EXISTS sur le blob n'est pas indexable : `d.date_lecture >= $5` exclut les
            // NULL et le scan arrière de l'index date sert l'ORDER BY sans tri.
            self.conn
                .query(
                    "
                    SELECT d.id, d.public_id, d.jurisdiction_type,
                           j.label, d.date_lecture::text, d.docket_numbers,
                           d.publication_codes, d.summary
                    FROM decisions d
                    JOIN legal_citation lc ON lc.decision_id = d.id
                    LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                    WHERE d.date_lecture >= $5
                      AND ($6::date IS NULL OR d.date_lecture < $6)
                      AND EXISTS (
                          SELECT 1 FROM jsonb_array_elements(lc.spans) AS el
                          WHERE el->>2 = $1 AND el->>3 = $2)
                    ORDER BY d.date_lecture DESC, d.id
                    LIMIT $3 OFFSET $4
                    ",
                    &params,
                )
                .await?
        } else {
            // Autorité d'abord : rang de portée dérivé de `publication_codes` (mêmes
            // groupes que la facette), puis date décroissante. Les listes de codes sont
            // des constantes du code, inlinées.
            let rank_expr = {
                let arr = |group: &str| {
                    let codes: Vec<String> = lj_core::publication::significance_codes(group)
                        .iter()
                        .map(|c| format!("'{c}'"))
                        .collect();
                    format!("ARRAY[{}]", codes.join(","))
                };
                format!(
                    "CASE WHEN d.publication_codes && {maj} THEN 0 \
                          WHEN d.publication_codes && {imp} THEN 1 \
                          WHEN d.publication_codes && {lim} THEN 2 \
                          ELSE 3 END",
                    maj = arr("majeure"),
                    imp = arr("importante"),
                    lim = arr("limitee"),
                )
            };
            self.conn
                .query(
                    &format!(
                        "
                        SELECT d.id, d.public_id, d.jurisdiction_type,
                               j.label, d.date_lecture::text, d.docket_numbers,
                               d.publication_codes, d.summary,
                               {rank_expr} AS significance_rank
                        FROM legal_citation lc
                        JOIN decisions d ON d.id = lc.decision_id
                        LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                        WHERE public.lj_cit_terms(lc.spans) @> ARRAY[$1 || '|' || $2]
                          -- fenêtre de validité [date_debut, date_fin) de la version
                          -- servie ; date_fin NULL = en vigueur (borne haute ouverte).
                          AND d.date_lecture >= $5
                          AND ($6::date IS NULL OR d.date_lecture < $6)
                        -- Texte ISO 'YYYY-MM-DD' → tri == chronologique. Une ligne
                        -- blob par décision : pas de doublons à dédupliquer.
                        ORDER BY significance_rank, d.date_lecture::text DESC NULLS LAST, d.id
                        LIMIT $3 OFFSET $4
                        "
                    ),
                    &params,
                )
                .await?
        };
        Ok(rows
            .iter()
            .map(|r| CitingDecisionRow {
                id: r.get(0),
                public_id: r.get(1),
                jurisdiction_type: r.get(2),
                jurisdiction_name: r.get(3),
                date_lecture: r.get(4),
                docket_numbers: r.get(5),
                publication_codes: r.get(6),
                summary: r.get(7),
                cited_num_keys: Vec::new(),
            })
            .collect())
    }

    /// Fil d'Ariane TOC d'une version d'article : les divisions enclosantes, de la
    /// racine à la section directe (`label`, `child_cid`). Marche remontante depuis
    /// l'arête de la version servie ; en cas d'arêtes multiples au même niveau, une
    /// seule chaîne est retenue. Vide si la version n'est pas dans la TOC.
    #[tracing::instrument(name = "db.article_toc_breadcrumb", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_toc_breadcrumb(
        &self,
        text_uid: &str,
        article_uid: &str,
    ) -> Result<Vec<(String, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "
                WITH RECURSIVE anchor AS (
                    SELECT e.owner_uid FROM legal_toc_edge e
                    WHERE e.text_uid = $1 AND e.child_kind = 'article'
                      AND e.child_uid = $2
                    ORDER BY e.seq LIMIT 1
                ), up AS (
                    SELECT e.owner_uid, e.label, e.child_cid, 0 AS d
                    FROM legal_toc_edge e
                    JOIN anchor a ON e.child_uid = a.owner_uid
                    UNION ALL
                    SELECT e.owner_uid, e.label, e.child_cid, up.d + 1
                    FROM legal_toc_edge e
                    JOIN up ON e.child_uid = up.owner_uid
                    WHERE up.d < 12
                )
                SELECT DISTINCT ON (d) label, child_cid FROM up
                ORDER BY d DESC
                ",
                &[&text_uid, &article_uid],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Articles co-cités avec `(ref_text_uid, num_key)` dans les décisions :
    /// échantillon des décisions citant l'article (500), puis agrégat de leurs
    /// autres citations pondéré tf-idf saturé — `n/(n+20) × ln(N / df)`, `df` depuis
    /// `citing_decision_counts` et `N` de l'estimation planner de `decisions`.
    /// Seuil ≥ 3 co-occurrences ; le compte affiché reste brut.
    #[tracing::instrument(name = "db.law_co_cited_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_co_cited_articles(
        &self,
        ref_text_uid: &str,
        num_key: &str,
        limit: i64,
    ) -> Result<Vec<CoCitedArticleRow>> {
        let rows = self
            .conn
            .query(
                "
                WITH citing AS (
                    SELECT decision_id FROM legal_citation
                    WHERE public.lj_cit_terms(spans) @> ARRAY[$1 || '|' || $2]
                    LIMIT 500
                ), co AS (
                    SELECT el->>2 AS ref_text_uid, el->>3 AS ref_num_key,
                           count(DISTINCT lc.decision_id) AS n
                    FROM legal_citation lc
                    JOIN citing c ON c.decision_id = lc.decision_id
                    CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
                    WHERE el->>3 IS NOT NULL
                      AND (el->>2 <> $1 OR el->>3 <> $2)
                    GROUP BY 1, 2
                    HAVING count(DISTINCT lc.decision_id) >= 3
                )
                SELECT co.ref_num_key, co.n, t.slug, t.title
                FROM co
                JOIN legal_text t ON t.text_uid = co.ref_text_uid
                -- df absent (terme jamais recompté — article tout neuf) :
                -- repli df = n_co, le plus conservateur des connus.
                LEFT JOIN citing_decision_counts s
                       ON s.cited_term = co.ref_text_uid || '|' || co.ref_num_key
                ORDER BY (co.n::float8 / (co.n + 20)) * ln(
                             (SELECT greatest(reltuples, 1)::float8 FROM pg_class
                              WHERE oid = 'decisions'::regclass)
                             / greatest(coalesce(s.decision_count, co.n), 1)::float8
                         ) DESC,
                         co.ref_num_key
                LIMIT $3
                ",
                &[&ref_text_uid, &num_key, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| CoCitedArticleRow {
                num_key: r.get(0),
                count: r.get(1),
                text_slug: r.get(2),
                text_title: r.get(3),
            })
            .collect())
    }

    /// Itère `(slug, num, lastmod)` pour les articles en vigueur (sitemaps
    /// `/texte/{slug}/{num}`). Une ligne par URL, pas par version : le
    /// `DISTINCT ON` retient la version que sert `law_article_at_date`. `lastmod` =
    /// `COALESCE(t.last_modified, a.date_debut)`, nullable et non filtré — les
    /// sentinelles DILA remontent telles quelles, `plausible_lastmod` côté
    /// `lj-ingest` décide de publier ou d'omettre. Ordre `(slug, num)` ; pas de
    /// pagination SQL.
    #[tracing::instrument(name = "db.iter_referential_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_referential_for_sitemap(
        &self,
    ) -> Result<Vec<(String, String, Option<NaiveDate>)>> {
        let (valid_a, norm_a, pick_a) = (
            article_valid_now("a"),
            article_is_norm("a"),
            article_pick_order("a"),
        );
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT DISTINCT ON (t.slug, a.num_key)
                       t.slug, a.num_key,
                       COALESCE(t.last_modified, a.date_debut)::date AS lastmod
                FROM legal_article a
                JOIN legal_text t ON t.text_uid = a.text_uid
                WHERE {valid_a} AND {norm_a} AND t.slug IS NOT NULL
                ORDER BY t.slug, a.num_key, {pick_a}
                "
                ),
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Itère `(slug, lastmod)` pour les codes navigables (pages TDM
    /// `/texte/{slug}`). Même filtre que `list_legal_texts` : `slug` non nul,
    /// nature navigable-comme-un-code, ≥1 article en vigueur. `lastmod` =
    /// `t.last_modified` brut (nullable, sentinelles comprises). Ordre déterministe
    /// par slug ; `build_sitemaps` pagine en mémoire.
    #[tracing::instrument(name = "db.iter_codes_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_codes_for_sitemap(&self) -> Result<Vec<(String, Option<NaiveDate>)>> {
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT t.slug, t.last_modified::date AS lastmod
                FROM legal_text t
                WHERE t.slug IS NOT NULL
                  AND (t.nature ILIKE 'code%'
                       OR upper(t.nature) IN ('CONSTITUTION', 'LOI_CONSTIT', 'LOI',
                           'LOI_ORGANIQUE', 'ORDONNANCE', 'DECRET_LOI', 'REGLEMENT',
                           'ETAT_CIVIL'))
                  AND EXISTS (SELECT 1 FROM legal_article a
                              WHERE a.text_uid = t.text_uid
                                AND {valid_a} AND {norm_a})
                ORDER BY t.slug
                "
                ),
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Rafraîchit les colonnes dénormalisées de `legal_article` depuis `legal_text`
    /// pour les articles dont elles diffèrent : `code_title` et les colonnes de
    /// recherche `jurisdiction`/`nature` (upper)/`slug`/`searchable`. Appelé en fin
    /// d'ingest référentiel, LEGI streamant articles et codes séparément. Pose aussi
    /// [`ARTICLE_KIND_EXPR`]. Renvoie le nombre de lignes mises à jour.
    #[tracing::instrument(name = "db.refresh_article_denorm", skip(self), fields(db.system = "postgresql"))]
    pub async fn refresh_article_denorm(&self) -> Result<u64> {
        self.conn.batch_execute("BEGIN").await?;
        let updated: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            let kind = ARTICLE_KIND_EXPR;
            let n = self
                .conn
                .execute(
                    &format!(
                        "UPDATE legal_article a SET \
                           code_title = t.title, \
                           jurisdiction = t.jurisdiction, \
                           nature = upper(t.nature), \
                           slug = t.slug, \
                           kind = {kind}, \
                           searchable = (t.slug IS NOT NULL \
                             AND t.role NOT IN ('individuel', 'vehicule', 'habilitation')) \
                         FROM legal_text t \
                         WHERE a.text_uid = t.text_uid \
                           AND (a.code_title IS DISTINCT FROM t.title \
                             OR a.jurisdiction IS DISTINCT FROM t.jurisdiction \
                             OR a.nature IS DISTINCT FROM upper(t.nature) \
                             OR a.slug IS DISTINCT FROM t.slug \
                             OR a.kind IS DISTINCT FROM {kind} \
                             OR a.searchable IS DISTINCT FROM (t.slug IS NOT NULL \
                               AND t.role NOT IN ('individuel', 'vehicule', 'habilitation')))"
                    ),
                    &[],
                )
                .await?;
            Ok(n)
        }
        .await;
        match updated {
            Ok(n) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(n)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Recalcule l'ordre de lecture des textes sales : ceux dont au moins un
    /// article a `reading_rank IS NULL`. Seul écrivain d'un rang ; le chemin
    /// d'écriture ne fait qu'invalider. Boucle par lots jusqu'à épuisement. Renvoie
    /// (textes traités, lignes écrites).
    #[tracing::instrument(name = "db.refresh_reading_ranks", skip(self), fields(db.system = "postgresql"))]
    pub async fn refresh_reading_ranks(&self) -> Result<(u64, u64)> {
        /// Lot de textes par aller-retour.
        const BATCH: i64 = 500;
        let (mut texts, mut rows) = (0u64, 0u64);
        loop {
            self.conn.batch_execute("BEGIN").await?;
            let batch: Result<(u64, u64)> = async {
                self.conn
                    .batch_execute("SET LOCAL statement_timeout = 0")
                    .await?;
                self.refresh_reading_ranks_batch(BATCH).await
            }
            .await;
            let (batch_texts, batch_rows) = match batch {
                Ok(v) => {
                    self.conn.batch_execute("COMMIT").await?;
                    v
                }
                Err(e) => {
                    let _ = self.conn.batch_execute("ROLLBACK").await;
                    return Err(e);
                }
            };
            if batch_texts == 0 {
                return Ok((texts, rows));
            }
            texts += batch_texts;
            rows += batch_rows;
            tracing::debug!(texts, rows, "reading ranks");
        }
    }

    /// Un lot du finisseur : les `limit` premiers textes sales, recalculés et
    /// réécrits. Renvoie (textes, lignes écrites) ; `(0, 0)` quand plus rien n'est
    /// sale.
    async fn refresh_reading_ranks_batch(&self, limit: i64) -> Result<(u64, u64)> {
        let uids: Vec<String> = self
            .conn
            .query(
                "SELECT DISTINCT text_uid FROM legal_article
                     WHERE reading_rank IS NULL LIMIT $1",
                &[&limit],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        if uids.is_empty() {
            return Ok((0, 0));
        }
        // Version canonique de chaque article : le rang est une propriété de
        // `(text_uid, num_key)`, partagée par ses versions.
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let inputs = self
            .conn
            .query(
                &format!(
                    "SELECT DISTINCT ON (text_uid, num_key)
                            text_uid, num_key, title_path, position
                     FROM legal_article a
                     WHERE text_uid = ANY($1)
                     ORDER BY text_uid, num_key, ({valid_a}) DESC, ({norm_a}) DESC,
                              (source = 'jorf') ASC, date_debut DESC NULLS LAST"
                ),
                &[&uids],
            )
            .await?;
        let mut per_text: std::collections::HashMap<String, Vec<ReadingOrderInput>> =
            std::collections::HashMap::new();
        for r in &inputs {
            per_text
                .entry(r.get(0))
                .or_default()
                .push(ReadingOrderInput {
                    num_key: r.get(1),
                    title_path: r.get(2),
                    position: r.get(3),
                });
        }
        // Ordre structurel réel quand le texte a un arbre ingéré : la séquence DILA
        // est l'autorité. Anchor `owner_uid`, `text_uid` porté le long de la récursion
        // pour traiter tout le lot en une requête.
        let structural = self
            .conn
            .query(
                "WITH RECURSIVE tree AS (
                   SELECT e.owner_uid AS root, e.child_kind, e.child_uid,
                          e.child_num_key, ARRAY[e.seq] AS path, 1 AS depth
                   FROM legal_toc_edge e
                   WHERE e.owner_uid = ANY($1)
                     AND e.date_debut <= current_date
                     AND (e.date_fin IS NULL OR e.date_fin >= current_date)
                   UNION ALL
                   SELECT t.root, c.child_kind, c.child_uid, c.child_num_key,
                          t.path || c.seq, t.depth + 1
                   FROM tree t JOIN legal_toc_edge c ON c.owner_uid = t.child_uid
                   WHERE t.child_kind = 'section' AND t.depth < 32
                     AND c.date_debut <= current_date
                     AND (c.date_fin IS NULL OR c.date_fin >= current_date)
                 )
                 SELECT root, child_num_key,
                        row_number() OVER (PARTITION BY root ORDER BY path) AS rn
                 FROM tree WHERE child_kind = 'article' AND child_num_key IS NOT NULL",
                &[&uids],
            )
            .await?;
        let mut structural_order: std::collections::HashMap<
            String,
            std::collections::HashMap<String, i64>,
        > = std::collections::HashMap::new();
        for r in &structural {
            structural_order
                .entry(r.get(0))
                .or_default()
                .insert(r.get(1), r.get(2));
        }

        let (mut u, mut k, mut ranks) = (Vec::new(), Vec::new(), Vec::new());
        for (text_uid, mut entries) in per_text {
            // Repli d'abord : il ordonne les articles absents de l'arbre du jour
            // (abrogés, transférés, versions futures), qui se rangent derrière.
            lj_core::article_order::sort_text_reading_order(
                &mut entries,
                |e| &e.num_key,
                |e| e.title_path.as_deref(),
                |e| e.position,
            );
            if let Some(order) = structural_order.get(&text_uid) {
                entries.sort_by_key(|e| order.get(&e.num_key).copied().unwrap_or(i64::MAX));
            }
            for (rank, e) in entries.into_iter().enumerate() {
                u.push(text_uid.clone());
                k.push(e.num_key);
                ranks.push(rank as i32);
            }
        }
        let written = self
            .conn
            .execute(
                "UPDATE legal_article a SET reading_rank = m.rank
                     FROM unnest($1::text[], $2::text[], $3::int[])
                          AS m(text_uid, num_key, rank)
                     WHERE a.text_uid = m.text_uid AND a.num_key = m.num_key
                       AND a.reading_rank IS DISTINCT FROM m.rank",
                &[&u, &k, &ranks],
            )
            .await?;
        Ok((uids.len() as u64, written))
    }

    /// Résout la cible d'un texte nommé en `text_uid`. Uid → tel quel ; slug
    /// d'alias et motifs de slug d'instrument → lookup, borné aux textes visibles à
    /// ≥ 1 article `VIGUEUR` ; à motifs ambigus, le texte au plus d'articles en
    /// vigueur gagne.
    async fn resolve_named_text(
        &self,
        target: lj_core::aliases::NamedTextTarget,
    ) -> Result<Option<String>> {
        use lj_core::aliases::NamedTextTarget as T;
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let sql = |cond: &str| {
            format!(
                "
                SELECT t.text_uid
                FROM legal_text t
                WHERE {cond}
                  AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                  AND {TEXT_ROLE_VISIBLE_SQL}
                  AND EXISTS (SELECT 1 FROM legal_article a
                              WHERE a.text_uid = t.text_uid
                                AND {valid_a} AND {norm_a})
                ORDER BY (SELECT count(*) FROM legal_article a
                          WHERE a.text_uid = t.text_uid
                            AND {valid_a} AND {norm_a}) DESC
                LIMIT 1
                "
            )
        };
        let rows = match &target {
            T::Uid(uid) => return Ok(Some(uid.clone())),
            T::Slug(slug) => self.conn.query(&sql("t.slug = $1"), &[slug]).await?,
            T::SlugPatterns(pats) => {
                self.conn
                    .query(&sql("t.slug LIKE ANY($1)"), &[pats])
                    .await?
            }
        };
        Ok(rows.first().map(|r| r.get(0)))
    }

    /// Catalogue `(text_uid, titre)` des codes navigables pour la jambe texte nommé
    /// de [`Self::search_articles`] : nature navigable, rôle visible, ≥ 1 article
    /// `VIGUEUR`, droit applicable en France, titre en « Code … ».
    async fn named_code_catalog(&self) -> Result<Vec<(String, String)>> {
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let rows = self
            .conn
            .query(
                &format!(
                    "
                    SELECT t.text_uid, t.title
                    FROM legal_text t
                    WHERE t.title ILIKE 'code %'
                      AND t.jurisdiction IN ('FR', 'UE', 'INTL')
                      AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                      AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                      AND EXISTS (SELECT 1 FROM legal_article a
                                  WHERE a.text_uid = t.text_uid
                                    AND {valid_a} AND {norm_a})
                    "
                ),
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Recherche plein-texte d'articles (`/recherche-textes`), titre-primaire. Par
    /// jambe, le prédicat combine titre conjonctif normalisé, filet titre OR et
    /// corps ([`article_search_predicates`]) ; la fusion entre les jambes se fait
    /// par rang (RRF, `1/(k + rang)`, jambes bornées à [`ARTICLE_LEG_LIMIT`]).
    ///
    /// Cinq jambes : articles domestiques (FR/UE/INTL + pays nommés), articles
    /// étrangers pondérés [`ARTICLE_FOREIGN_WEIGHT`], articles du pays nommé
    /// (requête débarrassée des tokens pays, les juridictions nommées sortant alors
    /// de la jambe domestique), textes à corps, et conteneurs (textes navigables
    /// comme un code, sans corps mais à articles — un hit `num = ''` vers
    /// `/texte/{slug}`, prioritaire à rang égal). Plus, quand la requête nomme un
    /// code du catalogue, les jambes code nommé : articles puis sacs d'usage du
    /// code visé, requête débarrassée du nom de code.
    ///
    /// Articles valides à `at` (fenêtre de validité ; `None` = aujourd'hui), une
    /// ligne par article, optionnellement bornés à un `text_uid`.
    /// `slug`/`code_title` joints pour le lien `/texte/{slug}/{num}` ; `score` =
    /// score RRF fusionné. La recherche à date déplace la fenêtre, pas le corpus :
    /// on sert la version couvrant `at` des articles cherchables d'aujourd'hui.
    #[tracing::instrument(name = "db.search_articles", skip(self, expansions), fields(db.system = "postgresql", limit, offset))]
    #[allow(clippy::too_many_arguments)]
    pub async fn search_articles(
        &self,
        query: &str,
        expansions: &[String],
        text_uid: Option<&str>,
        jurisdiction: Option<&str>,
        nature: Option<&str>,
        source: Option<&str>,
        nature_set: Option<(&[String], bool)>,
        at: Option<NaiveDate>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ArticleSearchRow>> {
        // `$1` titre OR (requête enrichie), `$2` corps (requête brute ; la clause
        // usage_terms est ajoutée boostée dans [`article_search_predicates`]),
        // alternatives conjonctives en `$3..`, puis filtres optionnels et pagination.
        let mut params = ArticleSearchParams::new(article_title_query(query, expansions), query);
        let (art_query, txt_query) = article_search_predicates(&mut params, query, expansions);
        let art_pred = format!("a.id @@@ {art_query}");
        let (art_filters, txt_filters) =
            params.push_filters(text_uid, jurisdiction, nature, source, nature_set);
        let prim_ph = params.push(Box::new(primary_jurisdictions(query)));
        let date_sql = sql_date_expr(at);
        let (valid_a, pick_a) = (article_valid_at("a", &date_sql), article_pick_order("a"));
        let (valid_a2, pick_a2) = (article_valid_at("a2", &date_sql), article_pick_order("a2"));
        let (norm_a, norm_a2) = (article_is_norm("a"), article_is_norm("a2"));
        // Jambe « pays nommé » : articles des juridictions que la requête nomme,
        // requête débarrassée des tokens pays ; la jambe domestique les exclut.
        let (art_ctry_excl, ctry_cte, ctry_union) =
            match lj_core::jurisdictions::strip_query_jurisdictions(query) {
                Some((codes, stripped)) => {
                    let codes: Vec<String> = codes.into_iter().map(String::from).collect();
                    let codes_ph = params.push(Box::new(codes));
                    let conj_clause = match lj_core::aliases::conj_title_query(&stripped) {
                        Some(c) => {
                            let cph = params.push(Box::new(c));
                            format!(
                                "paradedb.boost({ARTICLE_TITLE_CONJ_BOOST}, \
                                 paradedb.match('search_title', ${cph}, \
                                 conjunction_mode => true)), "
                            )
                        }
                        None => String::new(),
                    };
                    let s_ph = params.push(Box::new(stripped));
                    (
                        format!("\n  AND a.jurisdiction <> ALL(${codes_ph})"),
                        format!(
                            "
                    ctry AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ paradedb.boolean(should => ARRAY[{conj_clause}\
                                paradedb.boost({ARTICLE_TITLE_OR_BOOST}, paradedb.match('search_title', ${s_ph})), \
                                paradedb.match('texte', ${s_ph})])
                            AND {valid_a}
                            AND {norm_a}
                            AND a.searchable
                            AND a.jurisdiction = ANY(${codes_ph}){art_filters}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT {ARTICLE_LEG_LIMIT}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                      ) x
                    ),"
                        ),
                        format!(
                            "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, -1 AS leg
                      FROM ctry"
                        ),
                    )
                }
                None => (String::new(), String::new(), String::new()),
            };
        // Jambes « texte nommé » : requête débarrassée du nom du texte, bornée au
        // texte visé (articles + sacs d'usage). Votes sommés avec les jambes globales.
        let catalog = if lj_core::aliases::query_names_code(query) {
            self.named_code_catalog().await?
        } else {
            Vec::new()
        };
        let named_code = match lj_core::aliases::strip_query_named_text(query, &catalog) {
            Some((target, stripped)) => self
                .resolve_named_text(target)
                .await?
                .map(|uid| (uid, stripped)),
            None => None,
        };
        let (nmd_cte, nmd_union) = match &named_code {
            Some((uid, stripped)) => {
                let uid_ph = params.push(Box::new(uid.clone()));
                let conj_clause = match lj_core::aliases::conj_title_query(stripped) {
                    Some(c) => {
                        let cph = params.push(Box::new(c));
                        format!(
                            "paradedb.boost({ARTICLE_TITLE_CONJ_BOOST}, \
                             paradedb.match('search_title', ${cph}, \
                             conjunction_mode => true)), "
                        )
                    }
                    None => String::new(),
                };
                let s_ph = params.push(Box::new(stripped.clone()));
                let mut cte = format!(
                    "
                    nmd AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ paradedb.boolean(should => ARRAY[{conj_clause}\
                                paradedb.boost({ARTICLE_TITLE_OR_BOOST}, paradedb.match('search_title', ${s_ph})), \
                                paradedb.match('texte', ${s_ph})])
                            AND {valid_a}
                            AND {norm_a}
                            AND a.searchable
                            AND a.text_uid = ${uid_ph}{art_filters}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT {ARTICLE_LEG_LIMIT}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                      ) x
                    ),"
                );
                let mut union = format!(
                    "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, -3 AS leg
                      FROM nmd"
                );
                if !lj_core::usage::usage_reference_or_nav_query(stripped) {
                    let g_ph = params.push(Box::new(lj_core::usage::usage_grams(stripped)));
                    cte.push_str(&format!(
                        "
                    nmd_us AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.texte, paradedb.score(u.id) AS score
                        FROM legal_article_usage u
                        JOIN LATERAL (
                          SELECT a2.text_uid, a2.slug, a2.code_title, a2.num,
                                 a2.num_key, a2.title_path, a2.status,
                                 a2.source, a2.date_debut, a2.texte, a2.searchable,
                                 a2.jurisdiction, a2.nature
                          FROM legal_article a2
                          WHERE a2.text_uid = u.text_uid AND a2.num_key = u.num_key
                            AND {valid_a2}
                            AND {norm_a2}
                          ORDER BY {pick_a2} LIMIT 1
                        ) a ON true
                        WHERE u.id @@@ paradedb.match('terms', ${g_ph})
                          AND u.text_uid = ${uid_ph}
                          AND a.searchable{art_filters}
                        ORDER BY paradedb.score(u.id) DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) x
                    ),"
                    ));
                    union.push_str(&format!(
                        "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, texte,
                             {ARTICLE_NAMED_USAGE_WEIGHT} / ({ARTICLE_RRF_K} + rk) AS rrf, 4 AS leg
                      FROM nmd_us"
                    ));
                }
                (cte, union)
            }
            None => (String::new(), String::new()),
        };
        // Jambe conteneurs : forme conjonctive de la requête + expansions d'alias
        // pleine requête.
        let mut cont_alts: Vec<String> = Vec::new();
        cont_alts.extend(lj_core::aliases::conj_title_query(query));
        cont_alts.extend(
            lj_core::aliases::whole_query_expansions(query)
                .iter()
                .filter_map(|e| lj_core::aliases::conj_title_query(e)),
        );
        cont_alts.dedup();
        let (cont_cte, cont_union) = match cont_alts.as_slice() {
            [_, ..] => {
                let clauses: Vec<String> = cont_alts
                    .iter()
                    .map(|alt| {
                        let ph = params.push(Box::new(alt.clone()));
                        format!("paradedb.match('title', ${ph}, conjunction_mode => true)")
                    })
                    .collect();
                let cont_match = match clauses.as_slice() {
                    [single] => single.clone(),
                    many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
                };
                (
                    // Fence `OFFSET 0` : seuls le match titre et la juridiction descendent dans le
                    // scan ParadeDB, les filtres non indexables restent au-dessus.
                    format!(
                        "
                    cont AS (
                      SELECT z.*, row_number() OVER (ORDER BY z.score DESC, z.slug) AS rk
                      FROM (
                        SELECT t.text_uid, t.slug, t.title,
                               coalesce(t.status, 'VIGUEUR') AS status,
                               lower(t.nature) AS source,
                               t.score
                        FROM (
                          SELECT t.text_uid, t.slug, t.title, t.status, t.nature,
                                 t.role, t.jurisdiction, (t.body IS NULL) AS no_body,
                                 paradedb.score(t.id) AS score
                          FROM legal_text t
                          WHERE t.id @@@ {cont_match}
                            AND t.jurisdiction = ANY(${prim_ph})
                          OFFSET 0
                        ) t
                        WHERE t.no_body
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                          AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                          AND EXISTS (SELECT 1 FROM legal_article a
                                      WHERE a.text_uid = t.text_uid
                                        AND {valid_a} AND {norm_a}){txt_filters}
                        ORDER BY t.score DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) z
                    ),"
                    ),
                    format!(
                        "
                      UNION ALL
                      SELECT text_uid, slug, title, '' AS num, '' AS num_key,
                             NULL AS title_path, status, source,
                             NULL::date AS date_debut, NULL::text AS texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, -2 AS leg
                      FROM cont"
                    ),
                )
            }
            [] => (String::new(), String::new()),
        };
        // Jambe « usage » : grammes de la requête contre les sacs de contextes de
        // citation (`legal_article.usage_terms`). Elle recouvre la jambe articles, ses
        // votes se somment. Coupée sur les requêtes-référence et navigationnelles.
        let (us_cte, us_union) = if lj_core::usage::usage_reference_or_nav_query(query) {
            (String::new(), String::new())
        } else {
            let g_ph = params.push(Box::new(lj_core::usage::usage_grams(query)));
            (
                format!(
                    "
                    us AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.texte, paradedb.score(u.id) AS score
                        FROM legal_article_usage u
                        JOIN LATERAL (
                          SELECT a2.text_uid, a2.slug, a2.code_title, a2.num,
                                 a2.num_key, a2.title_path, a2.status,
                                 a2.source, a2.date_debut, a2.texte, a2.searchable,
                                 a2.jurisdiction, a2.nature
                          FROM legal_article a2
                          WHERE a2.text_uid = u.text_uid AND a2.num_key = u.num_key
                            AND {valid_a2}
                            AND {norm_a2}
                          ORDER BY {pick_a2} LIMIT 1
                        ) a ON true
                        WHERE u.id @@@ paradedb.match('terms', ${g_ph})
                          AND a.searchable{art_filters}
                        ORDER BY paradedb.score(u.id) DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) x
                    ),"
                ),
                format!(
                    "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, texte,
                             {ARTICLE_USAGE_WEIGHT} / ({ARTICLE_RRF_K} + rk) AS rrf, 3 AS leg
                      FROM us"
                ),
            )
        };
        // Jambe usage scopée au numéro cité : les sacs de contextes de citation des
        // seuls articles qui portent le numéro. Le scope passe par le littéral
        // `num_key` de l'index, sous `const_score(0, …)` : clause requise, poussée dans
        // le scan Tantivy, sans peser sur le score.
        let (nus_cte, nus_union) = match lj_core::article_key::query_article_keys(query).as_slice()
        {
            [] => (String::new(), String::new()),
            keys => {
                let k_ph = params.push(Box::new(keys.to_vec()));
                let g_ph = params.push(Box::new(lj_core::usage::usage_grams(query)));
                (
                    format!(
                        "
                    nus AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.texte, paradedb.score(u.id) AS score
                        FROM legal_article_usage u
                        JOIN LATERAL (
                          SELECT a2.text_uid, a2.slug, a2.code_title, a2.num,
                                 a2.num_key, a2.title_path, a2.status,
                                 a2.source, a2.date_debut, a2.texte, a2.searchable,
                                 a2.jurisdiction, a2.nature
                          FROM legal_article a2
                          WHERE a2.text_uid = u.text_uid AND a2.num_key = u.num_key
                            AND {valid_a2}
                            AND {norm_a2}
                          ORDER BY {pick_a2} LIMIT 1
                        ) a ON true
                        WHERE u.id @@@ paradedb.boolean(must => ARRAY[
                                paradedb.match('terms', ${g_ph}),
                                paradedb.const_score(0, paradedb.term_set('num_key', ${k_ph}::text[]))])
                          AND a.searchable{art_filters}
                        ORDER BY paradedb.score(u.id) DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) x
                    ),"
                    ),
                    format!(
                        "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, texte,
                             {ARTICLE_USAGE_WEIGHT} / ({ARTICLE_RRF_K} + rk) AS rrf, 5 AS leg
                      FROM nus"
                    ),
                )
            }
        };
        let limit_ph = params.push(Box::new(limit));
        let offset_ph = params.push(Box::new(offset));
        // Jambes articles domestique et étrangère : deux scans ParadeDB single-table
        // TopK — prédicat et filtres indexés + `ORDER BY score LIMIT` dans le scan.
        // Les colonnes d'affichage ne sont jointes que pour les ≤ leg_limit lignes
        // retenues. Coupe du top-K par score seul, puis fusion par rang. Tiebreak
        // final par `leg` : conteneur, pays nommé, articles domestiques, textes à
        // corps, articles étrangers.
        let sql = format!(
            "
                    WITH{cont_cte}{ctry_cte}{nmd_cte}{us_cte}{nus_cte}
                    art AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE {art_pred}
                            AND {valid_a}
                            AND {norm_a}
                            AND a.searchable
                            AND a.jurisdiction = ANY(${prim_ph}){art_ctry_excl}{art_filters}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT {ARTICLE_LEG_LIMIT}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                      ) x
                    ),
                    art_f AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE {art_pred}
                            AND {valid_a}
                            AND {norm_a}
                            AND a.searchable
                            AND NOT (a.jurisdiction = ANY(${prim_ph})){art_ctry_excl}{art_filters}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT {ARTICLE_LEG_LIMIT}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                      ) x
                    ),
                    txt AS (
                      SELECT y.*, row_number() OVER (ORDER BY y.score DESC, y.slug) AS rk
                      FROM (
                        SELECT t.text_uid, t.slug, t.title,
                               coalesce(t.status, 'VIGUEUR') AS status,
                               lower(t.nature) AS source, t.body AS texte,
                               paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE t.id @@@ {txt_query}
                          AND t.body IS NOT NULL AND {TEXT_ROLE_VISIBLE_SQL}
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL{txt_filters}
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) y
                    )
                    SELECT u.text_uid, u.slug, u.title, u.num, u.num_key,
                           u.title_path, u.status, u.source, u.date_debut, u.texte,
                           sum(u.rrf)::float4 AS score
                    FROM (
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, 0 AS leg
                      FROM art
                      UNION ALL
                      SELECT text_uid, slug, title, '' AS num, '' AS num_key,
                             NULL AS title_path, status, source,
                             NULL::date AS date_debut, texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, 1 AS leg
                      FROM txt
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, texte,
                             {ARTICLE_FOREIGN_WEIGHT} / ({ARTICLE_RRF_K} + rk) AS rrf, 2 AS leg
                      FROM art_f{ctry_union}{cont_union}{nmd_union}{us_union}{nus_union}
                    ) u
                    GROUP BY u.text_uid, u.slug, u.title, u.num, u.num_key,
                             u.title_path, u.status, u.source, u.date_debut, u.texte
                    ORDER BY sum(u.rrf) DESC, min(u.leg), u.num_key
                    LIMIT ${limit_ph} OFFSET ${offset_ph}
                    "
        );
        let rows = self.conn.query(&sql, &params.refs()).await?;
        Ok(rows
            .iter()
            .map(|r| ArticleSearchRow {
                text_uid: r.get(0),
                slug: r.get(1),
                code_title: r.get(2),
                num: r.get(3),
                num_key: r.get(4),
                title_path: r.get(5),
                status: r.get(6),
                source: r.get(7),
                date_debut: r.get(8),
                texte: r.get(9),
                score: r.get(10),
            })
            .collect())
    }

    /// Total exact + les quatre facettes de la recherche d'articles, sous le même
    /// prédicat BM25 + filtres que [`Self::search_articles`]. Les membres articles
    /// et textes à corps sont comptés chacun par une passe `paradedb.aggregate` —
    /// les quatre `terms` + le total `value_count`, tout le prédicat exprimé en
    /// `searchqueryinput` ; le membre conteneurs reste en SQL. Fusion des comptes
    /// par valeur côté app, sur le prédicat complet filtres inclus. Chaque axe est
    /// trié count décroissant puis valeur ascendante ; les valeurs vides sont
    /// écartées, l'axe `code` plafonné à son top-[`ARTICLE_CODE_FACET_LIMIT`] et
    /// `nature` normalisée `upper()`.
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(name = "db.article_search_stats", skip(self, expansions), fields(db.system = "postgresql"))]
    pub async fn article_search_stats(
        &self,
        query: &str,
        expansions: &[String],
        text_uid: Option<&str>,
        jurisdiction: Option<&str>,
        nature: Option<&str>,
        source: Option<&str>,
        nature_set: Option<(&[String], bool)>,
        at: Option<NaiveDate>,
    ) -> Result<ArticleSearchStats> {
        // Corps = requête brute ; la clause usage_terms est ajoutée par
        // `article_search_predicates`. Les filtres SQL deviennent des clauses `must` du
        // `searchqueryinput`. Jeu de paramètres dédié : chaque placeholder poussé doit
        // être référencé par la requête qui le porte.
        let mut params_art =
            ArticleSearchParams::new(article_title_query(query, expansions), query);
        let (art_query, _) = article_search_predicates(&mut params_art, query, expansions);
        let mut agg_filters = String::new();
        for (field, value) in [
            ("text_uid", text_uid.map(str::to_string)),
            ("jurisdiction", jurisdiction.map(str::to_string)),
            ("nature", nature.map(str::to_uppercase)),
            ("source", source.map(str::to_string)),
        ] {
            if let Some(v) = value {
                let ph = params_art.push(Box::new(v));
                agg_filters.push_str(&format!(", paradedb.term('{field}', ${ph})"));
            }
        }
        let mut agg_must_not = String::new();
        if let Some((natures, include)) = nature_set.filter(|(n, _)| !n.is_empty()) {
            let terms: Vec<String> = natures
                .iter()
                .map(|n| {
                    let ph = params_art.push(Box::new(n.clone()));
                    format!("paradedb.term('nature', ${ph})")
                })
                .collect();
            let set = format!("paradedb.boolean(should => ARRAY[{}])", terms.join(", "));
            if include {
                agg_filters.push_str(&format!(", {set}"));
            } else {
                // SQL `<> ALL` écarte aussi les natures NULL ; en Tantivy un doc sans champ ne
                // matche pas le `must_not` — `exists()` rétablit la même sémantique.
                agg_filters.push_str(", paradedb.exists('nature')");
                agg_must_not = format!(", must_not => ARRAY[{set}]");
            }
        }
        let valid_agg = article_valid_agg(&sql_date_expr(at));
        let agg_row = self
            .conn
            .query_one(
                &format!(
                    "SELECT paradedb.aggregate('legal_article_bm25', \
                       paradedb.boolean(must => ARRAY[{art_query}, \
                         {valid_agg}, \
                         paradedb.term('kind', 'article'), \
                         paradedb.term('searchable', true){agg_filters}]{agg_must_not}), \
                       '{ARTICLE_FACETS_AGG}'::json, \
                       bucket_limit => {ARTICLE_FACETS_BUCKET_LIMIT})::text"
                ),
                &params_art.refs(),
            )
            .await?;
        let agg_art: serde_json::Value = serde_json::from_str(agg_row.get(0))
            .map_err(|e| StoreError::Invalid(format!("agrégat facettes articles: {e}")))?;

        let mut params_txt =
            ArticleSearchParams::new(article_title_query(query, expansions), query);
        let txt_query = article_text_predicate(&mut params_txt, query, expansions);
        let mut txt_filters_agg = String::new();
        for (field, value) in [
            ("text_uid", text_uid.map(str::to_string)),
            ("jurisdiction", jurisdiction.map(str::to_string)),
            ("nature_up", nature.map(str::to_uppercase)),
            ("source", source.map(str::to_string)),
        ] {
            if let Some(v) = value {
                let ph = params_txt.push(Box::new(v));
                txt_filters_agg.push_str(&format!(", paradedb.term('{field}', ${ph})"));
            }
        }
        let mut txt_must_not = vec![TEXT_ROLE_HIDDEN_TERMS.to_string()];
        if let Some((natures, include)) = nature_set.filter(|(n, _)| !n.is_empty()) {
            let terms: Vec<String> = natures
                .iter()
                .map(|n| {
                    let ph = params_txt.push(Box::new(n.clone()));
                    format!("paradedb.term('nature_up', ${ph})")
                })
                .collect();
            let set = format!("paradedb.boolean(should => ARRAY[{}])", terms.join(", "));
            // `legal_text.nature` est NOT NULL : `<> ALL` n'écarte aucune ligne à nature
            // absente, un `must_not` nu a donc exactement la sémantique du SQL.
            if include {
                txt_filters_agg.push_str(&format!(", {set}"));
            } else {
                txt_must_not.push(set);
            }
        }
        let txt_agg_row = self
            .conn
            .query_one(
                &format!(
                    "SELECT paradedb.aggregate('legal_text_body_bm25', \
                       paradedb.boolean(must => ARRAY[{txt_query}, \
                         paradedb.term('has_body', true), \
                         paradedb.term('status_eff', 'VIGUEUR'), \
                         paradedb.exists('slug'){txt_filters_agg}], \
                         must_not => ARRAY[{}]), \
                       '{TEXT_FACETS_AGG}'::json, \
                       bucket_limit => {ARTICLE_FACETS_BUCKET_LIMIT})::text",
                    txt_must_not.join(", ")
                ),
                &params_txt.refs(),
            )
            .await?;
        let agg_txt: serde_json::Value = serde_json::from_str(txt_agg_row.get(0))
            .map_err(|e| StoreError::Invalid(format!("agrégat facettes textes: {e}")))?;

        let mut cont_alts: Vec<String> = Vec::new();
        cont_alts.extend(lj_core::aliases::conj_title_query(query));
        cont_alts.extend(
            lj_core::aliases::whole_query_expansions(query)
                .iter()
                .filter_map(|e| lj_core::aliases::conj_title_query(e)),
        );
        cont_alts.dedup();
        // `GROUPING(...)` encode le set actif en bitmask (bit levé = colonne NON
        // groupée) : code = 0b0111, jurisdiction = 0b1011, nature = 0b1101,
        // source = 0b1110, total = 0b1111.
        let rows = match cont_alts.as_slice() {
            [] => Vec::new(),
            [_, ..] => {
                let mut params = ArticleSearchParams::empty();
                let clauses: Vec<String> = cont_alts
                    .iter()
                    .map(|alt| {
                        let ph = params.push(Box::new(alt.clone()));
                        format!("paradedb.match('title', ${ph}, conjunction_mode => true)")
                    })
                    .collect();
                let cont_match = match clauses.as_slice() {
                    [single] => single.clone(),
                    many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
                };
                let prim_ph = params.push(Box::new(primary_jurisdictions(query)));
                let (_, txt_filters) =
                    params.push_filters(text_uid, jurisdiction, nature, source, nature_set);
                let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
                self.conn
                    .query(
                        &format!(
                            "
                    SELECT GROUPING(u.slug, u.jurisdiction, u.nature, u.source) AS gset,
                           u.slug, u.jurisdiction, u.nature, u.source, count(*) AS n
                    FROM (
                      SELECT t.slug, t.jurisdiction, upper(t.nature) AS nature,
                             lower(t.nature) AS source
                      FROM (
                        SELECT t.text_uid, t.slug, t.jurisdiction, t.nature,
                               t.status, t.role, (t.body IS NULL) AS no_body
                        FROM legal_text t
                        WHERE t.id @@@ {cont_match}
                          AND t.jurisdiction = ANY(${prim_ph})
                        OFFSET 0
                      ) t
                      WHERE t.no_body
                        AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                        AND t.slug IS NOT NULL
                        AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                        AND EXISTS (SELECT 1 FROM legal_article a
                                    WHERE a.text_uid = t.text_uid
                                      AND {valid_a} AND {norm_a}){txt_filters}
                    ) u
                    GROUP BY GROUPING SETS ((u.slug), (u.jurisdiction), (u.nature), (u.source), ())
                    "
                        ),
                        &params.refs(),
                    )
                    .await?
            }
        };
        // Fusion des membres : sommes par valeur d'axe. Une troncature de l'agrégat
        // Tantivy rendrait les comptes silencieusement faux → erreur franche.
        let mut axes: [std::collections::BTreeMap<String, i64>; 4] = Default::default();
        for (agg, membre) in [(&agg_art, "articles"), (&agg_txt, "textes")] {
            for (map, axis) in axes
                .iter_mut()
                .zip(["slug", "jurisdiction", "nature", "source"])
            {
                let terms = &agg[axis];
                // `slug` est coupé au top-100 dans l'agrégation même : un reste y est attendu.
                // Sur les autres axes, énumérés en entier, un reste signifie des comptes faux.
                if axis != "slug" && terms["sum_other_doc_count"].as_i64() != Some(0) {
                    return Err(StoreError::Invalid(format!(
                        "agrégat facettes {membre}: axe {axis} tronqué"
                    )));
                }
                for bucket in terms["buckets"].as_array().into_iter().flatten() {
                    let (Some(value), Some(count)) =
                        (bucket["key"].as_str(), bucket["doc_count"].as_i64())
                    else {
                        return Err(StoreError::Invalid(format!(
                            "agrégat facettes {membre}: bucket inattendu sur l'axe {axis}"
                        )));
                    };
                    if !value.is_empty() {
                        *map.entry(value.to_string()).or_default() += count;
                    }
                }
            }
        }
        let mut total = (agg_art["total"]["value"].as_f64().unwrap_or(0.0)
            + agg_txt["total"]["value"].as_f64().unwrap_or(0.0)) as i64;
        for r in &rows {
            let gset: i32 = r.get(0);
            let count: i64 = r.get(5);
            let (idx, value): (usize, Option<String>) = match gset {
                0b0111 => (0, r.get(1)),
                0b1011 => (1, r.get(2)),
                0b1101 => (2, r.get(3)),
                0b1110 => (3, r.get(4)),
                _ => {
                    total += count;
                    continue;
                }
            };
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                *axes[idx].entry(value).or_default() += count;
            }
        }
        let [mut code, jurisdiction, nature, source] = axes.map(|map| {
            let mut axis: Vec<FacetCount> = map
                .into_iter()
                .map(|(value, count)| FacetCount { value, count })
                .collect();
            axis.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
            axis
        });
        // L'axe code est plafonné au top-N : l'énumération complète sert aux comptes
        // exacts ci-dessus, elle n'est pas servie.
        code.truncate(ARTICLE_CODE_FACET_LIMIT);
        Ok(ArticleSearchStats {
            total,
            code,
            jurisdiction,
            nature,
            source,
        })
    }

    /// Bras du banc de ranking articles (`lj-bench article-rank-eval`) : les
    /// variantes du prédicat titre ([`ArticleTitleMode`]) et de la fusion
    /// inter-jambes (score brut vs RRF) de [`Self::search_articles`], sans filtre
    /// ni pagination.
    #[tracing::instrument(name = "db.search_articles_rank_arm", skip(self, expansions), fields(db.system = "postgresql", limit))]
    pub async fn search_articles_rank_arm(
        &self,
        query: &str,
        expansions: &[String],
        title_mode: ArticleTitleMode,
        or_boost: f64,
        rrf: Option<ArticleRrf>,
        limit: i64,
    ) -> Result<Vec<ArticleRankHit>> {
        // Jambe titre selon le mode : `Or` = requête enrichie ($1) boostée ×4 ;
        // `Conj` = un `should` d'alternatives toutes conjonctives, normalisées pour le
        // titre et devant matcher en entier ; `OrConj` = les deux clauses. Rien ne
        // subsiste après normalisation → repli Or. $1 porte la forme utile au mode
        // choisi, Postgres ne typant pas un placeholder jamais référencé.
        let mut conj_alts: Vec<String> = std::iter::once(query.to_string())
            .chain(expansions.iter().cloned())
            .filter_map(|q| lj_core::aliases::conj_title_query(&q))
            .collect();
        conj_alts.dedup();
        let title_mode = if conj_alts.is_empty() {
            ArticleTitleMode::Or
        } else {
            title_mode
        };
        let title_param = match title_mode {
            ArticleTitleMode::Conj => conj_alts[0].clone(),
            _ => article_title_query(query, expansions),
        };
        let mut params = ArticleSearchParams::new(title_param, query);
        let mut alt_phs: Vec<usize> = Vec::new();
        match title_mode {
            ArticleTitleMode::Or => {}
            ArticleTitleMode::Conj => {
                alt_phs.push(1);
                for alt in &conj_alts[1..] {
                    alt_phs.push(params.push(Box::new(alt.clone())));
                }
            }
            ArticleTitleMode::OrConj => {
                for alt in &conj_alts {
                    alt_phs.push(params.push(Box::new(alt.clone())));
                }
            }
        }
        let title_clause = |field: &str| {
            let or = format!("paradedb.match('{field}', $1)");
            let conj = || {
                let alts: Vec<String> = alt_phs
                    .iter()
                    .map(|ph| format!("paradedb.match('{field}', ${ph}, conjunction_mode => true)"))
                    .collect();
                match alts.as_slice() {
                    [single] => single.clone(),
                    many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
                }
            };
            match title_mode {
                ArticleTitleMode::Or => format!("paradedb.boost(4, {or})"),
                ArticleTitleMode::Conj => format!("paradedb.boost(4, {})", conj()),
                // `or_boost` module le filet OR, inliné : `paradedb.boost` n'accepte pas de
                // placeholder.
                ArticleTitleMode::OrConj => {
                    format!(
                        "paradedb.boost(4, {}), paradedb.boost({or_boost}, {or})",
                        conj()
                    )
                }
            }
        };
        let (valid_a, norm_a, pick_a) = (
            article_valid_now("a"),
            article_is_norm("a"),
            article_pick_order("a"),
        );
        let (valid_a2, norm_a2, pick_a2) = (
            article_valid_now("a2"),
            article_is_norm("a2"),
            article_pick_order("a2"),
        );
        let art_pred = format!(
            "a.id @@@ paradedb.boolean(should => ARRAY[{}, paradedb.match('texte', $2)]) \
             AND {valid_a} AND {norm_a} AND a.searchable",
            title_clause("search_title")
        );
        let txt_pred = format!(
            "t.id @@@ paradedb.boolean(should => ARRAY[{}, paradedb.match('body', $2)]) \
             AND t.body IS NOT NULL AND {TEXT_ROLE_VISIBLE_SQL}",
            title_clause("title")
        );

        let sql = match rrf {
            None => {
                let limit_ph = params.push(Box::new(limit));
                format!(
                    "
                    SELECT * FROM (
                      (SELECT DISTINCT ON (a.text_uid, a.num_key)
                             a.slug, a.num, a.num_key, a.code_title AS title,
                             a.title_path, a.texte,
                             paradedb.score(a.id) AS score
                      FROM legal_article a
                      WHERE {art_pred}
                      ORDER BY a.text_uid, a.num_key,
                               paradedb.score(a.id) DESC, {pick_a})
                      UNION ALL
                      SELECT t.slug, '' AS num, '' AS num_key, t.title,
                             NULL AS title_path, t.body AS texte,
                             paradedb.score(t.id) AS score
                      FROM legal_text t
                      WHERE {txt_pred}
                        AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                        AND t.slug IS NOT NULL
                    ) u
                    ORDER BY u.score DESC, u.num_key
                    LIMIT ${limit_ph}
                    "
                )
            }
            Some(ArticleRrf {
                k,
                txt_weight,
                leg_limit,
                split_title: true,
                ..
            }) => {
                let k_ph = params.push(Box::new(k));
                let w_ph = params.push(Box::new(txt_weight));
                let leg_ph = params.push(Box::new(leg_limit));
                let limit_ph = params.push(Box::new(limit));
                // 4 jambes mono-clause (titre et corps séparés). Scores RRF sommés par doc.
                // `$1` = requête titre enrichie (mode Or).
                format!(
                    "
                    WITH art_t AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.slug, a.num, a.num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ paradedb.match('search_title', $1)
                            AND {valid_a} AND {norm_a} AND a.searchable
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                      ) x
                    ),
                    art_b AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.slug, a.num, a.num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ paradedb.match('texte', $2)
                            AND {valid_a} AND {norm_a} AND a.searchable
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                      ) x
                    ),
                    txt_t AS (
                      SELECT y.*, row_number() OVER (ORDER BY y.score DESC, y.slug) AS rk
                      FROM (
                        SELECT t.slug, t.title, t.body AS texte, paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE t.id @@@ paradedb.match('title', $1)
                          AND t.body IS NOT NULL
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT ${leg_ph}
                      ) y
                    ),
                    txt_b AS (
                      SELECT y.*, row_number() OVER (ORDER BY y.score DESC, y.slug) AS rk
                      FROM (
                        SELECT t.slug, t.title, t.body AS texte, paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE t.id @@@ paradedb.match('body', $2)
                          AND t.body IS NOT NULL
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT ${leg_ph}
                      ) y
                    )
                    SELECT u.slug, u.num, u.title, u.title_path, u.texte FROM (
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, 0 AS leg
                      FROM art_t
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, 1 AS leg
                      FROM art_b
                      UNION ALL
                      SELECT slug, '' AS num, '' AS num_key, title,
                             NULL AS title_path, texte,
                             ${w_ph}::float8 / (${k_ph}::float8 + rk) AS rrf, 2 AS leg
                      FROM txt_t
                      UNION ALL
                      SELECT slug, '' AS num, '' AS num_key, title,
                             NULL AS title_path, texte,
                             ${w_ph}::float8 / (${k_ph}::float8 + rk) AS rrf, 3 AS leg
                      FROM txt_b
                    ) u
                    GROUP BY u.slug, u.num, u.num_key, u.title, u.title_path, u.texte
                    ORDER BY sum(u.rrf) DESC, min(u.leg), u.num_key
                    LIMIT ${limit_ph}
                    "
                )
            }
            Some(ArticleRrf {
                k,
                txt_weight,
                leg_limit,
                split_title: false,
                container,
                foreign_weight,
                container_alias,
                country_leg,
                foreign_score_merge,
                usage_weight,
                usage_table,
            }) => {
                let k_ph = params.push(Box::new(k));
                let w_ph = params.push(Box::new(txt_weight));
                let leg_ph = params.push(Box::new(leg_limit));
                let limit_ph = params.push(Box::new(limit));
                // Prior de juridiction optionnel : la jambe articles se scinde en domestique
                // et étrangère pondérée `foreign_weight`. Deux fusions : par rang (jambe RRF
                // séparée) ou par score (`foreign_score_merge`, membre UNION pondéré).
                let mut prim_ph: Option<usize> = None;
                let (art_juris, art_merge_union, artf_cte, artf_union) = match foreign_weight {
                    Some(w) if foreign_score_merge => {
                        let ph = params.push(Box::new(primary_jurisdictions(query)));
                        prim_ph = Some(ph);
                        (
                            format!("\n  AND a.jurisdiction = ANY(${ph})"),
                            format!(
                                "
                        UNION ALL
                        (SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.slug, a.num, a.num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) * {w}::float8 AS score
                          FROM legal_article a
                          WHERE {art_pred}
                            AND a.jurisdiction <> ALL(${ph})
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a})"
                            ),
                            String::new(),
                            String::new(),
                        )
                    }
                    Some(w) => {
                        let ph = params.push(Box::new(primary_jurisdictions(query)));
                        prim_ph = Some(ph);
                        (
                            format!("\n  AND a.jurisdiction = ANY(${ph})"),
                            String::new(),
                            format!(
                                "
                    art_f AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.slug, a.num, a.num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE {art_pred}
                            AND a.jurisdiction <> ALL(${ph})
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                      ) x
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             {w}::float8 / (${k_ph}::float8 + rk) AS rrf, 2 AS leg
                      FROM art_f"
                            ),
                        )
                    }
                    None => (String::new(), String::new(), String::new(), String::new()),
                };
                // Jambe « termes d'usage » optionnelle : requête en grammes contre les sacs de
                // contextes de citation, jointe à la version en vigueur.
                let usage_weight =
                    usage_weight.filter(|_| !lj_core::usage::usage_reference_or_nav_query(query));
                let (usage_cte, usage_union) = match usage_weight {
                    Some(uw) => {
                        let gq = lj_core::usage::usage_grams(query);
                        let g_ph = params.push(Box::new(gq));
                        let uw_ph = params.push(Box::new(uw));
                        let usage_table = usage_table.unwrap_or("legal_article_usage");
                        (
                            format!(
                                "
                    us AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT la.slug, la.num, u.num_key, la.code_title AS title,
                               la.title_path, la.texte,
                               paradedb.score(u.id) AS score
                        FROM {usage_table} u
                        JOIN LATERAL (
                          SELECT a2.slug, a2.code_title, a2.num, a2.title_path,
                                 a2.texte, a2.searchable
                          FROM legal_article a2
                          WHERE a2.text_uid = u.text_uid AND a2.num_key = u.num_key
                            AND {valid_a2} AND {norm_a2}
                          ORDER BY {pick_a2} LIMIT 1
                        ) la ON true
                        WHERE u.id @@@ paradedb.match('terms', ${g_ph})
                          AND la.searchable
                        ORDER BY paradedb.score(u.id) DESC
                        LIMIT ${leg_ph}
                      ) x
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             ${uw_ph}::float8 / (${k_ph}::float8 + rk) AS rrf, 3 AS leg
                      FROM us"
                            ),
                        )
                    }
                    None => (String::new(), String::new()),
                };
                // Fusion finale : les jambes historiques sont doc-disjointes, le merge-sort
                // des votes suffit. La jambe usage recouvre la jambe articles : ses votes se
                // somment par doc.
                let final_order = if usage_weight.is_some() {
                    "GROUP BY u.slug, u.num, u.num_key, u.title, u.title_path, u.texte
                    ORDER BY sum(u.rrf) DESC, min(u.leg), u.num_key"
                } else {
                    "ORDER BY u.rrf DESC, u.leg, u.num_key"
                };
                // Jambe « pays nommé » optionnelle ; la jambe articles exclut alors ces
                // juridictions. Tiebreak -1 : sous le conteneur, au-dessus du domestique.
                let ctry = country_leg
                    .then(|| lj_core::jurisdictions::strip_query_jurisdictions(query))
                    .flatten();
                let (art_ctry_excl, ctry_cte, ctry_union) = match ctry {
                    Some((codes, stripped)) => {
                        let codes: Vec<String> = codes.into_iter().map(String::from).collect();
                        let codes_ph = params.push(Box::new(codes));
                        let conj_clause = match lj_core::aliases::conj_title_query(&stripped) {
                            Some(c) => {
                                let cph = params.push(Box::new(c));
                                format!(
                                    "paradedb.boost({ARTICLE_TITLE_CONJ_BOOST}, \
                                         paradedb.match('search_title', ${cph}, \
                                         conjunction_mode => true)), "
                                )
                            }
                            None => String::new(),
                        };
                        let s_ph = params.push(Box::new(stripped));
                        (
                            format!("\n  AND a.jurisdiction <> ALL(${codes_ph})"),
                            format!(
                                "
                    ctry AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.slug, a.num, a.num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ paradedb.boolean(should => ARRAY[{conj_clause}\
                                paradedb.boost({ARTICLE_TITLE_OR_BOOST}, paradedb.match('search_title', ${s_ph})), \
                                paradedb.match('texte', ${s_ph})])
                            AND {valid_a} AND {norm_a} AND a.searchable
                            AND a.jurisdiction = ANY(${codes_ph})
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                      ) x
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, -1 AS leg
                      FROM ctry"
                            ),
                        )
                    }
                    None => (String::new(), String::new(), String::new()),
                };
                // Jambe conteneurs optionnelle : `legal_text` sans corps mais à articles en
                // vigueur, titre en conjonctif seul et sur la seule forme conjonctive de la
                // requête, bornée aux natures navigables comme un code et au prior de
                // juridiction quand il est actif. `leg = -1` : à rang RRF égal le conteneur
                // prime.
                let mut cont_alts: Vec<String> = Vec::new();
                if container {
                    cont_alts.extend(lj_core::aliases::conj_title_query(query));
                    if container_alias {
                        cont_alts.extend(
                            lj_core::aliases::whole_query_expansions(query)
                                .iter()
                                .filter_map(|e| lj_core::aliases::conj_title_query(e)),
                        );
                    }
                    cont_alts.dedup();
                }
                let (cont_cte, cont_union) = match cont_alts.as_slice() {
                    [_, ..] => {
                        let clauses: Vec<String> = cont_alts
                            .iter()
                            .map(|alt| {
                                let ph = params.push(Box::new(alt.clone()));
                                format!("paradedb.match('title', ${ph}, conjunction_mode => true)")
                            })
                            .collect();
                        let conj = match clauses.as_slice() {
                            [single] => single.clone(),
                            many => {
                                format!("paradedb.boolean(should => ARRAY[{}])", many.join(", "))
                            }
                        };
                        let cont_juris = prim_ph
                            .map(|ph| format!("\n  AND t.jurisdiction = ANY(${ph})"))
                            .unwrap_or_default();
                        (
                            format!(
                                "
                    cont AS (
                      SELECT z.*, row_number() OVER (ORDER BY z.score DESC, z.slug) AS rk
                      FROM (
                        SELECT t.slug, t.title, paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE t.id @@@ {conj}
                          AND t.body IS NULL
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                          AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                          AND EXISTS (SELECT 1 FROM legal_article a
                                      WHERE a.text_uid = t.text_uid
                                        AND {valid_a} AND {norm_a}){cont_juris}
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT ${leg_ph}
                      ) z
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, '' AS num, '' AS num_key, title,
                             NULL AS title_path, NULL AS texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, -2 AS leg
                      FROM cont"
                            ),
                        )
                    }
                    [] => (String::new(), String::new()),
                };
                // Chaque jambe est classée seule (l'ORDER BY interne reste sur le seul score
                // pour garder le Top-K scan ParadeDB), puis fusion par rang. Tiebreak : jambe
                // articles d'abord.
                format!(
                    "
                    WITH{cont_cte}{ctry_cte}{artf_cte}{usage_cte}
                    art AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        (SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.slug, a.num, a.num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE {art_pred}{art_juris}{art_ctry_excl}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}){art_merge_union}
                      ) x
                    ),
                    txt AS (
                      SELECT y.*, row_number() OVER (ORDER BY y.score DESC, y.slug) AS rk
                      FROM (
                        SELECT t.slug, t.title, t.body AS texte, paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE {txt_pred}
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT ${leg_ph}
                      ) y
                    )
                    SELECT u.slug, u.num, u.title, u.title_path, u.texte FROM (
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, 0 AS leg
                      FROM art
                      UNION ALL
                      SELECT slug, '' AS num, '' AS num_key, title,
                             NULL AS title_path, texte,
                             ${w_ph}::float8 / (${k_ph}::float8 + rk) AS rrf, 1 AS leg
                      FROM txt{artf_union}{usage_union}{ctry_union}{cont_union}
                    ) u
                    {final_order}
                    LIMIT ${limit_ph}
                    "
                )
            }
        };

        let rows = self.conn.query(&sql, &params.refs()).await?;
        Ok(rows
            .iter()
            .map(|r| match rrf {
                None => ArticleRankHit {
                    slug: r.get(0),
                    num: r.get(1),
                    code_title: r.get(3),
                    title_path: r.get(4),
                    texte: r.get(5),
                },
                Some(_) => ArticleRankHit {
                    slug: r.get(0),
                    num: r.get(1),
                    code_title: r.get(2),
                    title_path: r.get(3),
                    texte: r.get(4),
                },
            })
            .collect())
    }

    /// Compteurs corpus normatif pour la home (`GET /api/corpus-stats`) : nombre
    /// total de textes et d'articles en vigueur (identités `(text_uid, num_key)`
    /// distinctes). Corpus entier, pas le seul catalogue navigable. Les identités
    /// distinctes se comptent par `GROUP BY` en sous-requête : `count(DISTINCT …)`
    /// impose un tri sur un ROW composite qui déborde sur disque.
    #[tracing::instrument(name = "db.count_normative_corpus", skip(self), fields(db.system = "postgresql"))]
    pub async fn count_normative_corpus(&self) -> Result<(i64, i64)> {
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let row = self
            .conn
            .query_one(
                &format!(
                    "
                SELECT
                  (SELECT count(*) FROM legal_text) AS texts,
                  (SELECT count(*) FROM (
                     SELECT a.text_uid, a.num_key FROM legal_article a
                      WHERE {valid_a} AND {norm_a}
                      GROUP BY 1, 2) ids) AS articles
                "
                ),
                &[],
            )
            .await?;
        Ok((row.get(0), row.get(1)))
    }

    /// `head_only` borne le catalogue au fond `codes` (codes, constitutions, état
    /// civil). Le filtre est posé en SQL sur l'expression indexée, et
    /// `article_count` n'est calculé que pour les têtes.
    #[tracing::instrument(name = "db.list_legal_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn list_legal_texts(&self, head_only: bool) -> Result<Vec<LegalTextCatalogRow>> {
        let head_filter = if head_only {
            format!("AND {NORM_FOND_EXPR} = 'codes'")
        } else {
            String::new()
        };
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let sql = format!(
            "
                SELECT t.text_uid, t.slug, t.title, t.nature, t.jurisdiction,
                       (SELECT count(DISTINCT a.num_key)
                        FROM legal_article a
                        WHERE a.text_uid = t.text_uid
                          AND {valid_a} AND {norm_a}) AS article_count
                FROM legal_text t
                WHERE {NORM_SCOPE}
                  -- Catalogue = textes *navigables comme un code* (ADR 0133) : codes
                  -- (`code*`/`CODE`), constitutions, lois/ordonnances. On écarte les
                  -- actes unitaires (arrêtés, décrets) et les conventions collectives
                  -- (`TI`/`IDCC`) : cités, mais pas parcourus comme un corpus.
                  AND (t.nature ILIKE 'code%'
                       OR upper(t.nature) IN ('CONSTITUTION', 'LOI_CONSTIT', 'LOI',
                           'LOI_ORGANIQUE', 'ORDONNANCE', 'DECRET_LOI', 'REGLEMENT',
                           'ETAT_CIVIL'))
                  {head_filter}
                  -- … et seulement ceux qui ont AU MOINS un article en vigueur : on
                  -- écarte les versions abrogées/transférées (toutes fenêtres closes
                  -- → 0 en vigueur) et les coquilles sans articles. Elles
                  -- doublonnent le code vivant avec un « 0 article » non navigable.
                  AND EXISTS (SELECT 1 FROM legal_article a
                              WHERE a.text_uid = t.text_uid
                                AND {valid_a} AND {norm_a})
                ORDER BY t.title
                "
        );
        let rows = self.conn.query(&sql, &[]).await?;
        Ok(rows
            .iter()
            .map(|r| LegalTextCatalogRow {
                text_uid: r.get(0),
                slug: r.get(1),
                title: r.get(2),
                nature: r.get(3),
                jurisdiction: r.get(4),
                article_count: r.get(5),
            })
            .collect())
    }

    /// Table des matières d'un code : la version en vigueur de chaque article (une
    /// ligne par `num_key`), dans l'ordre de lecture persisté (`reading_rank` —
    /// `position` de la source quand elle existe, sinon divisions par clé médiane
    /// et tri naturel des `num_key`). Léger, sans corps. Champs bruts (mapping
    /// `TocEntry` côté `lj-api`).
    #[tracing::instrument(name = "db.code_table_of_contents", skip(self), fields(db.system = "postgresql"))]
    pub async fn code_table_of_contents(&self, text_uid: &str) -> Result<Vec<TocArticleRow>> {
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT num, num_key, title_path, status, position FROM (
                  -- Fenêtre du jour préférée, sinon dernière version (ADR 0300 §3) ;
                  -- vraie version avant moignon (0312), consolidée avant publication.
                  SELECT DISTINCT ON (num_key) num, num_key, title_path, status,
                         position, reading_rank, date_debut
                  FROM legal_article a
                  WHERE text_uid = $1
                  ORDER BY num_key, ({valid_a}) DESC, ({norm_a}) DESC,
                           (source = 'jorf') ASC, date_debut DESC NULLS LAST
                ) v
                ORDER BY reading_rank NULLS LAST, num_key
                "
                ),
                &[&text_uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| TocArticleRow {
                num: r.get(0),
                num_key: r.get(1),
                title_path: r.get(2),
                status: r.get(3),
                position: r.get(4),
            })
            .collect())
    }

    /// Vue-lecture à plat d'un texte sans structure ingérée (BOFiP, circulaires…) :
    /// la version en vigueur de chaque article avec son corps, dans le même ordre
    /// de lecture que [`Self::code_table_of_contents`]. Champs au format
    /// [`TocReadingRow`] (que des articles, profondeur 1).
    #[tracing::instrument(name = "db.flat_text_reading", skip(self), fields(db.system = "postgresql"))]
    pub async fn flat_text_reading(&self, text_uid: &str) -> Result<Vec<TocReadingRow>> {
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT num, num_key, status, texte, nota FROM (
                  -- Fenêtre du jour préférée, sinon dernière version (ADR 0300 §3) ;
                  -- vraie version avant moignon (0312), consolidée avant publication.
                  SELECT DISTINCT ON (num_key) num, num_key, status,
                         texte, nota, reading_rank, date_debut
                  FROM legal_article a
                  WHERE text_uid = $1
                  ORDER BY num_key, ({valid_a}) DESC, ({norm_a}) DESC,
                           (source = 'jorf') ASC, date_debut DESC NULLS LAST
                ) v
                ORDER BY reading_rank NULLS LAST, num_key
                "
                ),
                &[&text_uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| TocReadingRow {
                depth: 1,
                child_kind: "article".to_string(),
                child_cid: None,
                child_num_key: Some(r.get(1)),
                label: r.get(0),
                etat: r.get(2),
                texte: r.get(3),
                nota: r.get(4),
            })
            .collect())
    }

    /// Articles voisins de `num_key` pour le contexte de lecture, adaptatif : texte
    /// court ⇒ tous les articles ; sinon la division enclosante ; division trop
    /// grosse ⇒ fenêtre de `±CONTEXT_WINDOW`. Sans corps. Version servie = celle à
    /// `date` (ou `VIGUEUR`), dédoublonnée par `num_key`, dans l'ordre de lecture
    /// persisté (`reading_rank`).
    #[tracing::instrument(name = "db.article_context", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_context(
        &self,
        text_uid: &str,
        num_key: &str,
        date: Option<NaiveDate>,
    ) -> Result<Vec<ArticleNeighborRow>> {
        // Deux stratégies selon la vue. Vue courante : parcours bornés — les divisions
        // VIGUEUR sont contiguës en rang. Vue à date : agrégation globale — la
        // division est celle du `title_path` des versions d'époque, dont les membres
        // abrogés ou transférés depuis sont dispersés dans l'ordre du jour.
        match date {
            Some(d) => self.article_context_at(text_uid, num_key, d).await,
            None => self.article_context_current(text_uid, num_key).await,
        }
    }

    /// Contexte de la vue courante par parcours bornés. L'effectif se compte à
    /// concurrence de `CONTEXT_FULL_MAX + 1`, puis division et fenêtre se lisent en
    /// balayant l'index `(text_uid, reading_rank)` depuis le rang du courant,
    /// plafonné à `CONTEXT_DIVISION_MAX` de chaque côté — une division est une
    /// plage contiguë de rangs, le balayage s'arrête au premier changement de
    /// `title_path`. Les articles au rang encore NULL sont invisibles des balayages.
    async fn article_context_current(
        &self,
        text_uid: &str,
        num_key: &str,
    ) -> Result<Vec<ArticleNeighborRow>> {
        let probe = CONTEXT_FULL_MAX as i64 + 1;
        let (valid_a, norm_a, pick_a) = (
            article_valid_now("a"),
            article_is_norm("a"),
            article_pick_order("a"),
        );
        let cur = self
            .conn
            .query_opt(
                &format!(
                    "
                SELECT c.num, c.num_key, c.status, c.reading_rank, c.title_path,
                       (SELECT count(*) FROM (
                          SELECT DISTINCT num_key FROM legal_article a
                          WHERE text_uid = $1 AND {valid_a} AND {norm_a} LIMIT $3
                        ) s) AS n
                FROM (
                  SELECT num, num_key, status, reading_rank, title_path
                  FROM legal_article a
                  WHERE text_uid = $1 AND num_key = $2 AND {valid_a} AND {norm_a}
                  ORDER BY {pick_a} LIMIT 1
                ) c
                "
                ),
                &[&text_uid, &num_key, &probe],
            )
            .await?;
        let Some(cur) = cur else {
            return Ok(Vec::new());
        };

        if cur.get::<_, i64>(5) <= CONTEXT_FULL_MAX as i64 {
            let rows = self
                .conn
                .query(
                    &format!(
                        "
                    SELECT num, num_key, status FROM (
                      SELECT DISTINCT ON (num_key) num, num_key, status, reading_rank
                      FROM legal_article a
                      WHERE text_uid = $1 AND {valid_a} AND {norm_a}
                      ORDER BY num_key, {pick_a}
                    ) s
                    ORDER BY reading_rank NULLS LAST, num_key
                    "
                    ),
                    &[&text_uid],
                )
                .await?;
            return Ok(rows
                .iter()
                .map(|r| {
                    let nk: String = r.get(1);
                    ArticleNeighborRow {
                        num: r.get(0),
                        current: nk == num_key,
                        num_key: nk,
                        status: r.get(2),
                    }
                })
                .collect());
        }

        let current_row = ArticleNeighborRow {
            num: cur.get(0),
            num_key: cur.get(1),
            status: cur.get(2),
            current: true,
        };
        let Some(cur_rank) = cur.get::<_, Option<i32>>(3) else {
            // Rang en cours d'invalidation : impossible de situer l'article dans l'ordre,
            // il s'affiche seul jusqu'au passage du finisseur.
            return Ok(vec![current_row]);
        };
        let cur_title: Option<String> = cur.get(4);

        let cap = CONTEXT_DIVISION_MAX as i64;
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT num, num_key, status, title_path, reading_rank FROM (
                  (SELECT DISTINCT ON (reading_rank) num, num_key, status, title_path, reading_rank
                   FROM legal_article a
                   WHERE text_uid = $1 AND {valid_a} AND {norm_a} AND reading_rank < $2
                   ORDER BY reading_rank DESC, {pick_a} LIMIT $3)
                  UNION ALL
                  (SELECT DISTINCT ON (reading_rank) num, num_key, status, title_path, reading_rank
                   FROM legal_article a
                   WHERE text_uid = $1 AND {valid_a} AND {norm_a} AND reading_rank > $2
                   ORDER BY reading_rank ASC, {pick_a} LIMIT $3)
                ) s
                "
                ),
                &[&text_uid, &cur_rank, &cap],
            )
            .await?;

        struct Neighbor {
            num: String,
            num_key: String,
            status: String,
            title_path: Option<String>,
            rank: i32,
        }
        let (mut before, mut after): (Vec<Neighbor>, Vec<Neighbor>) = (Vec::new(), Vec::new());
        for r in &rows {
            let x = Neighbor {
                num: r.get(0),
                num_key: r.get(1),
                status: r.get(2),
                title_path: r.get(3),
                rank: r.get(4),
            };
            if x.rank < cur_rank {
                before.push(x);
            } else {
                after.push(x);
            }
        }
        // Du plus proche au plus lointain, de chaque côté.
        before.sort_by_key(|x| std::cmp::Reverse(x.rank));
        after.sort_by_key(|x| x.rank);

        let run = |side: &[Neighbor]| {
            side.iter()
                .take_while(|x| x.title_path == cur_title)
                .count()
        };
        let (b_run, a_run) = (run(&before), run(&after));
        let (nb, na) = if cur_title.is_some() && b_run + 1 + a_run <= CONTEXT_DIVISION_MAX {
            (b_run, a_run)
        } else {
            (CONTEXT_WINDOW, CONTEXT_WINDOW)
        };

        let neighbor = |x: &Neighbor| ArticleNeighborRow {
            num: x.num.clone(),
            num_key: x.num_key.clone(),
            status: x.status.clone(),
            current: false,
        };
        let mut out = Vec::with_capacity(nb + 1 + na);
        out.extend(before.iter().take(nb).rev().map(neighbor));
        out.push(current_row);
        out.extend(after.iter().take(na).map(neighbor));
        Ok(out)
    }

    /// Contexte de la vue à date par agrégation globale : la division est
    /// l'ensemble des versions d'époque partageant le `title_path` du courant, où
    /// qu'elles soient dans l'ordre du jour. `row_number()` re-densifie l'ordre sur
    /// l'ensemble filtré ; `NULLS LAST` place les articles absents de l'arbre du
    /// jour en fin de texte.
    async fn article_context_at(
        &self,
        text_uid: &str,
        num_key: &str,
        date: NaiveDate,
    ) -> Result<Vec<ArticleNeighborRow>> {
        let (full, division, window) = (
            CONTEXT_FULL_MAX as i64,
            CONTEXT_DIVISION_MAX as i64,
            CONTEXT_WINDOW as i64,
        );
        let rows = self
            .conn
            .query(
                "
                WITH cur AS MATERIALIZED (
                  SELECT title_path FROM legal_article
                  WHERE text_uid = $1 AND num_key = $2
                    AND date_debut <= $6 AND (date_fin IS NULL OR date_fin >= $6)
                  ORDER BY date_debut DESC LIMIT 1
                ), v AS (
                  SELECT DISTINCT ON (num_key) num, num_key, status, reading_rank,
                         (title_path IS NOT DISTINCT FROM (SELECT title_path FROM cur))
                           AS same_division,
                         date_debut
                  FROM legal_article
                  WHERE text_uid = $1
                    AND date_debut <= $6 AND (date_fin IS NULL OR date_fin >= $6)
                  ORDER BY num_key, date_debut DESC
                ), r AS (
                  SELECT num, num_key, status, same_division,
                         row_number() OVER (ORDER BY reading_rank NULLS LAST, num_key) AS rn,
                         count(*) OVER () AS n,
                         count(*) FILTER (WHERE same_division) OVER () AS division_size
                  FROM v
                ), c AS (
                  SELECT rn FROM r WHERE num_key = $2
                )
                SELECT num, num_key, status, rn = (SELECT rn FROM c) AS current
                FROM r
                WHERE (SELECT rn FROM c) IS NOT NULL
                  AND CASE
                    WHEN n <= $3 THEN true
                    WHEN (SELECT title_path FROM cur) IS NOT NULL AND division_size <= $4
                      THEN same_division
                    ELSE rn BETWEEN (SELECT rn FROM c) - $5 AND (SELECT rn FROM c) + $5
                  END
                ORDER BY rn
                ",
                &[&text_uid, &num_key, &full, &division, &window, &date],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| ArticleNeighborRow {
                num: r.get(0),
                num_key: r.get(1),
                status: r.get(2),
                current: r.get(3),
            })
            .collect())
    }
}

/// Entrée du calcul de rang de lecture : la version canonique d'un article,
/// réduite à ce dont l'ordre dépend.
struct ReadingOrderInput {
    num_key: String,
    title_path: Option<String>,
    position: Option<i32>,
}

/// Constantes du ranking de la recherche d'articles : boost du titre
/// conjonctif, poids du filet titre OR, constante RRF et profondeur de jambe
/// avant fusion.
const ARTICLE_TITLE_CONJ_BOOST: &str = "4";
const ARTICLE_TITLE_OR_BOOST: &str = "0.25";
/// Boost de la clause `num` ([`article_num_clause`]), multiplicatif sur le BM25
/// de la jambe articles.
const ARTICLE_NUM_BOOST: &str = "2";
/// Poids RRF de la jambe usage_terms : sacs de contextes de citation, votes
/// sommés avec les autres jambes (elle recouvre la jambe articles).
const ARTICLE_USAGE_WEIGHT: &str = "0.8";
/// Poids RRF de la jambe usage scopée au code nommé : grammes du concept seul
/// contre les sacs du code visé.
const ARTICLE_NAMED_USAGE_WEIGHT: &str = "4.0";
const ARTICLE_RRF_K: &str = "60";
const ARTICLE_LEG_LIMIT: i64 = 200;
/// « En vigueur » = la fenêtre de validité, seule définition du référentiel :
/// le littéral `status = 'VIGUEUR'` laisse dehors les articles que DILA ne
/// stampe pas, dont tout le fonds JORF. Mêmes bornes que la lecture à date
/// (`law_article_at_date`) : début inconnu `0001-01-01`, fin ouverte `NULL`.
/// `date_debut`/`date_fin` étant des fast fields, le prédicat se pousse entier
/// dans le scan Tantivy.
pub(crate) fn article_valid_now(a: &str) -> String {
    article_valid_at(a, "CURRENT_DATE")
}
/// La même fenêtre à une date arbitraire : `d` est une expression SQL de date
/// (`CURRENT_DATE` ou littéral `DATE '2020-06-15'` rendu depuis un `NaiveDate`
/// — le type ne sait produire que `%Y-%m-%d`, la splice est sûre).
pub(crate) fn article_valid_at(a: &str, d: &str) -> String {
    format!("{a}.date_debut <= {d} AND ({a}.date_fin IS NULL OR {a}.date_fin >= {d})")
}
/// Rend l'expression SQL de la date de consultation : `CURRENT_DATE` en
/// l'absence de date demandée, sinon le littéral ISO du `NaiveDate`.
fn sql_date_expr(at: Option<NaiveDate>) -> String {
    match at {
        Some(d) => format!("DATE '{d}'"),
        None => "CURRENT_DATE".to_string(),
    }
}
/// Seule définition de `legal_article.kind`, posée par
/// [`Repository::refresh_article_denorm`] et lue par la recherche via
/// [`article_is_norm`].
///
/// Un dispositif modificatif est consommé : son effet vit dans sa cible, et
/// LEGI ne garde de la ligne qu'une mention générée de ce qu'elle a modifié.
/// C'est le corps qui le dit, et lui seul.
const ARTICLE_KIND_EXPR: &str = "CASE WHEN a.texte ~ \
     '^[Aa] (modifié|abrogé|créé|transféré|dénoncé|annulé|remplacé|périmé) les dispositions suivantes' \
     THEN 'edit' ELSE 'article' END";
/// La recherche ne sert que des normes : un edit est lisible sur la fiche de
/// son texte, avec son arête, mais n'est pas un document qu'on cherche.
/// Égalité sur deux valeurs, `kind` étant un littéral de
/// `legal_article_bm25` : poussée en `term`.
pub(crate) fn article_is_norm(a: &str) -> String {
    format!("{a}.kind = 'article'")
}
/// La même fenêtre qu'[`article_valid_at`], en `searchqueryinput` pour le
/// chemin `paradedb.aggregate`. `d` = expression SQL de date
/// ([`sql_date_expr`]). Deux clauses à splicer dans un `must` : `range` sur
/// `date_debut`, et la disjonction `date_fin IS NULL OR date_fin >= $d` en
/// `should` d'un `must_not exists` et d'un `range`. Le `must_not` est adossé à
/// `all()` : un boolean Tantivy sans clause positive ne matche rien.
fn article_valid_agg(d: &str) -> String {
    format!(
        "paradedb.range('date_debut', daterange(NULL, {d}, '(]')), \
         paradedb.boolean(should => ARRAY[\
           paradedb.boolean(must => ARRAY[paradedb.all()], \
                            must_not => ARRAY[paradedb.exists('date_fin')]), \
           paradedb.range('date_fin', daterange({d}, NULL, '[)'))])"
    )
}
/// Départage des lignes d'un même article, après le score dans les jambes de
/// recherche : la version consolidée d'abord, la publication au JO
/// (`source='jorf'`) ensuite, puis la plus récente. Le score prime parce que
/// les deux fonds sont complémentaires — d'un article modificatif, LEGI ne
/// garde qu'un moignon et le lien vers sa cible, le JO garde le texte.
fn article_pick_order(a: &str) -> String {
    format!("({a}.source = 'jorf'), {a}.date_debut DESC")
}
/// Agrégation Tantivy des facettes articles (`paradedb.aggregate`, une passe) :
/// un `terms` par axe + le total en `value_count` sur `id` (les `terms`
/// ignorent les docs sans valeur, pas le total). L'axe `slug` ne demande que son
/// top-100, tampon de fusion pour le top-[`ARTICLE_CODE_FACET_LIMIT`] servi.
/// Les trois autres axes sont énumérés en entier et restent exacts, contrôlés
/// par `sum_other_doc_count` au parsing.
const ARTICLE_FACETS_AGG: &str = r#"{"slug":{"terms":{"field":"slug","size":100,"segment_size":5000}},"jurisdiction":{"terms":{"field":"jurisdiction","size":500}},"nature":{"terms":{"field":"nature","size":500}},"source":{"terms":{"field":"source","size":500}},"total":{"value_count":{"field":"id"}}}"#;
const ARTICLE_FACETS_BUCKET_LIMIT: i64 = 1_000_000;
/// Pendant de [`ARTICLE_FACETS_AGG`] pour le membre textes à corps, sur les
/// champs fast de `legal_text_body_bm25`. Les axes `nature` et `source` y
/// lisent deux champs distincts (`upper`/`lower` de la même colonne) là où
/// `legal_article` porte deux colonnes dénormalisées.
const TEXT_FACETS_AGG: &str = r#"{"slug":{"terms":{"field":"slug","size":100,"segment_size":5000}},"jurisdiction":{"terms":{"field":"jurisdiction","size":500}},"nature":{"terms":{"field":"nature_up","size":500}},"source":{"terms":{"field":"source","size":500}},"total":{"value_count":{"field":"id"}}}"#;
/// Plafond de l'axe `code` retourné par les stats (top par count).
const ARTICLE_CODE_FACET_LIMIT: usize = 20;
/// Poids RRF de la jambe articles étrangère.
const ARTICLE_FOREIGN_WEIGHT: &str = "0.25";

/// Natures « navigables comme un code » — miroir du filtre du catalogue
/// `/api/codes`, alias `t` requis. Borne la jambe conteneurs.
const NAVIGABLE_TEXT_NATURES_SQL: &str = "(t.nature ILIKE 'code%' \
     OR upper(t.nature) IN ('CONSTITUTION', 'LOI_CONSTIT', 'LOI', \
         'LOI_ORGANIQUE', 'ORDONNANCE', 'DECRET_LOI', 'REGLEMENT', \
         'ETAT_CIVIL'))";

/// Visibilité par défaut, alias `t` requis : la recherche ne sert pas les
/// parutions sans objet consultable — actes individuels (nominations…),
/// véhicules de publication, lois d'habilitation. Elles restent résolvables par
/// uid.
const TEXT_ROLE_VISIBLE_SQL: &str = "t.role NOT IN ('individuel', 'vehicule', 'habilitation')";

/// Miroir Tantivy de [`TEXT_ROLE_VISIBLE_SQL`] : les clauses `must_not` d'un
/// `searchqueryinput`, sur le champ fast `role`. Même liste de rôles — les deux
/// formes se lisent ensemble.
const TEXT_ROLE_HIDDEN_TERMS: &str = "paradedb.term('role', 'individuel'), \
     paradedb.term('role', 'vehicule'), paradedb.term('role', 'habilitation')";

/// Juridictions primaires d'une requête : le droit applicable en France
/// (FR/UE/INTL) + les pays que la requête nomme
/// ([`lj_core::jurisdictions::query_jurisdictions`]).
fn primary_jurisdictions(query: &str) -> Vec<String> {
    let mut primary: Vec<String> = ["FR", "UE", "INTL"].map(String::from).to_vec();
    primary.extend(
        lj_core::jurisdictions::query_jurisdictions(query)
            .into_iter()
            .map(String::from),
    );
    primary
}

/// Prédicats BM25 des deux jambes de la recherche d'articles et de ses
/// facettes. Par jambe (`legal_article_bm25` / `legal_text_body_bm25`),
/// `should` de trois clauses : titre conjonctif normalisé boosté (par
/// alternative requête/expansion, chacune devant matcher le titre en entier),
/// filet titre OR à faible poids ($1), corps ($2). Pousse les alternatives
/// conjonctives dans `params` ($3..) ; requête réduite à rien → filet OR seul.
/// Les deux jambes renvoient l'expression `searchqueryinput` nue (sans
/// `id @@@`).
///
/// Qui n'en veut qu'une prend [`article_text_predicate`] : la jambe articles
/// pousse les paramètres de sa clause `num`, qu'une requête textes ne
/// référencerait pas — et un placeholder poussé sans être référencé fait
/// échouer la requête entière.
fn article_search_predicates(
    params: &mut ArticleSearchParams,
    query: &str,
    expansions: &[String],
) -> (String, String) {
    let alt_phs = article_conj_alt_phs(params, query, expansions);
    let art = format!(
        "paradedb.boolean(should => ARRAY[{}, paradedb.match('texte', $2){}])",
        article_title_clauses("search_title", &alt_phs),
        article_num_clause(params, query),
    );
    (art, article_text_clause(&alt_phs))
}

/// Jambe textes seule — la facette textes, qui tourne dans sa propre requête
/// avec son propre jeu de paramètres.
fn article_text_predicate(
    params: &mut ArticleSearchParams,
    query: &str,
    expansions: &[String],
) -> String {
    article_text_clause(&article_conj_alt_phs(params, query, expansions))
}

/// Alternatives conjonctives poussées dans `params` ($3..), communes aux deux
/// jambes.
fn article_conj_alt_phs(
    params: &mut ArticleSearchParams,
    query: &str,
    expansions: &[String],
) -> Vec<usize> {
    let mut conj_alts: Vec<String> = std::iter::once(query.to_string())
        .chain(expansions.iter().cloned())
        .filter_map(|q| lj_core::aliases::conj_title_query(&q))
        .collect();
    conj_alts.dedup();
    conj_alts
        .into_iter()
        .map(|a| params.push(Box::new(a)))
        .collect()
}

/// Clauses titre d'une jambe : conjonctif normalisé boosté par alternative,
/// plus le filet titre OR à faible poids ($1). Requête réduite à rien
/// (« article ») → filet OR seul.
fn article_title_clauses(field: &str, alt_phs: &[usize]) -> String {
    let or = format!("paradedb.boost({ARTICLE_TITLE_OR_BOOST}, paradedb.match('{field}', $1))");
    if alt_phs.is_empty() {
        return or;
    }
    let alts: Vec<String> = alt_phs
        .iter()
        .map(|ph| format!("paradedb.match('{field}', ${ph}, conjunction_mode => true)"))
        .collect();
    let conj = match alts.as_slice() {
        [single] => single.clone(),
        many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
    };
    format!("paradedb.boost({ARTICLE_TITLE_CONJ_BOOST}, {conj}), {or}")
}

/// Jambe textes à corps : la requête nue (sans `t.id @@@`, sans les clauses SQL
/// de corps et de visibilité). Les hits la posent en prédicat SQL, les facettes
/// la passent telle quelle à `paradedb.aggregate`.
fn article_text_clause(alt_phs: &[usize]) -> String {
    format!(
        "paradedb.boolean(should => ARRAY[{}, paradedb.match('body', $2)])",
        article_title_clauses("title", alt_phs),
    )
}

/// Clause `num` de la jambe articles — le numéro d'article cité dans la
/// requête, cherché sur le champ qui le porte, rendue prête à concaténer dans
/// un `should` (`, clause` ; chaîne vide si la requête ne cite aucun numéro).
///
/// Deux formes par numéro, parce que le corpus en porte deux : collés
/// (`L423-7`, un seul token) et pointés ou étoilés (`D. 423-7`, deux tokens) —
/// d'où la forme conjonctive rendue par [`lj_core::article_key::display`], qui
/// les cherche sans les confondre (`L. 423-7` n'atteint pas `R*423-7`).
fn article_num_clause(params: &mut ArticleSearchParams, query: &str) -> String {
    let mut alts: Vec<String> = Vec::new();
    for key in lj_core::article_key::query_article_keys(query) {
        let dotted = lj_core::article_key::display(&key);
        let forms = if dotted == key {
            vec![key]
        } else {
            vec![key.clone(), dotted]
        };
        for form in forms {
            let ph = params.push(Box::new(form));
            alts.push(format!(
                "paradedb.match('num', ${ph}, conjunction_mode => true)"
            ));
        }
    }
    if alts.is_empty() {
        return String::new();
    }
    format!(
        ", paradedb.boost({ARTICLE_NUM_BOOST}, paradedb.boolean(should => ARRAY[{}]))",
        alts.join(", ")
    )
}

/// Construit la jambe titre : requête enrichie des expansions d'alias (OR
/// implicite du `match`).
fn article_title_query(query: &str, expansions: &[String]) -> String {
    if expansions.is_empty() {
        query.to_string()
    } else {
        format!("{query} {}", expansions.join(" "))
    }
}

/// Accumulateur de paramètres SQL pour la recherche d'articles et ses facettes.
/// `$1`/`$2` sont toujours la jambe titre et la jambe corps ; les filtres
/// optionnels (`text_uid`/`jurisdiction`/`nature`/`source`) puis les bornes de
/// pagination sont poussés à la suite.
struct ArticleSearchParams {
    params: Vec<Box<dyn ToSql + Sync + Send>>,
}

impl ArticleSearchParams {
    fn new(title_query: String, body_query: &str) -> Self {
        Self {
            params: vec![Box::new(title_query), Box::new(body_query.to_string())],
        }
    }

    /// Sans jambe titre ni jambe corps — pour une requête qui ne référence pas
    /// `$1`/`$2` (Postgres refuse un placeholder jamais référencé).
    fn empty() -> Self {
        Self { params: Vec::new() }
    }

    /// Pousse une valeur et renvoie son numéro de placeholder `$N` (1-indexé).
    fn push(&mut self, value: Box<dyn ToSql + Sync + Send>) -> usize {
        self.params.push(value);
        self.params.len()
    }

    /// Pousse les filtres présents et renvoie les DEUX clauses SQL `AND …`
    /// (chaînes vides si aucun filtre) : une pour la jambe articles, une pour la
    /// jambe textes à corps — mêmes placeholders, colonnes propres à chaque jambe.
    /// `a.nature` est stockée en upper ; `source` vaut `a.source` côté articles et
    /// `lower(t.nature)` côté textes à corps. `nature_set` filtre les deux jambes :
    /// `(liste, true)` = nature dans la liste, `(liste, false)` = hors liste.
    fn push_filters(
        &mut self,
        text_uid: Option<&str>,
        jurisdiction: Option<&str>,
        nature: Option<&str>,
        source: Option<&str>,
        nature_set: Option<(&[String], bool)>,
    ) -> (String, String) {
        let mut art = String::new();
        let mut txt = String::new();
        for (art_col, txt_col, value) in [
            ("a.text_uid", "t.text_uid", text_uid),
            ("a.jurisdiction", "t.jurisdiction", jurisdiction),
            (
                "a.nature",
                "upper(t.nature)",
                nature.map(str::to_uppercase).as_deref(),
            ),
            ("a.source", "lower(t.nature)", source),
        ] {
            if let Some(v) = value {
                let ph = self.push(Box::new(v.to_string()));
                art.push_str(&format!("\n  AND {art_col} = ${ph}"));
                txt.push_str(&format!("\n  AND {txt_col} = ${ph}"));
            }
        }
        if let Some((natures, include)) = nature_set.filter(|(n, _)| !n.is_empty()) {
            let ph = self.push(Box::new(natures.to_vec()));
            let op = if include { "= ANY" } else { "<> ALL" };
            art.push_str(&format!("\n  AND a.nature {op}(${ph})"));
            txt.push_str(&format!("\n  AND upper(t.nature) {op}(${ph})"));
        }
        (art, txt)
    }

    fn refs(&self) -> Vec<&(dyn ToSql + Sync)> {
        // `Send` sur les boxes garde le future de la méthode `Send` (frontière axum
        // Handler) ; on retombe ici sur `&(dyn ToSql + Sync)` attendu par
        // tokio-postgres.
        self.params
            .iter()
            .map(|p| &**p as &(dyn ToSql + Sync))
            .collect()
    }
}

/// Les six statements de classification des rôles : reset à `instrument` puis
/// cinq passes par signaux sûrs — fonction pure, dumpable pour validation psql.
/// `scoped` borne chaque statement aux `text_uid` d'un lot (`$1`).
///
/// Passes : motifs de titre pour `individuel` et `habilitation` ; pour
/// `vehicule` deux signaux, un décret « portant publication » vide porteur
/// d'une arête `modifie` sortante résolue, et la cible d'une arête `publie`
/// entrante possédée par une fiche instrument. Une loi constitutionnelle est
/// `modificatif` de l'instrument-Constitution.
fn text_role_sqls(scoped: bool) -> [String; 6] {
    // Deux fragments selon l'alias de la passe (`legal_text` nu ou `t`).
    let scope = if scoped {
        " AND text_uid = ANY($1)"
    } else {
        ""
    };
    let scope_t = if scoped {
        " AND t.text_uid = ANY($1)"
    } else {
        ""
    };
    [
        format!(
            "UPDATE legal_text SET role = 'instrument' WHERE role <> 'instrument'{scope}"
        ),
        format!(
            "
            UPDATE legal_text SET role = 'individuel'
            WHERE text_uid LIKE 'JORFTEXT%'{scope}
              AND (title ~* 'portant (nomination|promotion|titularisation|naturalisation|cessation de fonctions|admission à la retraite|acceptation de la démission|détachement|radiation des cadres|radiation \\()'
                   OR title ~* '^avis de vacance'
                   OR title ~* 'accordant la nationalité française|conférant l.honorariat|acceptant la démission|inscription au tableau d.avancement')
            "
        ),
        format!(
            "
            UPDATE legal_text SET role = 'habilitation'
            WHERE text_uid LIKE 'JORFTEXT%'{scope}
              AND title ~* 'autorisant (la ratification|l.approbation|l.adhésion|l.accession|le Président de la République à (ratifier|approuver|adhérer))'
              AND title !~* ' et (portant|modifiant)'
            "
        ),
        format!(
            "
            UPDATE legal_text t SET role = 'vehicule'
            WHERE t.text_uid LIKE 'JORFTEXT%'{scope_t}
              AND t.title ~* 'portant publication'
              AND t.body IS NULL
              AND NOT EXISTS (
                    SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)
              AND EXISTS (
                    SELECT 1 FROM legal_link l
                    WHERE l.owner_text_uid = t.text_uid
                      AND l.verb = 'modifie'
                      AND l.direction = 'outgoing'
                      AND l.target_text_uid IS NOT NULL
                      AND l.target_text_uid <> t.text_uid)
            "
        ),
        format!(
            "
            UPDATE legal_text t SET role = 'vehicule'
            WHERE t.role <> 'vehicule'{scope_t}
              AND EXISTS (
                    SELECT 1 FROM legal_link l
                    WHERE l.verb = 'publie'
                      AND l.direction = 'incoming'
                      AND l.target_text_uid = t.text_uid)
            "
        ),
        format!(
            "UPDATE legal_text SET role = 'modificatif' WHERE nature = 'LOI_CONSTIT'{scope}"
        ),
    ]
}

/// Texte court : on affiche tous les articles (DDHC, courtes lois/traités).
const CONTEXT_FULL_MAX: usize = 40;
/// Division enclosante affichée entière tant qu'elle ne dépasse pas ce seuil.
const CONTEXT_DIVISION_MAX: usize = 80;
/// Demi-fenêtre de repli (par position) quand la division est trop grosse.
const CONTEXT_WINDOW: usize = 6;

#[cfg(test)]
mod tests {
    use super::text_role_sqls;

    #[test]
    fn text_role_sqls_cover_passes_and_scope() {
        let full = text_role_sqls(false);
        assert!(full.iter().all(|s| !s.contains("ANY($1)")));
        assert!(full[0].contains("role = 'instrument'"));
        assert!(full[1].contains("'individuel'"));
        assert!(full[2].contains("'habilitation'"));
        assert!(full[3].contains("'vehicule'") && full[3].contains("'modifie'"));
        assert!(full[4].contains("'vehicule'") && full[4].contains("'publie'"));
        assert!(full[5].contains("'modificatif'"));
        let scoped = text_role_sqls(true);
        assert!(scoped.iter().all(|s| s.contains("ANY($1)")));
    }
}

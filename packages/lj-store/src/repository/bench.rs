//! Lectures/écritures hors chemin d'upsert : format de source, résolution
//! `public_id`↔`id`, corps/texte pour les bancs offline, backfills de reprise
//! (re-extract, summary), génération et service des sitemaps.

use super::support::now;
use super::types::{GtDoc, MissingSummaryRow, SitemapRow};
use super::DecisionRepository;
use crate::error::Result;
use chrono::{DateTime, NaiveDate, Utc};
use lj_core::{CERTIFIER_VERSION, EXTRACT_VERSION};
use serde_json::Value;
use tokio_postgres::types::Json;

/// Sort d'une écriture de révision manuelle par [`DecisionRepository::upsert_fields_at_version`].
///
/// `Unchanged` est distinct de `NotFound` : la décision existe et porte déjà
/// exactement cette révision, et l'appelant doit poursuivre son traitement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualUpsert {
    /// Au moins un champ différait : la ligne est réécrite, `updated_at` bumpé.
    Applied,
    /// Révision identique à celle en base : rien écrit, `updated_at` intact.
    Unchanged,
    /// Aucune décision pour ce `source_uid`, ou sa version l'emporte.
    NotFound,
}

/// Champs d'extraction d'une révision chargée manuellement — mêmes colonnes de
/// `decisions` que l'extraction déterministe, seule la version diffère. Elle
/// couvre TOUT ce qui est extrait — champs, facettes, citations — d'un bloc. Le
/// parsing du format d'annotation vit chez l'appelant ; ici n'arrive que la
/// projection typée.
#[derive(Debug, Clone, Default)]
pub struct ManualFields {
    pub date_lecture: Option<NaiveDate>,
    pub date_audience: Option<NaiveDate>,
    pub docket_numbers: Vec<String>,
    pub publication_codes: Vec<String>,
    pub solution_uid: Option<String>,
    pub procedure_uid: Option<String>,
    pub office_uid: Option<String>,
    pub legal_domain_uid: Option<String>,
    /// Column `decisions.seat` (jsonb), from which `chamber_key` is derived at
    /// write time: the gold annotates a room label, and one parse turns it into
    /// both forms.
    pub seat: Option<lj_core::chamber::Seat>,
    pub formation_uid: Option<String>,
    pub jurisdiction_code: Option<String>,
}

impl DecisionRepository<'_> {
    /// Persiste le `payload_format` (`xml`/`json`) d'une décision : c'est lui qui
    /// choisit le reconstructeur côté re-extract (`fetch_reextract_inputs`).
    #[tracing::instrument(name = "db.set_payload_format", skip(self), fields(db.system = "postgresql"))]
    pub async fn set_payload_format(&self, decision_id: i64, payload_format: &str) -> Result<()> {
        self.conn
            .execute(
                "
                INSERT INTO decision_full_text (decision_id, payload_format)
                VALUES ($1, $2)
                ON CONFLICT (decision_id) DO UPDATE SET
                  payload_format = EXCLUDED.payload_format
                ",
                &[&decision_id, &payload_format],
            )
            .await?;
        Ok(())
    }

    /// Résout des `public_id` (base62 exposés par l'API) vers les `id`
    /// canoniques. Les ids inconnus sont simplement absents du résultat.
    #[tracing::instrument(name = "db.resolve_public_ids", skip_all, fields(db.system = "postgresql"))]
    pub async fn resolve_public_ids(&self, public_ids: &[String]) -> Result<Vec<(String, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT public_id, id FROM decisions WHERE public_id = ANY($1)",
                &[&public_ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, i64>(1)))
            .collect())
    }

    /// Reading date per `decision_id`. Rows with a null `date_lecture` are
    /// omitted: the offline ranking bench needs the date to drop hits the
    /// ground truth could not have judged, and no date means no bound.
    pub async fn fetch_reading_dates(&self, ids: &[i64]) -> Result<Vec<(i64, NaiveDate)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, date_lecture FROM decisions \
                 WHERE id = ANY($1) AND date_lecture IS NOT NULL",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, NaiveDate>(1)))
            .collect())
    }

    /// Métadonnées d'identification par `decision_id` : `(id, public_id,
    /// search_title)`. Sens inverse de [`Self::resolve_public_ids`]. Les décisions
    /// sans `public_id` sont omises.
    pub async fn fetch_public_meta(&self, ids: &[i64]) -> Result<Vec<(i64, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, public_id, search_title FROM decisions \
                 WHERE id = ANY($1) AND public_id IS NOT NULL",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.try_get::<_, Option<String>>(2)
                        .unwrap_or_default()
                        .unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Résumé v4 + titre par `decision_id` : `(id, search_title, summary)`.
    /// Décisions sans `summary` omises.
    pub async fn fetch_summaries(&self, ids: &[i64]) -> Result<Vec<(i64, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, COALESCE(search_title, ''), summary FROM decisions \
                 WHERE id = ANY($1) AND summary IS NOT NULL AND summary <> ''",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, String>(2),
                )
            })
            .collect())
    }

    /// Texte intégral par `decision_id` : `(id, public_id, search_title,
    /// full_text)`, lu sur `decisions.full_text` (grain décision). Décisions sans
    /// `public_id`/`full_text` omises.
    pub async fn fetch_bodies(&self, ids: &[i64]) -> Result<Vec<(i64, String, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, d.public_id, COALESCE(d.search_title, ''), d.full_text \
                 FROM decisions d \
                 WHERE d.id = ANY($1) AND d.public_id IS NOT NULL AND d.full_text IS NOT NULL",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, String>(2),
                    r.get::<_, String>(3),
                )
            })
            .collect())
    }

    /// Texte intégral (`decisions.full_text`, grain décision) par id. Décisions sans
    /// `full_text` omises.
    pub async fn fetch_full_texts(&self, ids: &[i64]) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, full_text FROM decisions \
                 WHERE id = ANY($1) AND full_text IS NOT NULL",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Corps + métadonnées grain décision pour la matérialisation GT : `full_text`
    /// indexé + juridiction/niveau/date/titre. Décisions sans `full_text` omises.
    pub async fn fetch_gt_docs(&self, ids: &[i64]) -> Result<Vec<GtDoc>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, d.public_id, COALESCE(j.label, ''), \
                 COALESCE(d.jurisdiction_type, ''), COALESCE(d.date_lecture::text, ''), \
                 COALESCE(d.search_title, ''), d.full_text FROM decisions d \
                 LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code \
                 WHERE d.id = ANY($1) AND d.public_id IS NOT NULL AND d.full_text IS NOT NULL",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| GtDoc {
                id: r.get(0),
                public_id: r.get(1),
                jurisdiction_name: r.get(2),
                jurisdiction_type: r.get(3),
                date_lecture: r.get(4),
                search_title: r.get(5),
                full_text: r.get(6),
            })
            .collect())
    }

    /// Échantillon aléatoire de `full_text` (`TABLESAMPLE SYSTEM`). `sample_pct` est
    /// piloté par le code, jamais une entrée externe.
    pub async fn sample_full_texts(&self, sample_pct: f64) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT full_text FROM decisions TABLESAMPLE SYSTEM ({sample_pct}) \
             WHERE full_text IS NOT NULL"
        );
        let rows = self.conn.query(sql.as_str(), &[]).await?;
        Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
    }

    /// Fréquence documentaire `df` d'un terme indexé `term` dans `full_text`.
    /// `term` est déjà folded/lowercased comme le tokenizer de l'index ;
    /// `paradedb.term` ne le re-tokenise pas.
    pub async fn bm25_term_doc_count(&self, term: &str) -> Result<i64> {
        let row = self
            .conn
            .query_one(
                "SELECT count(*) FROM decisions \
                 WHERE id @@@ paradedb.term('full_text', $1::text)",
                &[&term],
            )
            .await?;
        Ok(row.get::<_, i64>(0))
    }

    #[tracing::instrument(name = "db.max_decision_id", skip(self), fields(db.system = "postgresql"))]
    pub async fn max_decision_id(&self) -> Result<i64> {
        let row = self
            .conn
            .query_one("SELECT COALESCE(MAX(id), 0) FROM decisions", &[])
            .await?;
        Ok(row.get::<_, i64>(0))
    }

    /// Entrées du re-extract : `(payload_format, full_text, source_fields,
    /// source_uid)`, de quoi reconstruire le payload source et le re-parser. Les
    /// trois derniers viennent de la provenance autoritaire (rang max active),
    /// celle dont le texte EST `full_text`. `None` si pas de provenance active ou si
    /// `full_text`/`source_fields` manque.
    /// Décisions dont aucune provenance ne porte de charge : elles ne peuvent
    /// pas être ré-extraites, et rien dans le produit ne le dit.
    #[tracing::instrument(name = "db.decisions_sans_charge", skip(self), fields(db.system = "postgresql"))]
    pub async fn decisions_sans_charge(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.public_id
                FROM decisions d
                WHERE d.deleted_at IS NULL
                  AND NOT EXISTS (
                    SELECT 1 FROM decision_sources ds
                    WHERE ds.decision_id = d.id AND ds.deleted_at IS NULL
                      AND ds.source_rank > 0
                  )
                ORDER BY d.id
                ",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Charge de ré-extraction d'une décision : sa provenance la plus
    /// autoritaire. Le rang 0 dit qu'une ligne ne porte AUCUNE décision (bundle
    /// de commentaires ArianeWeb, analyse ADDE) : elle ne peut pas être lue
    /// comme une charge, et une source de charge oubliée au barème laisse la
    /// décision non ré-extraite, jamais mal extraite.
    pub async fn fetch_reextract_inputs(
        &self,
        decision_id: i64,
    ) -> Result<Option<(String, Value, String)>> {
        let row = self
            .conn
            .query_opt(
                "
                SELECT d.full_text, ds.source_fields, ds.source_uid
                FROM decisions d
                JOIN LATERAL (
                    SELECT source_fields, source_uid
                    FROM decision_sources
                    WHERE decision_id = d.id AND deleted_at IS NULL AND source_rank > 0
                    ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, source_uid ASC
                    LIMIT 1
                ) ds ON true
                WHERE d.id = $1
                ",
                &[&decision_id],
            )
            .await?;
        let Some(row) = row else { return Ok(None) };
        let full_text: Option<String> = row.get(0);
        let source_fields: Option<Value> = row.get(1);
        let source_uid: String = row.get(2);
        match (full_text, source_fields) {
            (Some(ft), Some(sf)) => Ok(Some((ft, sf, source_uid))),
            _ => Ok(None),
        }
    }

    /// Même projection que [`Self::fetch_reextract_inputs`] pour un lot d'ids en une
    /// requête, plus l'`id` en tête pour ré-apparier. Les ids incomplets sont omis ;
    /// l'ordre n'est pas garanti.
    pub async fn fetch_reextract_inputs_batch(
        &self,
        ids: &[i64],
    ) -> Result<Vec<(i64, String, Value, String, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id, d.full_text, ds.source_fields, ds.source_uid, d.case_name
                FROM decisions d
                JOIN LATERAL (
                    SELECT source_fields, source_uid
                    FROM decision_sources
                    WHERE decision_id = d.id AND deleted_at IS NULL AND source_rank > 0
                    ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, source_uid ASC
                    LIMIT 1
                ) ds ON true
                WHERE d.id = ANY($1)
                  AND d.full_text IS NOT NULL
                  AND ds.source_fields IS NOT NULL
                ",
                &[&ids],
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let full_text: Option<String> = r.get(1);
                let source_fields: Option<Value> = r.get(2);
                match (full_text, source_fields) {
                    (Some(ft), Some(sf)) => Some((
                        r.get::<_, i64>(0),
                        ft,
                        sf,
                        r.get::<_, String>(3),
                        r.get::<_, Option<String>>(4),
                    )),
                    _ => None,
                }
            })
            .collect())
    }

    /// Même chose, adressé par `source_uid` : le triplet `(source_uid, full_text,
    /// source_fields)`, matière de
    /// [`lj_core::parsing::Decision::from_source_fields`]. Les uids absents ou
    /// incomplets sont omis.
    pub async fn fetch_reextract_inputs_by_source_uids(
        &self,
        source_uids: &[String],
    ) -> Result<Vec<(String, String, Value)>> {
        let rows = self
            .conn
            .query(
                "
                SELECT ds.source_uid, d.full_text, ds.source_fields
                FROM decision_sources ds
                JOIN decisions d ON d.id = ds.decision_id
                WHERE ds.source_uid = ANY($1)
                  AND ds.deleted_at IS NULL
                  AND d.deleted_at IS NULL
                  AND d.full_text IS NOT NULL
                  AND ds.source_fields IS NOT NULL
                ",
                &[&source_uids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get::<_, String>(0),
                    r.get::<_, String>(1),
                    r.get::<_, Value>(2),
                )
            })
            .collect())
    }

    /// Keyset (par `decision_id`) des décisions multi-provenances tout-judilibre
    /// (≥ 2 `decision_sources` actifs, tous `source='judilibre'`), avec la liste de
    /// leurs provenances `(source_uid, payload_format, source_fields)`. Lignes
    /// triées `(decision_id, ds.id)` : l'appelant regroupe. `last_id` = dernier
    /// `decision_id` traité.
    pub async fn fetch_judilibre_multiprovenance_batch(
        &self,
        last_id: i64,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, Value)>> {
        let rows = self
            .conn
            .query(
                "
                WITH ids AS (
                    SELECT decision_id
                    FROM decision_sources
                    WHERE deleted_at IS NULL AND decision_id > $1
                    GROUP BY decision_id
                    HAVING count(*) > 1
                       AND count(*) = count(*) FILTER (WHERE source = 'judilibre')
                    ORDER BY decision_id
                    LIMIT $2
                )
                SELECT ds.decision_id, ds.source_uid, ds.payload_format, ds.source_fields
                FROM decision_sources ds
                JOIN ids ON ids.decision_id = ds.decision_id
                WHERE ds.deleted_at IS NULL
                ORDER BY ds.decision_id, ds.id
                ",
                &[&last_id, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, String>(2),
                    r.get::<_, Value>(3),
                )
            })
            .collect())
    }

    /// Keyset des décisions ayant au moins un chunk sans embedding, avec les
    /// entrées de reconstruction plus le `public_id` et le `content_checksum`
    /// existants — de quoi rebâtir un `Candidate` fidèle qui re-traverse le pipeline
    /// sans changer l'identité ni le checksum.
    ///
    /// Chaque `decision_id` candidat est toujours renvoyé (l'appelant avance
    /// `last_id` au max du lot), champs de reconstruction en `Option` : `None` quand
    /// la décision n'a pas de provenance active ou pas de `full_text`. Renvoie
    /// `(id, Option<(public_id, payload_format, full_text, source_fields,
    /// source_uid, content_checksum)>)`.
    #[allow(clippy::type_complexity)]
    pub async fn fetch_missing_embedding_batch(
        &self,
        last_id: i64,
        limit: i64,
    ) -> Result<Vec<(i64, Option<(String, String, String, Value, String, String)>)>> {
        let rows = self
            .conn
            .query(
                "
                WITH ids AS (
                    SELECT DISTINCT c.decision_id AS id
                    FROM decision_chunks c
                    WHERE c.embedding IS NULL AND c.decision_id > $1
                    ORDER BY c.decision_id
                    LIMIT $2
                )
                SELECT ids.id, d.public_id, d.full_text,
                       ds.payload_format, ds.source_fields, ds.source_uid, ds.content_checksum
                FROM ids
                LEFT JOIN decisions d ON d.id = ids.id
                LEFT JOIN LATERAL (
                    SELECT payload_format, source_fields, source_uid, content_checksum
                    FROM decision_sources
                    WHERE decision_id = ids.id AND deleted_at IS NULL
                    ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, source_uid ASC
                    LIMIT 1
                ) ds ON true
                ORDER BY ids.id
                ",
                &[&last_id, &limit],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let id: i64 = r.get(0);
            let public_id: Option<String> = r.get(1);
            let full_text: Option<String> = r.get(2);
            let payload_format: Option<String> = r.get(3);
            let source_fields: Option<Value> = r.get(4);
            let source_uid: Option<String> = r.get(5);
            let content_checksum: Option<String> = r.get(6);
            let complete = match (
                public_id,
                full_text,
                payload_format,
                source_fields,
                source_uid,
                content_checksum,
            ) {
                (Some(pid), Some(ft), Some(fmt), Some(sf), Some(uid), Some(ck)) => {
                    Some((pid, fmt, ft, sf, uid, ck))
                }
                _ => None,
            };
            out.push((id, complete));
        }
        Ok(out)
    }

    /// Keyset des décisions à re-parser : `full_text` présent, ``extract_version``
    /// sous la version courante. Les révisions manuelles (version > courante) sont
    /// hors worklist.
    pub async fn decision_ids_for_reextract(&self, last_id: i64, limit: i64) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM decisions d
                WHERE d.id > $1
                  AND d.full_text IS NOT NULL
                  AND (d.extract_version IS NULL OR d.extract_version < $3)
                  AND (d.certified_version IS NULL OR d.certified_version < $4)
                ORDER BY d.id
                LIMIT $2
                ",
                &[&last_id, &limit, &EXTRACT_VERSION, &CERTIFIER_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Certifie un lot de décisions à `certified_version` : le re-extract par défaut
    /// les skippe tant que `certified_version >= CERTIFIER_VERSION`. Setter
    /// uniquement — l'invalidation est un bump de la constante.
    pub async fn certify_decisions(&self, ids: &[i64], version: i32) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let n = self
            .conn
            .execute(
                "UPDATE decisions SET certified_version = $2 WHERE id = ANY($1)",
                &[&ids, &version],
            )
            .await?;
        Ok(n)
    }

    /// Nombre de décisions périmées, même prédicat que
    /// [`Self::decision_ids_for_reextract`] : sert l'avancement `processed/total`.
    pub async fn count_decisions_for_reextract(&self) -> Result<i64> {
        let row = self
            .conn
            .query_one(
                "
                SELECT count(*) FROM decisions
                WHERE full_text IS NOT NULL
                  AND (extract_version IS NULL OR extract_version < $1)
                  AND (certified_version IS NULL OR certified_version < $2)
                ",
                &[&EXTRACT_VERSION, &CERTIFIER_VERSION],
            )
            .await?;
        Ok(row.get(0))
    }

    /// Worklist complète des ids périmés, triée par id, en un seul scan, ensuite
    /// partitionnée entre workers concurrents.
    pub async fn stale_decision_ids_for_reextract(&self) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT id FROM decisions
                WHERE full_text IS NOT NULL
                  AND (extract_version IS NULL OR extract_version < $1)
                  AND (certified_version IS NULL OR certified_version < $2)
                ORDER BY id
                ",
                &[&EXTRACT_VERSION, &CERTIFIER_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Worklist de la passe intégrale (`--full`) : tout le fonds jusqu'à la version
    /// courante incluse, même déjà à jour, même certifié — c'est le relink
    /// hebdomadaire. Les révisions manuelles restent hors worklist.
    pub async fn all_decision_ids_for_reextract(&self) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT id FROM decisions
                WHERE full_text IS NOT NULL
                  AND (extract_version IS NULL OR extract_version <= $1)
                ORDER BY id
                ",
                &[&EXTRACT_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Écrit une révision des champs d'extraction dans les colonnes de `decisions`,
    /// stampée `extract_version = version` ; une version > courante n'est jamais
    /// réécrite par le recognizer. Les champs `None` écrivent NULL (révision
    /// exhaustive : absent = confirmé absent), sauf `jurisdiction_code` (COALESCE :
    /// porté aussi par les métadonnées source).
    ///
    /// N'écrit que sur différence : une révision identique laisse `updated_at`
    /// intact. `cible` vide = `NotFound` (pas de décision, ou version supérieure) ;
    /// `cible` non vide sans ligne écrite = `Unchanged`.
    ///
    /// `seat_paths` is NOT written here: a path resolves against the room
    /// vocabulary of the whole corpus, which only `resolve-seats` holds. So a
    /// gold load leaves the axis to that pass, as the first write of the seats
    /// did.
    pub async fn upsert_fields_at_version(
        &self,
        source_uid: &str,
        version: i16,
        fields: &ManualFields,
    ) -> Result<ManualUpsert> {
        let seat = fields.seat.clone().map(Json);
        let chamber_key = fields.seat.as_ref().and_then(lj_core::chamber::Seat::key);
        let row = self
            .conn
            .query_one(
                "WITH cible AS (
                     SELECT d.id
                     FROM decisions d
                     JOIN decision_sources ds ON ds.decision_id = d.id
                     WHERE ds.source_uid = $1
                       AND (d.extract_version IS NULL OR d.extract_version <= $12)
                 ), maj AS (
                     UPDATE decisions d SET
                         date_lecture = $2, date_audience = $3, docket_numbers = $4,
                         publication_codes = $5,
                         solution_uid = $6, procedure_uid = $7, office_uid = $8,
                         legal_domain_uid = $9,
                         seat = $13, chamber_key = $14, formation_uid = $10,
                         jurisdiction_code = COALESCE($11, d.jurisdiction_code),
                         extract_version = $12, updated_at = now()
                     FROM cible
                     WHERE d.id = cible.id
                       AND (d.date_lecture, d.date_audience, d.docket_numbers,
                            d.publication_codes, d.solution_uid, d.procedure_uid,
                            d.office_uid, d.legal_domain_uid,
                            d.seat, d.chamber_key, d.formation_uid,
                            d.jurisdiction_code, d.extract_version)
                           IS DISTINCT FROM
                           ($2, $3, $4, $5, $6, $7, $8, $9, $13, $14, $10,
                            COALESCE($11, d.jurisdiction_code), $12)
                     RETURNING 1
                 )
                 SELECT (SELECT count(*) FROM cible), (SELECT count(*) FROM maj)",
                &[
                    &source_uid,
                    &fields.date_lecture,
                    &fields.date_audience,
                    &fields.docket_numbers,
                    &fields.publication_codes,
                    &fields.solution_uid,
                    &fields.procedure_uid,
                    &fields.office_uid,
                    &fields.legal_domain_uid,
                    &fields.formation_uid,
                    &fields.jurisdiction_code,
                    &version,
                    &seat,
                    &chamber_key,
                ],
            )
            .await?;
        let trouvee: i64 = row.get(0);
        let ecrite: i64 = row.get(1);
        Ok(match (trouvee, ecrite) {
            (0, _) => ManualUpsert::NotFound,
            (_, 0) => ManualUpsert::Unchanged,
            _ => ManualUpsert::Applied,
        })
    }

    /// Remplace la couche citations d'une décision à une `extract_version`
    /// explicite : le set fourni EST la vérité, DELETE + COPY via l'écrivain commun.
    /// Les `rows` arrivent déjà validées par l'appelant. `None` si aucun
    /// `source_uid` ne matche ou si la décision porte une version supérieure, sinon
    /// le nombre de lignes insérées.
    pub async fn replace_citations_at_version(
        &self,
        source_uid: &str,
        version: i16,
        rows: &[super::types::CitationOccurrenceRow],
    ) -> Result<Option<u64>> {
        let row = self
            .conn
            .query_opt(
                "SELECT ds.decision_id FROM decision_sources ds
                 JOIN decisions d ON d.id = ds.decision_id
                 WHERE ds.source_uid = $1
                   AND (d.extract_version IS NULL OR d.extract_version <= $2)",
                &[&source_uid, &version],
            )
            .await?;
        let Some(row) = row else { return Ok(None) };
        let decision_id: i64 = row.get(0);
        self.write_citation_occurrences(&[(decision_id, rows)], version)
            .await?;
        Ok(Some(rows.len() as u64))
    }

    /// Keyset des décisions à re-parser ciblées par `jurisdiction_type`,
    /// indépendamment de `extract_version` et sans bump global. Révisions
    /// manuelles exclues. `text_query` restreint au gisement d'une expression
    /// BM25 : une grammaire neuve n'atteint que les textes qui portent le
    /// littéral qu'elle exige, au lieu du fonds entier.
    pub async fn decision_ids_for_reextract_by_juridiction(
        &self,
        last_id: i64,
        limit: i64,
        jurisdiction_types: &[String],
        text_query: Option<&str>,
    ) -> Result<Vec<i64>> {
        let filtre = match text_query {
            Some(_) => "AND d.id @@@ paradedb.parse_with_field('full_text', $5)",
            None => "",
        };
        let sql = format!(
            "SELECT d.id
             FROM decisions d
             WHERE d.id > $1
               AND d.full_text IS NOT NULL
               AND d.jurisdiction_type = ANY($3)
               AND (d.extract_version IS NULL OR d.extract_version <= $4)
               {filtre}
             ORDER BY d.id
             LIMIT $2"
        );
        let rows = match text_query {
            Some(q) => {
                let q = q.to_string();
                self.conn
                    .query(
                        sql.as_str(),
                        &[&last_id, &limit, &jurisdiction_types, &EXTRACT_VERSION, &q],
                    )
                    .await?
            }
            None => {
                self.conn
                    .query(
                        sql.as_str(),
                        &[&last_id, &limit, &jurisdiction_types, &EXTRACT_VERSION],
                    )
                    .await?
            }
        };
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Décisions à re-parser ciblées par un texte cité (`ref_text_uid`),
    /// indépendamment de `extract_version` : à combiner avec
    /// `--field legal_references`. Un seul scan, GIN-piloté : la work-list part de
    /// `legal_citation` (`lj_cit_terms`), pas d'un keyset sur `decisions`.
    /// L'appelant lève le `statement_timeout`.
    pub async fn decision_ids_for_reextract_by_citing_ref_uid(
        &self,
        ref_text_uid: &str,
    ) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM (SELECT DISTINCT lc.decision_id
                      FROM legal_citation lc
                      WHERE public.lj_cit_terms(lc.spans) @> ARRAY[$1]) c
                JOIN decisions d ON d.id = c.decision_id
                WHERE d.full_text IS NOT NULL
                  AND (d.extract_version IS NULL OR d.extract_version <= $2)
                ORDER BY d.id
                ",
                &[&ref_text_uid, &EXTRACT_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    #[tracing::instrument(name = "db.set_summary", skip(self, summary), fields(db.system = "postgresql"))]
    pub async fn set_summary(
        &self,
        decision_id: i64,
        summary: &str,
        prompt_version: i16,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE decisions SET summary = $1, summary_prompt_version = $2, \
                 updated_at = $3 WHERE id = $4",
                &[&summary, &prompt_version, &now(), &decision_id],
            )
            .await?;
        Ok(())
    }

    /// Itère `(public_id, lastmod)` pour toutes les décisions, en ordre PK.
    /// `lastmod` = `GREATEST(date_lecture, updated_at::date)`. Les décisions sans
    /// `public_id` sont exclues ; pagination keyset par `d.id`, matérialisée en
    /// `Vec`.
    #[tracing::instrument(name = "db.iter_decisions_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_decisions_for_sitemap(
        &self,
        batch_size: i64,
    ) -> Result<Vec<(String, Option<NaiveDate>)>> {
        let mut out: Vec<(String, Option<NaiveDate>)> = Vec::new();
        let mut last_id: i64 = 0;
        loop {
            let rows = self
                .conn
                .query(
                    "
                SELECT id, public_id,
                       GREATEST(date_lecture, updated_at::date) AS lastmod
                FROM decisions
                WHERE public_id IS NOT NULL
                  AND id > $1
                ORDER BY id
                LIMIT $2
                ",
                    &[&last_id, &batch_size],
                )
                .await?;
            if rows.is_empty() {
                return Ok(out);
            }
            for row in &rows {
                let public_id: String = row.get(1);
                let lastmod: Option<NaiveDate> = row.get(2);
                out.push((public_id, lastmod));
            }
            last_id = rows[rows.len() - 1].get::<_, i64>(0);
            if (rows.len() as i64) < batch_size {
                return Ok(out);
            }
        }
    }

    /// Remplace l'intégralité de la table `sitemaps` par `files` : les sub-sitemaps
    /// d'un run précédent disparaissent. L'appelant wrappe l'appel dans une
    /// transaction.
    #[tracing::instrument(name = "db.replace_sitemaps", skip(self, files), fields(db.system = "postgresql", count = files.len()))]
    pub async fn replace_sitemaps(&self, files: &[SitemapRow]) -> Result<()> {
        self.conn.execute("DELETE FROM sitemaps", &[]).await?;
        for f in files {
            self.conn
                .execute(
                    "INSERT INTO sitemaps (filename, content_type, body, lastmod) \
                     VALUES ($1, $2, $3, $4)",
                    &[&f.filename, &f.content_type, &f.body, &f.lastmod],
                )
                .await?;
        }
        Ok(())
    }

    /// Lit un sitemap par nom de fichier — `(body, content_type)`, `None` si absent.
    #[tracing::instrument(name = "db.fetch_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn fetch_sitemap(&self, filename: &str) -> Result<Option<(Vec<u8>, String)>> {
        let row = self
            .conn
            .query_opt(
                "SELECT body, content_type FROM sitemaps WHERE filename = $1",
                &[&filename],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), r.get(1))))
    }

    /// Itère les `public_id` des décisions dont `updated_at >= since`. Pagination
    /// keyset par `id`, matérialisée en `Vec`.
    #[tracing::instrument(name = "db.iter_public_ids_updated_since", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_public_ids_updated_since(
        &self,
        since: DateTime<Utc>,
        batch_size: i64,
    ) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        let mut last_id: i64 = 0;
        loop {
            let rows = self
                .conn
                .query(
                    "
                SELECT id, public_id
                FROM decisions
                WHERE public_id IS NOT NULL
                  AND updated_at >= $1
                  AND id > $2
                ORDER BY id
                LIMIT $3
                ",
                    &[&since, &last_id, &batch_size],
                )
                .await?;
            if rows.is_empty() {
                return Ok(out);
            }
            for row in &rows {
                out.push(row.get(1));
            }
            last_id = rows[rows.len() - 1].get::<_, i64>(0);
            if (rows.len() as i64) < batch_size {
                return Ok(out);
            }
        }
    }

    /// Itère les décisions sans summary à jour, par batches.
    ///
    /// Chaque ligne = `(decision_id, public_id, jurisdiction_type,
    /// jurisdiction_name, date_lecture, docket_numbers)` — les quatre dernières
    /// reconstruisent le titre côté appelant. `summary IS NULL`, plus avec
    /// `include_stale` les `summary_prompt_version < target_version`. Tombstones et
    /// décisions sans corps exclues. `limit` borne le total yieldé sur toute
    /// l'itération ; `wrap` reprend sur `]0, start_id]` une fois l'arc
    /// `]start_id, max]` épuisé.
    ///
    /// Renvoie tous les batches matérialisés : les frontières de batch, donc
    /// l'identité des appels Mistral concurrents, sont préservées.
    #[allow(clippy::type_complexity)]
    #[tracing::instrument(name = "db.iter_decisions_missing_summary", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_decisions_missing_summary(
        &self,
        target_version: i16,
        batch_size: i64,
        limit: Option<i64>,
        start_id: i64,
        wrap: bool,
        include_stale: bool,
    ) -> Result<Vec<Vec<MissingSummaryRow>>> {
        // Plafond sur l'arc courant : max bigint avant un éventuel wrap, puis
        // `start_id` une fois qu'on a bouclé sur le bas de la plage.
        let mut ceiling: i64 = i64::MAX;
        let mut wrapped = false;
        let mut emitted: i64 = 0;
        let mut last_id = start_id;
        let mut batches: Vec<Vec<MissingSummaryRow>> = Vec::new();
        loop {
            let remaining = match limit {
                None => batch_size,
                Some(lim) => batch_size.min(lim - emitted),
            };
            if remaining <= 0 {
                break;
            }
            let rows = self
                .conn
                .query(
                    "
                SELECT d.id, d.public_id, d.jurisdiction_type, j.label,
                       d.date_lecture::text, d.docket_numbers
                FROM decisions d
                LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                WHERE d.id > $1
                  AND d.id <= $2
                  AND (
                    d.summary IS NULL
                    OR ($5 AND (
                      d.summary_prompt_version IS NULL
                      OR d.summary_prompt_version < $3
                    ))
                  )
                  AND d.public_id IS NOT NULL
                  AND d.deleted_at IS NULL
                  AND d.full_text IS NOT NULL
                ORDER BY d.id
                LIMIT $4
                ",
                    &[
                        &last_id,
                        &ceiling,
                        &target_version,
                        &remaining,
                        &include_stale,
                    ],
                )
                .await?;
            let n = rows.len();
            if n > 0 {
                let mut batch: Vec<MissingSummaryRow> = Vec::with_capacity(n);
                for row in &rows {
                    let docket: Option<Vec<String>> = row.get(5);
                    batch.push(MissingSummaryRow {
                        decision_id: row.get(0),
                        public_id: row.get(1),
                        jurisdiction_type: row.get(2),
                        jurisdiction_name: row.get(3),
                        date_lecture: row.get(4),
                        docket_numbers: docket.filter(|v| !v.is_empty()),
                    });
                }
                emitted += n as i64;
                last_id = rows[n - 1].get::<_, i64>(0);
                batches.push(batch);
            }
            if n == 0 || (n as i64) < remaining {
                if wrap && !wrapped && start_id > 0 {
                    wrapped = true;
                    ceiling = start_id;
                    last_id = 0;
                    continue;
                }
                break;
            }
        }
        Ok(batches)
    }
}

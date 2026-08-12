//! Provenance, résolution d'identité et déduplication : sondes d'idempotence,
//! backfills, fusion de clusters doublons, retrait par provenance (tombstone) +
//! reconcile.

use std::collections::{HashMap, HashSet};

use super::support::{now, source_from_source_uid};
use super::types::ExistingDecisionState;
use super::DecisionRepository;
use crate::error::Result;
use serde_json::Value;

impl DecisionRepository<'_> {
    /// Sonde d'idempotence intra-source sur le pivot `decision_sources.source_uid`
    /// (UNIQUE). Renvoie `(decision_id, content_checksum, active)`, provenance
    /// tombstonée comprise : un `source_uid` connu mais tombstoné se ré-update en
    /// place. `None` = provenance jamais vue → résolution d'identité en aval.
    #[tracing::instrument(name = "db.find_provenance", skip(self), fields(db.system = "postgresql"))]
    pub async fn find_provenance(&self, source_uid: &str) -> Result<Option<(i64, String, bool)>> {
        let row = self
            .conn
            .query_opt(
                "SELECT decision_id, content_checksum, deleted_at IS NULL AS active \
                 FROM decision_sources WHERE source_uid = $1",
                &[&source_uid],
            )
            .await?;
        Ok(row.map(|r| {
            (
                r.get::<_, i64>(0),
                r.get::<_, String>(1),
                r.get::<_, bool>(2),
            )
        }))
    }

    /// Cible de fusion par `canonical_ref`, sous l'invariant *≤1 provenance active
    /// par source et par décision* : `min(id)` parmi les décisions actives portant
    /// cette clé qui ne portent pas déjà une provenance active de `incoming_source`.
    /// `None` → aucune cible compatible, l'appelant crée une nouvelle décision. On ne
    /// fusionne donc jamais deux provenances same-source par `canonical_ref` ; le merge
    /// cross-source reste autorisé.
    #[tracing::instrument(name = "db.find_canonical_ref_merge_target", skip(self), fields(db.system = "postgresql"))]
    pub async fn find_canonical_ref_merge_target(
        &self,
        canonical_ref: &str,
        incoming_source: &str,
    ) -> Result<Option<i64>> {
        let row = self
            .conn
            .query_one(
                "SELECT min(d.id) FROM decisions d \
                 WHERE d.canonical_ref = $1 AND d.deleted_at IS NULL \
                   AND NOT EXISTS ( \
                     SELECT 1 FROM decision_sources s \
                     WHERE s.decision_id = d.id AND s.source = $2 \
                       AND s.deleted_at IS NULL)",
                &[&canonical_ref, &incoming_source],
            )
            .await?;
        Ok(row.get::<_, Option<i64>>(0))
    }

    /// `source_uid` de la provenance **autoritaire** : rang max, puis rendition
    /// FR-prioritaire (`lang = 'fra'` DESC), puis `source_uid` croissant, tombstonée
    /// comprise. `None` si la décision n'a aucune provenance. Seule l'autorité écrit
    /// `full_text` et les métadonnées.
    #[tracing::instrument(name = "db.authority_source_uid", skip(self), fields(db.system = "postgresql"))]
    pub(super) async fn authority_source_uid(&self, decision_id: i64) -> Result<Option<String>> {
        let row = self
            .conn
            .query_opt(
                "SELECT source_uid FROM decision_sources WHERE decision_id = $1 \
                 ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, source_uid ASC LIMIT 1",
                &[&decision_id],
            )
            .await?;
        Ok(row.map(|r| r.get::<_, String>(0)))
    }

    /// Toutes les provenances CNDA actives, avec de quoi recalculer `solution_uid`
    /// sans re-télécharger le PDF : `(source_uid, full_text, source_fields)`.
    #[tracing::instrument(name = "db.cnda_sources_for_solution", skip(self), fields(db.system = "postgresql"))]
    pub async fn cnda_sources_for_solution(&self) -> Result<Vec<(String, String, Value)>> {
        let rows = self
            .conn
            .query(
                "SELECT s.source_uid, d.full_text, s.source_fields \
                 FROM decision_sources s JOIN decisions d ON d.id = s.decision_id \
                 WHERE s.source_uid LIKE 'cnda/%' \
                   AND s.deleted_at IS NULL \
                   AND d.full_text IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Aligne `solution_uid` dans les `source_fields` sur ce que le parser dit
    /// aujourd'hui : pose la clé pour `set`, la retire pour `remove`. Rejouable —
    /// chaque branche ne touche que les lignes qui changent. Renvoie
    /// `(posées, retirées)`.
    #[tracing::instrument(name = "db.align_source_fields_solution_uid", skip(self, set_uids, set_values, remove_uids), fields(db.system = "postgresql", set = set_uids.len(), remove = remove_uids.len()))]
    pub async fn align_source_fields_solution_uid(
        &self,
        set_uids: &[String],
        set_values: &[String],
        remove_uids: &[String],
    ) -> Result<(u64, u64)> {
        let posees = if set_uids.is_empty() {
            0
        } else {
            self.conn
                .execute(
                    "UPDATE decision_sources s \
                     SET source_fields = jsonb_set(s.source_fields, ARRAY['solution_uid'], \
                                                   to_jsonb(v.solution_uid)) \
                     FROM UNNEST($1::text[], $2::text[]) AS v(source_uid, solution_uid) \
                     WHERE s.source_uid = v.source_uid \
                       AND s.source_fields->>'solution_uid' IS DISTINCT FROM v.solution_uid",
                    &[&set_uids, &set_values],
                )
                .await?
        };
        let retirees = if remove_uids.is_empty() {
            0
        } else {
            self.conn
                .execute(
                    "UPDATE decision_sources s \
                     SET source_fields = s.source_fields - 'solution_uid' \
                     WHERE s.source_uid = ANY($1) \
                       AND s.source_fields->'solution_uid' IS NOT NULL",
                    &[&remove_uids],
                )
                .await?
        };
        Ok((posees, retirees))
    }

    #[tracing::instrument(name = "db.upsert_decision_source", skip(self, source_fields), fields(db.system = "postgresql"))]
    pub async fn upsert_decision_source(
        &self,
        decision_id: i64,
        source_uid: &str,
        content_checksum: &str,
        payload_format: &str,
        source_fields: &Value,
    ) -> Result<()> {
        // `source_rank` est une colonne **générée** depuis `source`, jamais écrite ici.
        let source = source_from_source_uid(source_uid);
        // `lang` = langue de la source, **dite par l'ingester** dans
        // `source_fields["lang"]` (CEDH/CJUE ; 'fra'/'eng'), matérialisée en colonne.
        // Absente pour les sources FR-only → NULL.
        let lang = source_fields.get("lang").and_then(|v| v.as_str());
        self.conn
            .execute(
                "
                INSERT INTO decision_sources
                  (decision_id, source, source_uid, content_checksum,
                   payload_format, source_fields, lang)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (source_uid) DO UPDATE SET
                  decision_id = EXCLUDED.decision_id,
                  source = EXCLUDED.source,
                  content_checksum = EXCLUDED.content_checksum,
                  payload_format = EXCLUDED.payload_format,
                  source_fields = EXCLUDED.source_fields,
                  lang = EXCLUDED.lang,
                  deleted_at = NULL,
                  ingested_at = NOW()
                ",
                &[
                    &decision_id,
                    &source,
                    &source_uid,
                    &content_checksum,
                    &payload_format,
                    &source_fields,
                    &lang,
                ],
            )
            .await?;
        Ok(())
    }

    /// Backfill batché (keyset par `decisions.id`) de `decisions.ecli` depuis
    /// `decision_sources.source_fields->>'ecli'` de la provenance autoritaire.
    /// Idempotent : n'écrit que les lignes dont `ecli IS NULL` et dont `source_fields`
    /// porte la clé. L'ECLI n'étant pas unique, il peut se poser sur plusieurs
    /// décisions distinctes. Renvoie `(rows_updated, last_id)`, `None` quand le lot est
    /// vide.
    #[tracing::instrument(name = "db.backfill_ecli_batch", skip(self), fields(db.system = "postgresql"))]
    pub async fn backfill_ecli_batch(
        &self,
        after_id: i64,
        batch: i64,
    ) -> Result<Option<(u64, i64)>> {
        let row = self
            .conn
            .query_one(
                "
                WITH lot AS (
                    SELECT d.id
                    FROM decisions d
                    WHERE d.id > $1
                    ORDER BY d.id
                    LIMIT $2
                ),
                upd AS (
                    UPDATE decisions d
                    SET ecli = ds.source_fields->>'ecli'
                    FROM lot l
                    JOIN LATERAL (
                        SELECT s.source_fields
                        FROM decision_sources s
                        WHERE s.decision_id = l.id
                          AND s.deleted_at IS NULL
                          AND s.source_fields ? 'ecli'
                        ORDER BY s.source_rank DESC, (s.lang = 'fra') IS TRUE DESC, s.source_uid ASC
                        LIMIT 1
                    ) ds ON true
                    WHERE d.id = l.id
                      AND d.ecli IS NULL
                    RETURNING 1
                )
                SELECT
                    (SELECT count(*) FROM upd) AS updated,
                    (SELECT max(id) FROM lot)  AS last_id
                ",
                &[&after_id, &batch],
            )
            .await?;
        let updated = row.get::<_, i64>(0) as u64;
        match row.get::<_, Option<i64>>(1) {
            Some(last_id) => Ok(Some((updated, last_id))),
            None => Ok(None),
        }
    }

    /// Keyset (par `decisions.id`) des décisions dont le `canonical_ref` reste à
    /// calculer : `canonical_ref IS NULL` et `full_text` présent. Une décision sans clé
    /// exploitable reste `NULL` et sera relue à chaque rerun.
    pub async fn decision_ids_for_canonical_ref_backfill(
        &self,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM decisions d
                WHERE d.id > $1
                  AND d.canonical_ref IS NULL
                  AND d.full_text IS NOT NULL
                ORDER BY d.id
                LIMIT $2
                ",
                &[&after_id, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Keyset (par `decisions.id`) de **toutes** les décisions à `canonical_ref`
    /// recalculable (`full_text` présent), pour un re-dérive forcé : la colonne n'étant
    /// plus `NULL`, le backfill normal ne les revisite pas. Une clé recalculée `None`
    /// laisse l'existante.
    pub async fn decision_ids_for_canonical_ref_recompute(
        &self,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM decisions d
                WHERE d.id > $1
                  AND d.full_text IS NOT NULL
                ORDER BY d.id
                LIMIT $2
                ",
                &[&after_id, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Écrit en masse le `canonical_ref` calculé en Rust, `unnest` apparié
    /// `(id, key)`. N'inclut que les décisions ayant produit une clé — celles sans clé
    /// restent `NULL`. Renvoie le nombre de lignes mises à jour.
    #[tracing::instrument(name = "db.update_canonical_refs_bulk", skip(self, ids, keys), fields(db.system = "postgresql", n = ids.len()))]
    pub async fn update_canonical_refs_bulk(&self, ids: &[i64], keys: &[String]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let n = self
            .conn
            .execute(
                "
                UPDATE decisions d
                SET canonical_ref = m.canonical_ref
                FROM (SELECT unnest($1::bigint[]) AS id, unnest($2::text[]) AS canonical_ref) m
                WHERE d.id = m.id
                ",
                &[&ids, &keys],
            )
            .await?;
        Ok(n)
    }

    #[tracing::instrument(name = "db.find_ingest_states", skip(self, source_uids), fields(db.system = "postgresql"))]
    pub async fn find_ingest_states(
        &self,
        source_uids: &[String],
    ) -> Result<HashMap<String, ExistingDecisionState>> {
        if source_uids.is_empty() {
            return Ok(HashMap::new());
        }
        // Idempotence par provenance : on lit `source_uid` / `content_checksum` sur
        // `decision_sources`, join `decisions` pour `public_id`. Provenances actives
        // seules : une tombstonée n'apparaît pas ici, le candidat retombe en survivant et
        // `upsert` la ressuscite par `source_uid`.
        let rows = self
            .conn
            .query(
                "
                SELECT
                  ds.source_uid,
                  ds.decision_id,
                  ds.content_checksum,
                  (
                    EXISTS (SELECT 1 FROM decision_chunks ch WHERE ch.decision_id = ds.decision_id)
                    AND NOT EXISTS (
                      SELECT 1 FROM decision_chunks ch
                      WHERE ch.decision_id = ds.decision_id AND ch.embedding IS NULL
                    )
                  ),
                  d.public_id
                FROM decision_sources ds
                JOIN decisions d ON d.id = ds.decision_id
                WHERE ds.source_uid = ANY($1) AND ds.deleted_at IS NULL
                ",
                &[&source_uids],
            )
            .await?;
        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            let source_uid: String = row.get(0);
            out.insert(
                source_uid.clone(),
                ExistingDecisionState {
                    id: row.get(1),
                    source_uid,
                    content_checksum: row.get(2),
                    has_embeddings: row.get(3),
                    public_id: row.get(4),
                },
            );
        }
        Ok(out)
    }

    /// Sous-ensemble des `source_uid` présents (provenances actives) dont la
    /// rendition servie est le français (`lang = 'fra'`). Les syncs CEDH/CJUE s'en
    /// servent pour ne re-fetcher que les nouveaux et les EN-only : un arrêt déjà en FR
    /// est définitif.
    #[tracing::instrument(name = "db.find_fr_source_uids", skip(self, source_uids), fields(db.system = "postgresql"))]
    pub async fn find_fr_source_uids(&self, source_uids: &[String]) -> Result<HashSet<String>> {
        if source_uids.is_empty() {
            return Ok(HashSet::new());
        }
        let rows = self
            .conn
            .query(
                "
                SELECT ds.source_uid
                FROM decision_sources ds
                WHERE ds.source_uid = ANY($1) AND ds.deleted_at IS NULL
                  AND ds.lang = 'fra'
                ",
                &[&source_uids],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// Résout une décision canonique EXISTANTE par identité : `canonical_ref`, seul
    /// axe de rattachement. `None` si aucune identité connue → création.
    pub async fn resolve_identity(
        &self,
        decision: &lj_core::decision::Decision,
        canonical_ref: Option<&str>,
    ) -> Result<Option<i64>> {
        if let Some(key) = canonical_ref {
            // Invariant ≤1 provenance/source : on ne rattache par `canonical_ref` qu'à une
            // décision ne portant **pas déjà** la source entrante — jamais de fusion
            // intra-source (clé non unique → perte de texte irréversible si faux merge).
            // Le merge cross-source reste permis.
            let source = source_from_source_uid(&decision.source_uid);
            if let Some(id) = self.find_canonical_ref_merge_target(key, source).await? {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Retrait d'une provenance, **toujours par `source_uid`** et jamais par
    /// identité canonique : tombstone la ligne `decision_sources`, puis
    /// `reconcile(decision_id)` recalcule la décision. Les deux sont atomiques et sous
    /// verrou (`pg_advisory_xact_lock` sur `decision_id`), les appelants tournant en
    /// autocommit. Le squelette (id, public_id, identité) reste : un ré-update se
    /// rejoint par `source_uid`. `source_uid` inconnu → no-op idempotent (`false`).
    #[tracing::instrument(name = "db.delete", skip(self), fields(db.system = "postgresql"))]
    pub async fn delete(&self, source_uid: &str) -> Result<bool> {
        self.conn.batch_execute("BEGIN").await?;
        let res = self.delete_locked(source_uid).await;
        match res {
            Ok(deleted) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(deleted)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Corps transactionnel de [`delete`](Self::delete) : résout la provenance, pose
    /// le verrou xact sur `decision_id`, tombstone, `reconcile`. À exécuter dans la
    /// transaction ouverte par l'appelant.
    async fn delete_locked(&self, source_uid: &str) -> Result<bool> {
        let row = self
            .conn
            .query_opt(
                "SELECT decision_id FROM decision_sources WHERE source_uid = $1",
                &[&source_uid],
            )
            .await?;
        let Some(row) = row else {
            // Provenance inconnue : rien à retirer.
            return Ok(false);
        };
        let decision_id: i64 = row.get(0);
        // Verrou par décision (auto-relâché en fin de transaction) : sérialise retrait
        // et upsert concurrents sur la même décision.
        self.conn
            .execute("SELECT pg_advisory_xact_lock($1)", &[&decision_id])
            .await?;
        self.conn
            .execute(
                "UPDATE decision_sources SET deleted_at = NOW() WHERE source_uid = $1",
                &[&source_uid],
            )
            .await?;
        self.reconcile(decision_id).await?;
        Ok(true)
    }

    /// `reconcile(decision_id)` — recalcule la décision canonique comme **fonction
    /// pure de ses `decision_sources`**. À appeler après toute mutation de provenance,
    /// sous verrou, dans la même transaction.
    ///
    /// 1. **0 provenance** → hard-delete la décision (cascade chunks/refs).
    /// 2. **autorité** = `source_rank` max (active ou tombstonée), départagée par
    ///    rendition FR-prioritaire puis `source_uid` croissant.
    /// 3. **autorité active** → décision active, `deleted_at = NULL`. `reconcile` ne
    ///    re-dérive jamais le texte : il est posé par l'ingest de la provenance.
    /// 4. **autorité tombstonée** → vidé : `full_text`/`summary` NULL,
    ///    `deleted_at = NOW()`, chunks + payload brut + mots-clés supprimés, même si
    ///    une source inférieure est encore active.
    #[tracing::instrument(name = "db.reconcile", skip(self), fields(db.system = "postgresql"))]
    pub async fn reconcile(&self, decision_id: i64) -> Result<()> {
        // Autorité = rang max, active ou tombstonée. Son `deleted_at` décide de l'état
        // de la décision.
        let authority = self
            .conn
            .query_opt(
                "SELECT deleted_at IS NULL AS active \
                 FROM decision_sources WHERE decision_id = $1 \
                 ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, source_uid ASC LIMIT 1",
                &[&decision_id],
            )
            .await?;

        let Some(authority) = authority else {
            // 0 provenance → hard-delete (cascade chunks/refs/full_text).
            self.conn
                .execute("DELETE FROM decisions WHERE id = $1", &[&decision_id])
                .await?;
            return Ok(());
        };

        let authority_active: bool = authority.get("active");
        if authority_active {
            // Autorité active → décision active. `full_text`/métadonnées déjà posés par
            // l'ingest de l'autorité ; on lève juste un éventuel vide.
            self.conn
                .execute(
                    "UPDATE decisions SET deleted_at = NULL \
                     WHERE id = $1 AND deleted_at IS NOT NULL",
                    &[&decision_id],
                )
                .await?;
            return Ok(());
        }

        // Autorité tombstonée → vidé : on retire tout porteur de données perso (texte,
        // chunks+embeddings, payload brut, résumé).
        self.conn
            .execute(
                "UPDATE decisions SET deleted_at = NOW(), full_text = NULL, summary = NULL \
                 WHERE id = $1",
                &[&decision_id],
            )
            .await?;
        self.conn
            .execute(
                "DELETE FROM decision_chunks WHERE decision_id = $1",
                &[&decision_id],
            )
            .await?;
        self.conn
            .execute(
                "DELETE FROM decision_full_text WHERE decision_id = $1",
                &[&decision_id],
            )
            .await?;
        Ok(())
    }

    /// Prédicat d'un tombstone **orphelin** (retrait total, purgeable) : décision
    /// retirée, vidée (`full_text IS NULL`) et sans aucune provenance active — une
    /// provenance active la ressusciterait via [`reconcile`](Self::reconcile). Partagé
    /// par le comptage, l'échantillon et le DELETE, qui visent la même cible.
    const ORPHAN_TOMBSTONE_WHERE: &'static str = "d.deleted_at IS NOT NULL \
         AND d.full_text IS NULL \
         AND NOT EXISTS ( \
           SELECT 1 FROM decision_sources s \
           WHERE s.decision_id = d.id AND s.deleted_at IS NULL)";

    /// Compte les tombstones orphelins (cf. [`Self::ORPHAN_TOMBSTONE_WHERE`]).
    pub async fn count_orphan_tombstones(&self) -> Result<i64> {
        let sql = format!(
            "SELECT count(*) FROM decisions d WHERE {}",
            Self::ORPHAN_TOMBSTONE_WHERE
        );
        Ok(self.conn.query_one(&sql, &[]).await?.get(0))
    }

    /// Échantillon de `source_uid` d'orphelins (contrôle avant purge).
    pub async fn sample_orphan_tombstones(&self, limit: i64) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT (SELECT source_uid FROM decision_sources \
                     WHERE decision_id = d.id LIMIT 1) \
             FROM decisions d WHERE {} LIMIT $1",
            Self::ORPHAN_TOMBSTONE_WHERE
        );
        let rows = self.conn.query(&sql, &[&limit]).await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.get::<_, Option<String>>(0))
            .collect())
    }

    /// Hard-delete des tombstones orphelins (cascade FK). Renvoie le nombre
    /// supprimé. La clause re-vérifie l'absence de provenance active au moment du
    /// DELETE : une réactivation concurrente sort la ligne de la cible.
    pub async fn purge_orphan_tombstones(&self) -> Result<u64> {
        let sql = format!(
            "DELETE FROM decisions d WHERE {}",
            Self::ORPHAN_TOMBSTONE_WHERE
        );
        Ok(self.conn.execute(&sql, &[]).await?)
    }

    /// Détache un groupe de provenances faussement fusionnées du canonique vers une
    /// **nouvelle** décision — l'inverse d'une fusion. Sous la transaction de
    /// l'appelant : INSERT d'une ligne `decisions` squelette (identité posée via
    /// `canonical_ref`, contenu rempli ensuite par la ré-ingestion), puis re-pointage
    /// des `decision_sources` du groupe. **Idempotent** : un groupe déjà scindé ne
    /// touche 0 ligne.
    pub async fn create_split_decision(
        &self,
        canonical_id: i64,
        group_source_uids: &[String],
        jurisdiction_type: &str,
        public_id: &str,
        canonical_ref: &str,
    ) -> Result<i64> {
        // 1. Squelette : identité posée, contenu (full_text/chunks/embeddings) laissé
        //    vide — rempli par la ré-ingestion du re-fetch (update_existing).
        let new_id: i64 = self
            .conn
            .query_one(
                "INSERT INTO decisions (jurisdiction_type, public_id, updated_at, \
                   canonical_ref) \
                 VALUES ($1, $2, $3, $4) RETURNING id",
                &[&jurisdiction_type, &public_id, &now(), &canonical_ref],
            )
            .await?
            .get(0);
        // 2. Re-pointe le groupe : SEULEMENT les provenances encore sur le canonique
        //    (idempotence : un groupe déjà déplacé ne rebouge pas).
        self.conn
            .execute(
                "UPDATE decision_sources SET decision_id = $1 \
                 WHERE source_uid = ANY($2) AND decision_id = $3",
                &[&new_id, &group_source_uids, &canonical_id],
            )
            .await?;
        Ok(new_id)
    }

    /// Clusters de décisions actives à **fusionner** (faux splits cross-source) :
    /// même `canonical_ref`, ≥2 décisions actives, **sources disjointes**. La
    /// disjonction garantit qu'aucune fusion ne recolle deux provenances same-source ;
    /// les affaires sérielles same-source restent donc scindées. Renvoie, par cluster,
    /// `(decision_id, max_source_rank)` triés rang décroissant puis id croissant :
    /// l'appelant garde `[0]` et fusionne les suivants dedans.
    #[tracing::instrument(name = "db.fetch_cross_source_merge_groups", skip(self), fields(db.system = "postgresql"))]
    pub async fn fetch_cross_source_merge_groups(&self) -> Result<Vec<Vec<(i64, i32)>>> {
        let rows = self
            .conn
            .query(
                "
                WITH prov AS (
                    SELECT d.id AS decision_id, d.canonical_ref, s.source, s.source_rank
                    FROM decisions d
                    JOIN decision_sources s ON s.decision_id = d.id AND s.deleted_at IS NULL
                    WHERE d.deleted_at IS NULL AND d.canonical_ref IS NOT NULL
                ),
                per_ref AS (
                    SELECT canonical_ref,
                           count(DISTINCT decision_id)            AS n_dec,
                           count(DISTINCT source)                 AS n_src,
                           count(DISTINCT (decision_id, source))  AS n_dec_src
                    FROM prov GROUP BY canonical_ref
                ),
                safe AS (
                    -- ≥2 décisions ET sources disjointes (n_src = n_dec_src ⟺ aucune
                    -- source répétée entre décisions du cluster).
                    SELECT canonical_ref FROM per_ref
                    WHERE n_dec > 1 AND n_src = n_dec_src
                )
                SELECT p.canonical_ref, p.decision_id, max(p.source_rank)::int AS max_rank
                FROM prov p JOIN safe USING (canonical_ref)
                GROUP BY p.canonical_ref, p.decision_id
                ORDER BY p.canonical_ref, max_rank DESC, p.decision_id
                ",
                &[],
            )
            .await?;
        // Regroupe les lignes (déjà triées par canonical_ref) en clusters.
        let mut groups: Vec<Vec<(i64, i32)>> = Vec::new();
        let mut cur_ref: Option<String> = None;
        for row in rows {
            let cref: String = row.get(0);
            let pair = (row.get::<_, i64>(1), row.get::<_, i32>(2));
            if cur_ref.as_deref() == Some(cref.as_str()) {
                groups.last_mut().expect("cluster ouvert").push(pair);
            } else {
                groups.push(vec![pair]);
                cur_ref = Some(cref);
            }
        }
        Ok(groups)
    }

    /// `source_uid` opendata des décisions dont l'autorité est opendata mais dont le
    /// `full_text` reste figé sur l'autre provenance (rang 50). Cible du re-ingest
    /// opendata ciblé : elles seules doivent re-parser leur payload pour que
    /// `full_text`/chunks/embeddings repassent sur le texte opendata. Exclut les
    /// décisions portant une provenance judilibre, qui reste autorité.
    #[tracing::instrument(name = "db.opendata_source_uids_stale_authority", skip(self), fields(db.system = "postgresql"))]
    pub async fn opendata_source_uids_stale_authority(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "
                SELECT od.source_uid
                FROM decision_sources od
                WHERE od.source = 'opendata' AND od.deleted_at IS NULL
                  AND EXISTS (
                      SELECT 1 FROM decision_sources o
                      WHERE o.decision_id = od.decision_id
                        AND o.source <> 'opendata' AND o.source_rank = 50
                        AND o.deleted_at IS NULL)
                  AND NOT EXISTS (
                      SELECT 1 FROM decision_sources j
                      WHERE j.decision_id = od.decision_id
                        AND j.source = 'judilibre' AND j.deleted_at IS NULL)
                ",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
    }

    /// Fusionne `loser_id` dans `keeper_id` — l'inverse de
    /// [`create_split_decision`](Self::create_split_decision). Re-pointe toutes les
    /// provenances du perdant (actives + tombstonées) vers le gardien, puis `reconcile`
    /// les deux : le perdant, sans provenance, est hard-delete. Transactionnel, sous
    /// verrous ordonnés (anti-deadlock). **Sûr uniquement si les sources sont
    /// disjointes**, ce que garantit
    /// [`fetch_cross_source_merge_groups`](Self::fetch_cross_source_merge_groups). Le
    /// contenu du gardien est conservé ; aucun re-embed. No-op si
    /// `keeper_id == loser_id`.
    #[tracing::instrument(name = "db.merge_into", skip(self), fields(db.system = "postgresql"))]
    pub async fn merge_into(&self, keeper_id: i64, loser_id: i64) -> Result<()> {
        if keeper_id == loser_id {
            return Ok(());
        }
        self.conn.batch_execute("BEGIN").await?;
        let res = self.merge_into_locked(keeper_id, loser_id).await;
        match res {
            Ok(()) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Corps transactionnel de [`merge_into`](Self::merge_into) : verrous ordonnés,
    /// re-pointage, reconcile des deux décisions. À exécuter dans la transaction
    /// ouverte par l'appelant.
    async fn merge_into_locked(&self, keeper_id: i64, loser_id: i64) -> Result<()> {
        let (lo, hi) = if keeper_id < loser_id {
            (keeper_id, loser_id)
        } else {
            (loser_id, keeper_id)
        };
        self.conn
            .execute("SELECT pg_advisory_xact_lock($1)", &[&lo])
            .await?;
        self.conn
            .execute("SELECT pg_advisory_xact_lock($1)", &[&hi])
            .await?;
        self.conn
            .execute(
                "UPDATE decision_sources SET decision_id = $1 WHERE decision_id = $2",
                &[&keeper_id, &loser_id],
            )
            .await?;
        self.reconcile(keeper_id).await?;
        self.reconcile(loser_id).await?;
        Ok(())
    }

    /// Provenances judilibre **perdantes** d'un faux merge intra-source : pour
    /// chaque décision portant ≥2 provenances judilibre actives, celles de rang
    /// non-autoritaire (`row_number > 1` sur `source_rank DESC, source_uid ASC`).
    /// Renvoie `(decision_id, source_uid, source_fields)` triées par `decision_id`. Le
    /// `source_fields` porte `jurisdiction` + `decision_date`, de quoi localiser le
    /// payload dans le cache local. Tombstonées exclues : on ne re-matérialise jamais
    /// ce que judilibre a supprimé.
    pub async fn fetch_same_source_judilibre_losers(&self) -> Result<Vec<(i64, String, Value)>> {
        let rows = self
            .conn
            .query(
                "WITH ranked AS ( \
                   SELECT s.decision_id, s.source_uid, s.source_fields, \
                          row_number() OVER (PARTITION BY s.decision_id \
                                             ORDER BY s.source_rank DESC, s.source_uid ASC) AS rn \
                   FROM decision_sources s \
                   WHERE s.deleted_at IS NULL AND s.source = 'judilibre' \
                     AND s.decision_id IN ( \
                       SELECT decision_id FROM decision_sources \
                       WHERE deleted_at IS NULL AND source = 'judilibre' \
                       GROUP BY decision_id HAVING count(*) >= 2) \
                 ) \
                 SELECT decision_id, source_uid, source_fields FROM ranked \
                 WHERE rn > 1 ORDER BY decision_id",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, Value>(2),
                )
            })
            .collect())
    }

    /// Provenances **perdantes** d'un faux merge intra-source `dila-jade`, analogue
    /// de [`Self::fetch_same_source_judilibre_losers`]. La re-matérialisation DILA
    /// passe par un streaming des tarballs, non indexés par décision : pas besoin du
    /// `source_fields`, on renvoie `canonical_ref` et `jurisdiction_type` de la
    /// décision pour amorcer le squelette de `create_split_decision`. Tombstonées
    /// exclues.
    pub async fn fetch_same_source_dila_jade_losers(
        &self,
    ) -> Result<Vec<(i64, String, Option<String>, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "WITH ranked AS ( \
                   SELECT s.decision_id, s.source_uid, \
                          row_number() OVER (PARTITION BY s.decision_id \
                                             ORDER BY s.source_rank DESC, s.source_uid ASC) AS rn \
                   FROM decision_sources s \
                   WHERE s.deleted_at IS NULL AND s.source = 'dila-jade' \
                     AND s.decision_id IN ( \
                       SELECT decision_id FROM decision_sources \
                       WHERE deleted_at IS NULL AND source = 'dila-jade' \
                       GROUP BY decision_id HAVING count(*) >= 2) \
                 ) \
                 SELECT r.decision_id, r.source_uid, d.canonical_ref, d.jurisdiction_type \
                 FROM ranked r JOIN decisions d ON d.id = r.decision_id \
                 WHERE r.rn > 1 ORDER BY r.decision_id",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, Option<String>>(2),
                    r.get::<_, Option<String>>(3),
                )
            })
            .collect())
    }

    /// Index de rattachement ArianeWeb : une entrée par (n° de dossier, date de
    /// lecture ISO) des décisions CE actives — `docket_numbers` dénesté. Chargé une
    /// fois par run `sync-ariane`, la jointure se fait en mémoire.
    #[tracing::instrument(name = "db.ce_docket_date_index", skip(self), fields(db.system = "postgresql"))]
    pub async fn ce_docket_date_index(&self) -> Result<Vec<(String, String, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT unnest(docket_numbers), date_lecture::text, id \
                 FROM decisions \
                 WHERE jurisdiction_type = 'CE' AND deleted_at IS NULL \
                   AND date_lecture IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, String>(0),
                    r.get::<_, String>(1),
                    r.get::<_, i64>(2),
                )
            })
            .collect())
    }

    /// Checksums des bundles ArianeWeb déjà en base (`source = 'ariane-web'`), par
    /// `source_uid` — skip d'idempotence avant upsert.
    #[tracing::instrument(name = "db.ariane_checksums", skip(self), fields(db.system = "postgresql"))]
    pub async fn ariane_checksums(&self) -> Result<HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, content_checksum FROM decision_sources \
                 WHERE source = 'ariane-web' AND deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Upsert d'un commentaire de norme. Ancré sur le **slug** du texte : le rendu le
    /// détient déjà, et une re-clé du `text_uid` en source ne peut pas orphaniner
    /// l'ancre. `num_key` NULL = commentaire du texte entier. Conflit sur `source_uid`
    /// (idempotent).
    #[tracing::instrument(name = "db.upsert_article_commentaire", skip(self, source_fields), fields(db.system = "postgresql"))]
    pub async fn upsert_article_commentaire(
        &self,
        text_slug: &str,
        num_key: Option<&str>,
        source: &str,
        source_uid: &str,
        content_checksum: &str,
        source_fields: &Value,
    ) -> Result<()> {
        self.conn
            .execute(
                "
                INSERT INTO article_commentaire
                  (text_slug, num_key, source, source_uid, content_checksum, source_fields)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (source_uid) DO UPDATE SET
                  text_slug = EXCLUDED.text_slug,
                  num_key = EXCLUDED.num_key,
                  source = EXCLUDED.source,
                  content_checksum = EXCLUDED.content_checksum,
                  source_fields = EXCLUDED.source_fields,
                  deleted_at = NULL,
                  ingested_at = NOW()
                ",
                &[
                    &text_slug,
                    &num_key,
                    &source,
                    &source_uid,
                    &content_checksum,
                    &source_fields,
                ],
            )
            .await?;
        Ok(())
    }

    /// Checksums des commentaires de norme d'une `source`, par `source_uid` — skip
    /// d'idempotence avant upsert.
    #[tracing::instrument(name = "db.article_commentaire_checksums", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_commentaire_checksums(
        &self,
        source: &str,
    ) -> Result<HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, content_checksum FROM article_commentaire \
                 WHERE source = $1 AND deleted_at IS NULL",
                &[&source],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Tombstone des commentaires d'une `source` **absents du jeu courant** : le
    /// dataset curé est la source de vérité, une entrée retirée doit disparaître de la
    /// fiche. Renvoie le nombre de lignes retirées.
    #[tracing::instrument(name = "db.tombstone_absent_article_commentaires", skip(self, keep), fields(db.system = "postgresql"))]
    pub async fn tombstone_absent_article_commentaires(
        &self,
        source: &str,
        keep: &[String],
    ) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE article_commentaire SET deleted_at = NOW() \
                 WHERE source = $1 AND deleted_at IS NULL \
                   AND NOT (source_uid = ANY($2))",
                &[&source, &keep],
            )
            .await?)
    }

    /// Commentaires d'un article de norme (page `/texte/{code}/{num_key}`) : entrées
    /// `commentaires[]` ancrées sur cet article **et** celles du texte entier
    /// (`num_key IS NULL`, ex. débats de la loi). Agrégées en un tableau jsonb, `None`
    /// si aucune.
    ///
    /// Clé = le `slug` d'URL de la page, que le handler tient déjà — aucune résolution
    /// d'identité entre l'URL et cette lecture.
    #[tracing::instrument(name = "db.article_commentaires", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_commentaires(
        &self,
        text_slug: &str,
        num_key: &str,
    ) -> Result<Option<Value>> {
        let row = self
            .conn
            .query_opt(
                "SELECT jsonb_agg(c) FROM article_commentaire ac \
                 CROSS JOIN LATERAL jsonb_array_elements(ac.source_fields->'commentaires') c \
                 WHERE ac.text_slug = $1 AND (ac.num_key = $2 OR ac.num_key IS NULL) \
                   AND ac.deleted_at IS NULL \
                   AND jsonb_typeof(ac.source_fields->'commentaires') = 'array'",
                &[&text_slug, &num_key],
            )
            .await?;
        Ok(row.and_then(|r| r.get::<_, Option<Value>>(0)))
    }

    /// Résout une décision par (n° de dossier, date de lecture) — le n° seul n'est
    /// pas unique. Renvoie les `(id, public_id)` correspondants, plusieurs si des
    /// dossiers homonymes partagent la date.
    #[tracing::instrument(name = "db.decisions_by_docket_date", skip(self), fields(db.system = "postgresql"))]
    pub async fn decisions_by_docket_date(
        &self,
        docket: &str,
        date_iso: &str,
    ) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, public_id FROM decisions \
                 WHERE $1 = ANY(docket_numbers) AND date_lecture::text = $2 \
                   AND deleted_at IS NULL",
                &[&docket, &date_iso],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Checksums des commentaires ADDE déjà en base (`source = 'adde'`), par
    /// `source_uid` — skip d'idempotence avant upsert.
    #[tracing::instrument(name = "db.adde_checksums", skip(self), fields(db.system = "postgresql"))]
    pub async fn adde_checksums(&self) -> Result<HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, content_checksum FROM decision_sources \
                 WHERE source = 'adde' AND deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
            .collect())
    }
}
